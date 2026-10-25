//! Resolve optional encoders before cache lookup and retain inputs for safe retry.
use super::{ImportJob, Message, ProcessWorker, Request};
use crate::{Cancellation, Digest, Error, ImportInput, ImportOutput, Progress, Result};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Default)]
pub(super) struct State {
    identity: OnceLock<Option<String>>,
    failed: AtomicBool,
}

impl ProcessWorker {
    /// Resolves an optional texture encoder on a background/CLI thread. Old or
    /// CPU-only workers retain the original CPU action keys without an error.
    pub fn texture_encoder(&self, input: &ImportInput) -> Option<&str> {
        if input.importer == "ara.texture" && input.settings.get("usage").and_then(serde_json::Value::as_str) == Some("normal") {
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
            .get_or_init(|| self.query_encoder().ok().flatten())
            .as_deref()
    }

    fn query_encoder(&self) -> Result<Option<String>> {
        let mut job = self.start_request(Request::Encoder)?;
        job.timeout = self.timeout.min(std::time::Duration::from_secs(15));
        loop {
            match job.message()? {
                Some(Message::Encoder(identity)) => return Ok(identity),
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
        let result = self
            .start_encoded(Arc::clone(&input), backend)?
            .wait(cancellation, &mut progress);
        match result {
            Err(Error::EncoderUnavailable(reason)) if backend.is_some() => {
                self.encoder.failed.store(true, Ordering::Release);
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
