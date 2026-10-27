//! Menu clicks are available before the simulation, using the same native mixer/device.
use super::*;
use ara_audio::{
    AudioEngine, Clip, Playback, VoiceParameters,
    codec::{MediaDecoder, decode_clip},
    device::SdlBackend,
};

pub(super) struct UiAudio {
    engine: AudioEngine,
    clip: Clip,
    device: SdlBackend,
}
impl UiAudio {
    pub(super) fn open(bytes: &[u8]) -> Result<Option<Self>> {
        if bytes.is_empty() {
            return Ok(None);
        }
        let Ok(device) = SdlBackend::open(48_000) else {
            return Ok(None);
        };
        let mut decoder =
            MediaDecoder::open(Box::new(std::io::Cursor::new(bytes.to_vec())), "ogg")?;
        let clip = decode_clip(&mut decoder, 1024 * 1024)?;
        Ok(Some(Self {
            engine: AudioEngine::new(Default::default())?,
            clip,
            device,
        }))
    }
    pub(super) fn click(&mut self, gain: f32) -> Result<()> {
        let playback = Playback {
            at_frame: self.engine.next_frame(),
            spatial: false,
            parameters: VoiceParameters {
                gain,
                ..Default::default()
            },
            ..Default::default()
        };
        let bus = self.engine.bus("master").context("UI master bus")?;
        // The immutable clip shares PCM storage only when a UI action starts a voice.
        self.engine.play(bus, self.clip.clone(), playback)?;
        Ok(())
    }
    pub(super) fn pump(&mut self) -> Result<()> {
        ara_audio::device::pump(&mut self.engine, &mut self.device, crate::AUDIO_WATERMARK)?;
        Ok(())
    }
}
