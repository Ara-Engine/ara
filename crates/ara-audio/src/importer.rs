//! Pure audio cooking contract, ready for an ara-asset worker adapter.

use crate::{
    AudioError, Clip, Frame, Result,
    codec::{MediaDecoder, OpusDecoder, decode_clip},
};
use ogg::PacketWriteEndInfo;
use std::io::Cursor;

/// Runtime content codec, shared by every target platform (R08 §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookCodec {
    /// Ogg Opus for music and dialogue, always decoded at 48 kHz.
    Opus,
    /// Stereo IMA ADPCM in WAV for short, memory-sensitive sound effects.
    Adpcm,
    /// Signed 16-bit PCM WAV for the shortest latency-critical sounds.
    Pcm,
}
/// Authoring use determines the codec family, independent of target OS.
#[derive(Debug, Clone, Copy)]
pub enum AudioUse {
    /// Streamed music.
    Music,
    /// Streamed speech.
    Dialogue,
    /// Resident, latency-critical sound effects.
    ShortEffect,
}
/// Stable, hashable cook settings belong in the asset cache key alongside importer version.
#[derive(Debug, Clone, Copy)]
pub struct CookSettings {
    /// Content use, selecting the R08 codec policy.
    pub usage: AudioUse,
    /// PCM or ADPCM for ShortEffect. Opus is rejected in this lane.
    pub short_codec: CookCodec,
    /// Opus bits per second for stereo; mobile quality changes this, not the codec set.
    pub opus_bitrate: i32,
    /// Resident stereo PCM staging budget. Music/dialogue use bounded blocks and
    /// require at least 65536 bytes; compressed payloads and codec/meter scratch are separate.
    pub decoded_budget: usize,
}
impl Default for CookSettings {
    fn default() -> Self {
        Self {
            usage: AudioUse::ShortEffect,
            short_codec: CookCodec::Pcm,
            opus_bitrate: 128000,
            decoded_budget: 64 * 1024 * 1024,
        }
    }
}
/// Source bytes and settings delivered by an asset worker.
pub struct ImportInput<'a> {
    /// Lowercase extension without a dot.
    pub extension: &'a str,
    /// Immutable source media bytes; pack IO stays outside the importer.
    pub bytes: &'a [u8],
    /// Explicit content use and memory/quality policy.
    pub settings: CookSettings,
}
/// Cooked content-addressable artifact; never written to disk by an importer.
pub struct ImportOutput {
    /// Encoded payload for the pack builder.
    pub bytes: Vec<u8>,
    /// Runtime decoding family.
    pub codec: CookCodec,
    /// Frames per second after cooking.
    pub sample_rate: u32,
    /// BLAKE3 of the cooked bytes.
    pub hash: String,
    /// Integrated loudness (LUFS) of the decoded source, for bus loudness normalization.
    /// None for silence and sources shorter than 400 ms.
    pub loudness: Option<f32>,
    /// Largest absolute decoded sample, bounding normalization boosts.
    pub peak: f32,
}
/// Local worker contract mirroring ara-asset's pure importer boundary.
/// The eventual adapter translates the asset crate's request/artifact envelopes only.
pub trait Importer {
    /// Stable importer identity for the derived-data cache.
    fn id(&self) -> &'static str;
    /// Increment when cooking output or settings semantics change.
    fn version(&self) -> u32;
    /// Accepted source extensions.
    fn extensions(&self) -> &'static [&'static str];
    /// Pure conversion; no device access, globals, filesystem writes or authoring edits.
    fn import(&self, input: &ImportInput<'_>) -> Result<ImportOutput>;
}
/// Audio worker implementation of the R08 content policy.
pub struct AudioImporter;
impl Importer for AudioImporter {
    fn id(&self) -> &'static str {
        "ara.audio"
    }
    fn version(&self) -> u32 {
        // v4: bounded PCM staging for streamed music/dialogue instead of whole-clip decoding.
        4
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["wav", "flac", "ogg", "vorbis", "opus"]
    }
    fn import(&self, input: &ImportInput<'_>) -> Result<ImportOutput> {
        let mut decoder: Box<dyn crate::codec::AudioDecoder> = if input.extension == "opus" {
            Box::new(OpusDecoder::open(Cursor::new(input.bytes.to_vec()))?)
        } else {
            Box::new(MediaDecoder::open(
                Box::new(Cursor::new(input.bytes.to_vec())),
                input.extension,
            )?)
        };
        let codec = match input.settings.usage {
            AudioUse::Music | AudioUse::Dialogue => CookCodec::Opus,
            AudioUse::ShortEffect => match input.settings.short_codec {
                CookCodec::Opus => {
                    return Err(AudioError::Invalid(
                        "short SFX must use PCM or ADPCM under R08",
                    ));
                }
                codec => codec,
            },
        };
        let (bytes, sample_rate, loudness, peak) = if codec == CookCodec::Opus {
            let mut source =
                crate::import_stream::Source::new(decoder.as_mut(), input.settings.decoded_budget)?;
            let bytes = opus_ogg(|| source.next(), input.settings.opus_bitrate)?;
            let (loudness, peak) = source.measurement()?;
            (bytes, 48000, loudness, peak)
        } else {
            let clip = decode_clip(decoder.as_mut(), input.settings.decoded_budget)?;
            let loudness = crate::loudness::integrated(clip.sample_rate(), clip.frames())?;
            let bytes = if codec == CookCodec::Pcm {
                pcm_wav(clip.sample_rate(), clip.frames())?
            } else {
                adpcm_wav(&clip)?
            };
            (bytes, clip.sample_rate(), loudness, clip.peak())
        };
        let hash = blake3::hash(&bytes).to_hex().to_string();
        Ok(ImportOutput {
            bytes,
            codec,
            sample_rate,
            hash,
            loudness,
            peak,
        })
    }
}

/// Writes standard stereo PCM16 WAV without nondeterministic dithering.
pub fn pcm_wav(rate: u32, frames: &[Frame]) -> Result<Vec<u8>> {
    let size = frames
        .len()
        .checked_mul(4)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(AudioError::Capacity("WAV size"))?;
    if size > u32::MAX - 36 {
        return Err(AudioError::Capacity("RIFF size"));
    }
    let mut out = Vec::with_capacity(size as usize + 44);
    out.extend(b"RIFF");
    out.extend((36 + size).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16_u32.to_le_bytes());
    out.extend(1_u16.to_le_bytes());
    out.extend(2_u16.to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * 4).to_le_bytes());
    out.extend(4_u16.to_le_bytes());
    out.extend(16_u16.to_le_bytes());
    out.extend(b"data");
    out.extend(size.to_le_bytes());
    for frame in frames {
        out.extend(pcm(frame.left).to_le_bytes());
        out.extend(pcm(frame.right).to_le_bytes());
    }
    Ok(out)
}
fn pcm(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

fn opus_ogg(mut next: impl FnMut() -> Result<Option<Frame>>, bitrate: i32) -> Result<Vec<u8>> {
    if !(6000..=512000).contains(&bitrate) {
        return Err(AudioError::Invalid("Opus bitrate"));
    }
    let err = |e: ara_opus_sys::Error| AudioError::Codec(e.to_string());
    let mut encoder = ara_opus_sys::Encoder::new(48000, 2).map_err(err)?;
    encoder.set_bitrate(bitrate).map_err(err)?;
    let skip = encoder.get_lookahead().map_err(err)? as u16;
    let mut head = b"OpusHead".to_vec();
    head.extend([1, 2]);
    head.extend(skip.to_le_bytes());
    head.extend(48000_u32.to_le_bytes());
    head.extend([0, 0, 0]);
    let mut tags = b"OpusTags".to_vec();
    tags.extend(9_u32.to_le_bytes());
    tags.extend(b"ara-audio");
    tags.extend(0_u32.to_le_bytes());
    let mut out = Vec::new();
    let mut writer = ogg::PacketWriter::new(&mut out);
    writer.write_packet(head, 1, PacketWriteEndInfo::EndPage, 0)?;
    writer.write_packet(tags, 1, PacketWriteEndInfo::EndPage, 0)?;
    let mut pcm = [0.0; 1920];
    let mut encoded = [0_u8; 4000];
    let (mut frames, mut offset, mut eof) = (0_u64, 0_u64, false);
    loop {
        pcm.fill(0.0);
        for pair in pcm.as_chunks_mut::<2>().0 {
            if eof {
                break;
            }
            match next()? {
                Some(frame) => {
                    *pair = [frame.left, frame.right];
                    frames += 1;
                }
                None => {
                    eof = true;
                    break;
                }
            }
        }
        if eof && frames == 0 {
            return Err(AudioError::Invalid("empty audio source"));
        }
        let len = encoder.encode_float(&pcm, &mut encoded).map_err(err)?;
        offset += 960;
        let last = eof && offset >= frames + u64::from(skip);
        let granule = if last {
            frames + u64::from(skip)
        } else {
            offset
        };
        writer.write_packet(
            encoded[..len].to_vec(),
            1,
            if last {
                PacketWriteEndInfo::EndStream
            } else {
                PacketWriteEndInfo::EndPage
            },
            granule,
        )?;
        if last {
            break;
        }
    }
    Ok(out)
}

// IMA ADPCM's specified index/step tables. Each stereo WAV block contains 249 frames.
const INDEX: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];
const STEP: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
fn nibble(sample: i16, predictor: &mut i32, index: &mut i32) -> u8 {
    let delta = sample as i32 - *predictor;
    let step = STEP[*index as usize];
    let mut remainder = delta.abs();
    let mut code = if delta < 0 { 8 } else { 0 };
    let mut reconstructed = step / 8;
    for (bit, part) in [(4, step), (2, step / 2), (1, step / 4)] {
        if remainder >= part {
            code |= bit;
            remainder -= part;
            reconstructed += part;
        }
    }
    *predictor = (*predictor
        + if code & 8 != 0 {
            -reconstructed
        } else {
            reconstructed
        })
    .clamp(-32768, 32767);
    *index = (*index + INDEX[code as usize]).clamp(0, 88);
    code
}
fn adpcm_wav(clip: &Clip) -> Result<Vec<u8>> {
    let blocks = clip.len().div_ceil(249);
    let data_len = u32::try_from(blocks.saturating_mul(256))
        .map_err(|_| AudioError::Capacity("ADPCM size"))?;
    let count = u32::try_from(clip.len()).map_err(|_| AudioError::Capacity("ADPCM frames"))?;
    if data_len > u32::MAX - 52 {
        return Err(AudioError::Capacity("ADPCM RIFF"));
    }
    let mut out = Vec::with_capacity(data_len as usize + 60);
    out.extend(b"RIFF");
    out.extend((data_len + 52).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(20_u32.to_le_bytes());
    out.extend(17_u16.to_le_bytes());
    out.extend(2_u16.to_le_bytes());
    out.extend(clip.sample_rate().to_le_bytes());
    out.extend((clip.sample_rate() * 256 / 249).to_le_bytes());
    out.extend(256_u16.to_le_bytes());
    out.extend(4_u16.to_le_bytes());
    out.extend(2_u16.to_le_bytes());
    out.extend(249_u16.to_le_bytes());
    out.extend(b"fact");
    out.extend(4_u32.to_le_bytes());
    out.extend(count.to_le_bytes());
    out.extend(b"data");
    out.extend(data_len.to_le_bytes());
    for block in 0..blocks {
        let first = clip.frames()[block * 249];
        let mut pred = [pcm(first.left) as i32, pcm(first.right) as i32];
        let mut index = [0, 0];
        for p in pred {
            out.extend((p as i16).to_le_bytes());
            out.extend([0, 0]);
        }
        for group in 0..31 {
            for channel in 0..2 {
                for pair in 0..4 {
                    let mut byte = 0;
                    for half in 0..2 {
                        let i = (block * 249 + 1 + group * 8 + pair * 2 + half).min(clip.len() - 1);
                        let f = clip.frames()[i];
                        let code = nibble(
                            pcm(if channel == 0 { f.left } else { f.right }),
                            &mut pred[channel],
                            &mut index[channel],
                        );
                        byte |= code << (half * 4);
                    }
                    out.push(byte);
                }
            }
        }
    }
    Ok(out)
}
