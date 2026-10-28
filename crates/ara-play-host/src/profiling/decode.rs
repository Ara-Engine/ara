//! Bounded capture decoding never runs on the host's frame thread.
use super::*;

pub(super) struct Incoming {
    pub world: String,
    pub player: usize,
    pub source: String,
    pub value: Value,
}
pub(super) struct Decoded {
    pub world: String,
    pub player: usize,
    pub source: String,
    pub ticket: u64,
    pub result: Result<Reply, String>,
}
pub(super) struct Decoder {
    tx: SyncSender<Incoming>,
    rx: Receiver<Decoded>,
}
impl Decoder {
    pub fn new() -> std::io::Result<Self> {
        let (tx, work) = mpsc::sync_channel::<Incoming>(4);
        let (done, rx) = mpsc::sync_channel(4);
        std::thread::Builder::new()
            .name("ara-play-profile-decode".into())
            .spawn(move || {
                while let Ok(input) = work.recv() {
                    let ticket = input.value["ticket"].as_u64().unwrap_or(0);
                    let result = serde_json::from_value::<Reply>(input.value)
                        .map_err(|error| error.to_string());
                    if done
                        .send(Decoded {
                            world: input.world,
                            player: input.player,
                            source: input.source,
                            ticket,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok(Self { tx, rx })
    }
    pub fn submit(&self, input: Incoming) -> Result<(), Box<Incoming>> {
        self.tx.try_send(input).map_err(|error| match error {
            mpsc::TrySendError::Full(input) | mpsc::TrySendError::Disconnected(input) => {
                Box::new(input)
            }
        })
    }
    pub fn poll(&self) -> Option<Decoded> {
        self.rx.try_recv().ok()
    }
}
