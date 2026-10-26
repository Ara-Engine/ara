//! Pure dependency results are memoized; cold concurrent queries share batches.
use super::{Message, ProcessWorker, Request};
use crate::{
    Artifact, ArtifactKind, CacheBackend, Cancellation, Digest, Error, FileCache, ImportOutput,
    ImporterRegistry, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Mutex, OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Input {
    pub importer: String,
    pub version: String,
    #[serde(with = "serde_bytes")]
    pub source: Vec<u8>,
    pub settings: serde_json::Value,
}
struct Item {
    input: Input,
    cancellation: Cancellation,
    reply: mpsc::Sender<Result<Vec<String>>>,
}
#[derive(Debug, Default)]
pub(super) struct State {
    memo: OnceLock<Option<FileCache>>,
    sender: Mutex<Option<mpsc::Sender<Item>>>,
}
pub(super) fn execute(registry: &ImporterRegistry, input: &Input) -> Result<Vec<String>> {
    let importer = registry.get(&input.importer)?;
    if importer.version() != input.version {
        return Err(Error::Invalid("importer version mismatch".into()));
    }
    importer.dependencies_with_settings(&input.source, &input.settings)
}
impl ProcessWorker {
    pub(super) fn dependencies(
        &self,
        input: Input,
        cancellation: &Cancellation,
    ) -> Result<Vec<String>> {
        cancellation.check()?;
        let memo = self
            .discovery
            .memo
            .get_or_init(|| {
                self.memo
                    .as_ref()
                    .and_then(|root| FileCache::at(root.join("discovery")).ok())
            })
            .as_ref();
        let key = memo.map(|_| {
            let arguments: Vec<_> = self
                .arguments
                .iter()
                .map(|arg| Digest::of(arg.as_encoded_bytes()))
                .collect();
            let value = serde_json::json!([
                "Ara dependency discovery v1",
                input.importer,
                input.version,
                Digest::of(&input.source),
                input.settings,
                arguments
            ]);
            Digest::of(ara_json::to_canonical_string(&ara_json::sort_keys(&value)).as_bytes())
        });
        if let (Some(cache), Some(key)) = (memo, key)
            && let Ok(Some(hit)) = cache.load(key)
            && hit.output.artifacts.len() == 1
            && hit.output.artifacts[0].name == "dependencies.json"
            && let Ok(dependencies) = serde_json::from_slice(&hit.output.artifacts[0].bytes)
        {
            cancellation.check()?;
            crate::trace::event(
                "worker.discovery_cache_hit",
                &input.importer,
                serde_json::json!({}),
            );
            return Ok(dependencies);
        }
        let (reply, received) = mpsc::channel();
        let item = Item {
            input,
            cancellation: cancellation.clone(),
            reply,
        };
        let mut sender = self
            .discovery
            .sender
            .lock()
            .map_err(|_| Error::Worker("discovery queue poisoned".into()))?;
        if sender.is_none() {
            let (tx, rx) = mpsc::channel();
            // Do not capture our own sender/state: dropping the last caller must
            // close the queue and release the broker's encoder lease as well.
            let mut worker = self.clone();
            worker.discovery = std::sync::Arc::default();
            thread::Builder::new()
                .name("ara-discovery".into())
                .spawn(move || batches(worker, rx))?;
            *sender = Some(tx);
        }
        if let Err(error) = sender
            .as_ref()
            .expect("initialized discovery sender")
            .send(item)
        {
            *sender = None;
            drop(sender);
            return self.discover_one(error.0.input, cancellation);
        }
        drop(sender);
        let dependencies = loop {
            cancellation.check()?;
            match received.recv_timeout(Duration::from_millis(2)) {
                Ok(result) => break result?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Error::Worker("discovery queue closed".into()));
                }
            }
        };
        if let (Some(cache), Some(key)) = (memo, key) {
            let value = serde_json::to_value(&dependencies)?;
            let bytes = ara_json::to_canonical_string(&ara_json::sort_keys(&value)).into_bytes();
            // Memo storage is optional; a cache error cannot change discovery.
            let _ = cache.store(
                key,
                &ImportOutput {
                    artifacts: vec![Artifact {
                        name: "dependencies.json".into(),
                        kind: ArtifactKind::Custom,
                        bytes,
                    }],
                },
            );
        }
        Ok(dependencies)
    }

    fn discover_one(&self, input: Input, cancellation: &Cancellation) -> Result<Vec<String>> {
        let mut job = self.start_request(Request::Discover {
            importer: input.importer,
            version: input.version,
            source: input.source,
            settings: input.settings,
        })?;
        loop {
            cancellation.check()?;
            match job.message()? {
                Some(Message::Dependencies(dependencies)) => return Ok(dependencies),
                Some(Message::Failed(error)) => return Err(Error::Worker(error)),
                Some(_) => return Err(Error::Worker("unexpected dependency reply".into())),
                None => thread::sleep(Duration::from_millis(2)),
            }
        }
    }

    fn discover_batch(
        &self,
        items: &[Item],
    ) -> Result<Vec<std::result::Result<Vec<String>, String>>> {
        let mut job = self.start_request(Request::DiscoverBatch(
            items.iter().map(|item| item.input.clone()).collect(),
        ))?;
        job.timeout = self.timeout.min(Duration::from_secs(30));
        loop {
            if items.iter().all(|item| item.cancellation.is_cancelled()) {
                return Err(Error::Cancelled);
            }
            match job.message()? {
                Some(Message::DiscoveryBatch(results)) if results.len() == items.len() => {
                    return Ok(results);
                }
                Some(Message::Failed(error)) => return Err(Error::Worker(error)),
                Some(_) => return Err(Error::Worker("unexpected dependency batch reply".into())),
                None => thread::sleep(Duration::from_millis(2)),
            }
        }
    }
}

fn batches(worker: ProcessWorker, received: mpsc::Receiver<Item>) {
    let mut pending = None;
    while let Some(first) = pending.take().or_else(|| received.recv().ok()) {
        let mut bytes = first.input.source.len();
        let mut items = vec![first];
        let deadline = Instant::now() + Duration::from_millis(2);
        while items.len() < 128 {
            let Some(wait) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let Ok(next) = received.recv_timeout(wait) else {
                break;
            };
            if bytes.saturating_add(next.input.source.len()) > 8 * 1024 * 1024 {
                pending = Some(next);
                break;
            }
            bytes += next.input.source.len();
            items.push(next);
        }
        items.retain(|item| !item.cancellation.is_cancelled());
        if items.is_empty() {
            continue;
        }
        crate::trace::event(
            "worker.discovery_batch",
            "metadata",
            serde_json::json!({"requests":items.len(),"bytes":bytes}),
        );
        if let Ok(results) = worker.discover_batch(&items) {
            for (item, result) in items.into_iter().zip(results) {
                let _ = item.reply.send(result.map_err(Error::Worker));
            }
        } else {
            // Older workers do not know the additive batch request. A crashing
            // parser is also retried individually so it cannot lose other items.
            for item in items {
                let _ = item
                    .reply
                    .send(worker.discover_one(item.input, &item.cancellation));
            }
        }
    }
}
