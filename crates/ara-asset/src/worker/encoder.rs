//! Resolve optional encoders before cache lookup and retain inputs for safe retry.
use super::{ImportJob, Message, ProcessWorker, Request};
use crate::{Cancellation, Digest, Error, ImportInput, ImportOutput, Progress, Result};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Default)]
pub(super) struct State {
    identity: OnceLock<Option<String>>,
    failed: AtomicBool,
    service: Mutex<Option<std::net::TcpStream>>,
    prepared: Mutex<bool>,
    hint_probed: AtomicBool,
}

impl ProcessWorker {
    /// Resolves an optional texture encoder on a background/CLI thread. Old or
    /// CPU-only workers retain the original CPU action keys without an error.
    pub fn texture_encoder(&self, input: &ImportInput) -> Option<&str> {
        if !self.texture_encoder_enabled {
            return None;
        }
        if input.importer == "ara.texture"
            && input
                .settings
                .get("usage")
                .and_then(serde_json::Value::as_str)
                == Some("normal")
        {
            return None;
        }
        if !matches!(
            input.importer.as_str(),
            "ara.texture" | "ara.gltf" | "ara.fbx"
        ) || self.encoder.failed.load(Ordering::Acquire)
        {
            return None;
        }
        self.encoder
            .identity
            .get_or_init(|| match self.resolve_encoder() {
                Ok(identity) => identity,
                Err(error) => {
                    crate::trace::event(
                        "texture.backend_unavailable",
                        "encoder",
                        serde_json::json!({"reason":error.to_string()}),
                    );
                    None
                }
            })
            .as_deref()
    }

    fn resolve_encoder(&self) -> Result<Option<String>> {
        // Additive probing keeps old/custom workers compatible. A valid hint
        // selects action keys immediately; only a miss needs device preparation.
        if !self.encoder.hint_probed.swap(true, Ordering::AcqRel)
            && let Ok(Some(identity)) = self.query_encoder_hint()
        {
            crate::trace::event(
                "texture.identity_cache_hit",
                "encoder",
                serde_json::json!({}),
            );
            return Ok(Some(identity));
        }
        let identity = self.query_encoder()?;
        if identity.is_some() {
            *self
                .encoder
                .prepared
                .lock()
                .map_err(|_| Error::Worker("encoder preparation poisoned".into()))? = true;
        }
        Ok(identity)
    }

    pub(super) fn accept_encoder_hint(&self, identity: Option<String>) {
        self.encoder.hint_probed.store(true, Ordering::Release);
        if let Some(identity) = identity
            && self.encoder.identity.set(Some(identity)).is_ok()
        {
            crate::trace::event(
                "texture.identity_cache_hit",
                "metadata",
                serde_json::json!({}),
            );
        }
    }

    fn query_encoder_hint(&self) -> Result<Option<String>> {
        let mut job = self.start_request(Request::EncoderHint)?;
        job.timeout = self.timeout.min(std::time::Duration::from_secs(30));
        loop {
            match job.message()? {
                Some(Message::EncoderHint(identity)) => return Ok(identity),
                Some(_) => return Ok(None),
                None => std::thread::sleep(std::time::Duration::from_millis(2)),
            }
        }
    }

    fn prepare_encoder(&self, expected: Option<&str>) -> Result<()> {
        let Some(expected) = expected else {
            return Ok(());
        };
        let mut prepared = self
            .encoder
            .prepared
            .lock()
            .map_err(|_| Error::Worker("encoder preparation poisoned".into()))?;
        if *prepared {
            return Ok(());
        }
        let actual = self
            .query_encoder()
            .map_err(|error| Error::EncoderUnavailable(error.to_string()))?;
        if actual.as_deref() != Some(expected) {
            return Err(Error::EncoderUnavailable(
                "cached encoder identity changed during miss negotiation".into(),
            ));
        }
        *prepared = true;
        Ok(())
    }

    fn query_encoder(&self) -> Result<Option<String>> {
        let mut job = self.start_request(Request::Encoder)?;
        // Cold driver compilation happens once here, rather than racing in
        // every below-normal-priority asset worker on the first batch.
        job.timeout = self.timeout.min(std::time::Duration::from_secs(60));
        loop {
            match job.message()? {
                Some(Message::Encoder(identity)) => return Ok(identity),
                Some(Message::EncoderService { identity, endpoint }) => {
                    let lease = endpoint.lease()?;
                    *self
                        .encoder
                        .service
                        .lock()
                        .map_err(|_| Error::Worker("encoder lease poisoned".into()))? = Some(lease);
                    return Ok(Some(identity));
                }
                Some(Message::Failed(error)) => return Err(Error::Worker(error)),
                Some(_) => return Err(Error::Worker("unexpected encoder reply".into())),
                None => std::thread::sleep(std::time::Duration::from_millis(2)),
            }
        }
    }

    /// Starts an explicit encoder without copying model dependencies. None uses
    /// the original CPU wire request, including compatibility with older workers.
    pub fn start_encoded(
        &self,
        input: Arc<ImportInput>,
        backend: Option<&str>,
    ) -> Result<ImportJob> {
        let request = match backend {
            Some(backend) => Request::ImportEncoded {
                input,
                backend: backend.into(),
            },
            None => Request::Import(input),
        };
        self.start_request(request)
    }

    /// Runs the selected encoder, retrying a failed optional backend under the
    /// CPU identity. No output is ever published under a backend that did not run.
    pub fn encode_with_fallback(
        &self,
        input: Arc<ImportInput>,
        cancellation: &Cancellation,
        mut progress: impl FnMut(Progress),
    ) -> Result<(Digest, ImportOutput)> {
        let backend = self.texture_encoder(&input);
        let key = input.key_with_backend(backend)?;
        let result = self.prepare_encoder(backend).and_then(|()| {
            self.start_encoded(Arc::clone(&input), backend)?
                .wait(cancellation, &mut progress)
        });
        match result {
            Err(Error::EncoderUnavailable(reason)) if backend.is_some() => {
                self.encoder.failed.store(true, Ordering::Release);
                if let Ok(mut service) = self.encoder.service.lock() {
                    service.take();
                }
                crate::trace::event(
                    "texture.backend_fallback",
                    &input.source_path,
                    serde_json::json!({"reason":reason}),
                );
                cancellation.check()?;
                let key = input.key()?;
                let output = self
                    .start_encoded(input, None)?
                    .wait(cancellation, progress)?;
                Ok((key, output))
            }
            result => result.map(|output| (key, output)),
        }
    }
}

// Arc affects ownership only. Its wire representation is exactly the existing
// ImportInput value, so legacy workers still accept ordinary CPU requests.
pub(super) mod shared_input {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub(in crate::worker) fn serialize<S: Serializer>(
        input: &Arc<ImportInput>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        input.as_ref().serialize(serializer)
    }
    pub(in crate::worker) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Arc<ImportInput>, D::Error> {
        ImportInput::deserialize(deserializer).map(Arc::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    enum LegacyRequest {
        Import(ImportInput),
        Discover {
            importer: String,
            version: String,
            #[serde(with = "serde_bytes")]
            source: Vec<u8>,
            settings: serde_json::Value,
        },
        Versions,
    }

    #[test]
    fn optional_encoder_protocol_preserves_all_legacy_cpu_requests() {
        let input = ImportInput {
            preset: crate::CookPreset::Balanced,
            thread_budget: 2,
            source_path: "fixture.bin".into(),
            source: vec![0, 127, 255],
            importer: "fixture".into(),
            version: "fixture-1".into(),
            settings: serde_json::Value::Null,
            target: crate::TargetPlatform::Windows,
            dependencies: Default::default(),
            asset_dependencies: Default::default(),
        };
        let discover = Request::Discover {
            importer: input.importer.clone(),
            version: input.version.clone(),
            source: input.source.clone(),
            settings: input.settings.clone(),
        };
        let legacy_discover = LegacyRequest::Discover {
            importer: input.importer.clone(),
            version: input.version.clone(),
            source: input.source.clone(),
            settings: input.settings.clone(),
        };
        for (current, legacy) in [
            (
                Request::Import(Arc::new(input.clone())),
                LegacyRequest::Import(input),
            ),
            (discover, legacy_discover),
            (Request::Versions, LegacyRequest::Versions),
        ] {
            let bytes = rmp_serde::to_vec_named(&current).expect("current wire");
            assert_eq!(
                bytes,
                rmp_serde::to_vec_named(&legacy).expect("legacy wire")
            );
            let decoded: Request = rmp_serde::from_slice(&bytes).expect("current decode");
            assert_eq!(
                bytes,
                rmp_serde::to_vec_named(&decoded).expect("round trip")
            );
        }
    }
}
