//! Cooked audio stays compressed; only bounded decoder queues hold streamed PCM.
use crate::{Result, invalid};
use ara_audio::{
    Clip, Frame, Playback, PreparedVoice,
    codec::{AudioDecoder, MediaDecoder, OpusDecoder, decode_clip},
    streaming::{self, StreamBudget, StreamWorker, StreamWriter},
};
use ara_runtime::assets::{AssetSource, asset_id};
use ara_schema::AssetRef;
use std::{io::Cursor, sync::Arc};

/// Control-side asset loader. Graph samplers request resident clips; ordinary
/// sources and music request prepared voices, allowing long assets to stream.
pub trait VoiceLoader {
    /// Loads a bounded resident clip for a graph sampler or short sound.
    fn clip(&mut self, reference: &AssetRef) -> Result<Clip>;
    /// Prepares a voice outside the audio render callback.
    fn prepare(&mut self, reference: &AssetRef, playback: Playback) -> Result<PreparedVoice> {
        PreparedVoice::resident(self.clip(reference)?, playback)
    }
}

impl<F: FnMut(&AssetRef) -> Result<Clip>> VoiceLoader for F {
    fn clip(&mut self, reference: &AssetRef) -> Result<Clip> {
        self(reference)
    }
}

/// Streaming producer scheduling. Offline rendering must refill synchronously.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamMode {
    /// Prebuffer, then decode on background workers during device playback.
    Live,
    /// Decode only when the isolated offline renderer requests another quantum.
    Offline,
}

/// PCM queue frames per voice (32 KiB stereo plus bounded queue bookkeeping).
const QUEUE_FRAMES: usize = 4096;
/// Default aggregate PCM queue budget. Compressed assets and codec scratch are separate.
pub const STREAM_BUDGET: usize = 8 * 1024 * 1024;

enum Producer {
    Live(StreamWorker),
    Offline {
        writer: StreamWriter,
        decoder: Box<dyn AudioDecoder>,
    },
}

/// Asset-backed voice preparation with a hard aggregate PCM queue budget.
/// Own and drop this loader on a control worker, never inside the audio callback.
pub struct AssetVoices {
    assets: Arc<dyn AssetSource>,
    mode: StreamMode,
    budget: StreamBudget,
    producers: Vec<Producer>,
}

impl AssetVoices {
    /// Creates a loader for pinned imported generations. `budget_bytes` limits PCM
    /// queues, independently of compressed storage and the codec's packet scratch.
    pub fn new(assets: Arc<dyn AssetSource>, mode: StreamMode, budget_bytes: usize) -> Self {
        Self {
            assets,
            mode,
            budget: StreamBudget::new(budget_bytes),
            producers: Vec::new(),
        }
    }

    /// Uses current Play pins for subsequently prepared voices. Playing voices keep
    /// their immutable source generation until they finish.
    pub fn set_assets(&mut self, assets: Arc<dyn AssetSource>) {
        self.assets = assets;
    }

    /// Bytes reserved by live PCM queues, including queues draining after EOF.
    pub fn queued_bytes(&self) -> usize {
        self.budget.used_bytes()
    }

    pub(crate) fn require_offline(&self) -> Result<()> {
        if self.mode != StreamMode::Offline {
            return Err(invalid(
                "offline capture requires synchronous stream producers".into(),
            ));
        }
        Ok(())
    }

    /// Refills offline queues and reaps completed producers on the control path.
    /// Call before each isolated render quantum; live workers refill independently.
    pub fn refill(&mut self) -> Result<()> {
        let mut index = 0;
        while index < self.producers.len() {
            let finished = match &mut self.producers[index] {
                Producer::Live(worker) => worker.is_finished(),
                Producer::Offline { writer, decoder } => {
                    while writer.fill(decoder.as_mut())? != 0 {}
                    writer.is_finished()
                }
            };
            if finished {
                if let Producer::Live(worker) = self.producers.swap_remove(index) {
                    worker.finish()?;
                }
            } else {
                index += 1;
            }
        }
        Ok(())
    }
}

struct Source {
    bytes: Arc<[u8]>,
    opus: bool,
    streamed: bool,
    loudness: Option<f32>,
    peak: Option<f32>,
}

impl Source {
    fn load(assets: &dyn AssetSource, reference: &AssetRef) -> Result<Self> {
        let id = asset_id(reference).map_err(|e| invalid(e.0))?;
        let (_, cached) = assets
            .load(id)
            .map_err(|e| invalid(e.0))?
            .ok_or_else(|| invalid(format!("sound {} is not imported", reference.0)))?;
        let meta = cached
            .output
            .artifacts
            .iter()
            .find(|a| a.name == "audio.json")
            .ok_or_else(|| invalid(format!("sound {} has no audio.json", reference.0)))?;
        let meta: serde_json::Value = serde_json::from_slice(&meta.bytes)
            .map_err(|e| invalid(format!("sound {}: {e}", reference.0)))?;
        let bytes = cached
            .output
            .artifacts
            .into_iter()
            .find(|a| a.name == "audio.bin")
            .ok_or_else(|| invalid(format!("sound {} has no audio.bin", reference.0)))?
            .bytes;
        let opus = meta["codec"] == "opus";
        Ok(Self {
            bytes: bytes.into(),
            opus,
            // Legacy Opus imports predate the explicit streamed flag.
            streamed: meta["streamed"].as_bool().unwrap_or(opus),
            loudness: meta["loudness"].as_f64().map(|v| v as f32),
            peak: meta["peak"].as_f64().map(|v| v as f32),
        })
    }

    fn decoder(&self) -> Result<Box<dyn AudioDecoder>> {
        let bytes = Cursor::new(Arc::clone(&self.bytes));
        if self.opus {
            Ok(Box::new(OpusDecoder::open(bytes)?))
        } else {
            Ok(Box::new(MediaDecoder::open(Box::new(bytes), "wav")?))
        }
    }
}

/// The explicit resident API retains its 64 MiB limit; streamed consumers use prepare.
pub(crate) fn resident(assets: &dyn AssetSource, reference: &AssetRef) -> Result<Clip> {
    decode_clip(
        Source::load(assets, reference)?.decoder()?.as_mut(),
        64 * 1024 * 1024,
    )
}

/// Reopening a compressed generation at EOF keeps loops on the decoder producer.
struct Looping {
    source: Source,
    decoder: Box<dyn AudioDecoder>,
    looping: bool,
}

impl AudioDecoder for Looping {
    fn sample_rate(&self) -> u32 {
        self.decoder.sample_rate()
    }

    fn read_frames(&mut self, out: &mut [Frame]) -> Result<usize> {
        let read = self.decoder.read_frames(out)?;
        if read != 0 || !self.looping || out.is_empty() {
            return Ok(read);
        }
        self.decoder = self.source.decoder()?;
        let read = self.decoder.read_frames(out)?;
        if read == 0 {
            return Err(invalid("cannot loop an empty audio stream".into()));
        }
        Ok(read)
    }
}

impl VoiceLoader for AssetVoices {
    fn clip(&mut self, reference: &AssetRef) -> Result<Clip> {
        resident(self.assets.as_ref(), reference)
    }

    fn prepare(&mut self, reference: &AssetRef, mut playback: Playback) -> Result<PreparedVoice> {
        self.refill()?;
        let source = Source::load(self.assets.as_ref(), reference)?;
        if !source.streamed {
            return PreparedVoice::resident(
                decode_clip(source.decoder()?.as_mut(), 64 * 1024 * 1024)?,
                playback,
            );
        }
        let (loudness, peak) = (source.loudness, source.peak);
        let decoder = source.decoder()?;
        let mut decoder: Box<dyn AudioDecoder> = Box::new(Looping {
            source,
            decoder,
            looping: playback.looping,
        });
        playback.looping = false;
        let (reader, producer) = match self.mode {
            StreamMode::Live => {
                let (reader, worker) = streaming::spawn(decoder, QUEUE_FRAMES, &self.budget)?;
                (reader, Producer::Live(worker))
            }
            StreamMode::Offline => {
                let (mut writer, reader) =
                    streaming::channel(decoder.sample_rate(), QUEUE_FRAMES, &self.budget)?;
                while writer.fill(decoder.as_mut())? != 0 {}
                (reader, Producer::Offline { writer, decoder })
            }
        };
        let prepared = PreparedVoice::streaming(reader, playback)?.with_loudness(loudness, peak);
        self.producers.push(producer);
        Ok(prepared)
    }
}
