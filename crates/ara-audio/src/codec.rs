//! Incremental WAV/ADPCM, Vorbis and FLAC via Symphonia; Ogg Opus via libopus.

use crate::{AudioError, Clip, Frame, Result};
use std::io::{Read, Seek, SeekFrom};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{Decoder, DecoderOptions},
    errors::Error,
    formats::{FormatOptions, FormatReader},
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    probe::Hint,
};

/// Bounded pull interface shared by resident imports and streaming workers.
pub trait AudioDecoder: Send {
    /// Source frames per second.
    fn sample_rate(&self) -> u32;
    /// Reads up to out.len() stereo frames. Zero means EOF; errors never silently truncate.
    fn read_frames(&mut self, out: &mut [Frame]) -> Result<usize>;
}

/// Symphonia decoder retaining only one decoded packet at a time.
pub struct MediaDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    rate: u32,
    pending: Vec<Frame>,
    cursor: usize,
    eof: bool,
    expected_frames: Option<u64>,
    decoded_frames: u64,
}
impl MediaDecoder {
    /// Opens WAV (PCM/ADPCM), Ogg Vorbis or FLAC. Use OpusDecoder for Ogg Opus.
    pub fn open(mut source: Box<dyn MediaSource>, extension: &str) -> Result<Self> {
        let fact = if extension == "wav" && source.is_seekable() {
            wav_fact(source.as_mut())?
        } else {
            None
        };
        let mut hint = Hint::new();
        hint.with_extension(extension);
        let stream = MediaSourceStream::new(source, Default::default());
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(codec_error)?;
        let track = probed
            .format
            .default_track()
            .ok_or_else(|| AudioError::Codec("no audio track".into()))?;
        let rate = track
            .codec_params
            .sample_rate
            .ok_or_else(|| AudioError::Codec("missing sample rate".into()))?;
        let channels = track
            .codec_params
            .channels
            .ok_or_else(|| AudioError::Codec("missing channel layout".into()))?
            .count();
        if channels == 0 || channels > 2 || !(8000..=192000).contains(&rate) {
            return Err(AudioError::Codec(
                "only mono/stereo at 8–192 kHz are supported".into(),
            ));
        }
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(codec_error)?;
        let id = track.id;
        let expected_frames = fact.or(track.codec_params.n_frames);
        Ok(Self {
            format: probed.format,
            decoder,
            track: id,
            rate,
            pending: Vec::new(),
            cursor: 0,
            eof: false,
            expected_frames,
            decoded_frames: 0,
        })
    }
    fn packet(&mut self) -> Result<bool> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    if self
                        .expected_frames
                        .is_some_and(|n| self.decoded_frames < n)
                    {
                        return Err(AudioError::Codec("truncated media frames".into()));
                    }
                    self.eof = true;
                    return Ok(false);
                }
                Err(e) => return Err(codec_error(e)),
            };
            if packet.track_id() != self.track {
                continue;
            }
            let decoded = self.decoder.decode(&packet).map_err(codec_error)?;
            let spec = *decoded.spec();
            if spec.rate != self.rate || !(1..=2).contains(&spec.channels.count()) {
                return Err(AudioError::Codec("stream format changed".into()));
            }
            if decoded.capacity() > 1_048_576 {
                return Err(AudioError::Capacity("decoded packet scratch"));
            }
            let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            samples.copy_interleaved_ref(decoded);
            self.pending.clear();
            self.cursor = 0;
            let remaining = self.expected_frames.map_or(usize::MAX, |n| {
                n.saturating_sub(self.decoded_frames).min(usize::MAX as u64) as usize
            });
            for s in samples
                .samples()
                .chunks_exact(spec.channels.count())
                .take(remaining)
            {
                if s.iter().any(|v| !v.is_finite()) {
                    return Err(AudioError::Codec("non-finite decoded PCM".into()));
                }
                self.pending
                    .push(Frame::new(s[0], s[spec.channels.count() - 1]));
            }
            self.decoded_frames += self.pending.len() as u64;
            if self.expected_frames == Some(self.decoded_frames) {
                self.eof = true;
            }
            if !self.pending.is_empty() {
                return Ok(true);
            }
        }
    }
}
impl AudioDecoder for MediaDecoder {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn read_frames(&mut self, out: &mut [Frame]) -> Result<usize> {
        let mut n = 0;
        while n < out.len() {
            if self.cursor == self.pending.len() && (self.eof || !self.packet()?) {
                break;
            }
            let count = (out.len() - n).min(self.pending.len() - self.cursor);
            out[n..n + count].copy_from_slice(&self.pending[self.cursor..self.cursor + count]);
            self.cursor += count;
            n += count;
        }
        Ok(n)
    }
}
fn codec_error(e: impl std::fmt::Display) -> AudioError {
    AudioError::Codec(e.to_string())
}

// Symphonia 0.5.5 overwrites WAV fact's exact ADPCM length with padded block length.
// Preserve the declared frame count and reject truncated seekable WAVs before demuxing.
fn wav_fact(source: &mut dyn MediaSource) -> Result<Option<u64>> {
    let start = source.stream_position()?;
    let result = (|| {
        let mut header = [0_u8; 12];
        source.read_exact(&mut header)?;
        if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
            return Ok(None);
        }
        let riff_len = u32::from_le_bytes(
            header[4..8]
                .try_into()
                .map_err(|_| AudioError::Invalid("RIFF header"))?,
        ) as u64
            + 8;
        if source
            .byte_len()
            .is_some_and(|n| riff_len > n.saturating_sub(start))
        {
            return Err(AudioError::Codec("truncated RIFF/WAV".into()));
        }
        for _ in 0..4096 {
            let mut chunk = [0_u8; 8];
            source.read_exact(&mut chunk)?;
            let len = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as u64;
            if &chunk[..4] == b"data" {
                return Ok(None);
            }
            if &chunk[..4] == b"fact" && len >= 4 {
                let mut count = [0_u8; 4];
                source.read_exact(&mut count)?;
                return Ok(Some(u32::from_le_bytes(count) as u64));
            }
            source.seek(SeekFrom::Current((len + (len & 1)) as i64))?;
        }
        Err(AudioError::Codec("too many WAV metadata chunks".into()))
    })();
    source.seek(SeekFrom::Start(start))?;
    result
}

/// Ogg Opus decoder supporting mapping family zero, header gain, pre-skip and end trim.
pub struct OpusDecoder<R: Read + Seek + Send> {
    reader: ogg::PacketReader<R>,
    decoder: ara_opus_sys::Decoder,
    channels: usize,
    serial: u32,
    gain: f32,
    skip: usize,
    decoded: u64,
    buffer: [f32; 11520],
    cursor: usize,
    len: usize,
    eof: bool,
}
impl<R: Read + Seek + Send> OpusDecoder<R> {
    /// Opens a mono/stereo Ogg Opus asset; decoded output is always 48 kHz.
    pub fn open(source: R) -> Result<Self> {
        let mut reader = ogg::PacketReader::new(source);
        let header = reader.read_packet_expected().map_err(codec_error)?;
        let h = &header.data;
        if h.len() != 19
            || &h[..8] != b"OpusHead"
            || h[8] > 15
            || !(1..=2).contains(&h[9])
            || h[18] != 0
        {
            return Err(AudioError::Codec(
                "unsupported OpusHead (mapping family zero required)".into(),
            ));
        }
        let skip = u16::from_le_bytes([h[10], h[11]]) as usize;
        let gain = 10.0_f32.powf(i16::from_le_bytes([h[16], h[17]]) as f32 / 256.0 / 20.0);
        let channels = h[9] as usize;
        let serial = header.stream_serial();
        let tags = reader.read_packet_expected().map_err(codec_error)?;
        if tags.stream_serial() != serial || !tags.data.starts_with(b"OpusTags") {
            return Err(AudioError::Codec("missing OpusTags".into()));
        }
        let decoder = ara_opus_sys::Decoder::new(48000, channels as u8).map_err(codec_error)?;
        Ok(Self {
            reader,
            decoder,
            channels,
            serial,
            gain,
            skip,
            decoded: 0,
            buffer: [0.0; 11520],
            cursor: 0,
            len: 0,
            eof: false,
        })
    }
    fn packet(&mut self) -> Result<bool> {
        let Some(packet) = self.reader.read_packet().map_err(codec_error)? else {
            if !self.eof {
                return Err(AudioError::Codec(
                    "truncated Opus stream (missing EOS)".into(),
                ));
            }
            return Ok(false);
        };
        if self.eof || packet.stream_serial() != self.serial {
            return Err(AudioError::Codec(
                "chained/multiplexed Opus requires a separate asset".into(),
            ));
        }
        let frames = self
            .decoder
            .decode_float(&packet.data, &mut self.buffer)
            .map_err(codec_error)?;
        let start = self.decoded;
        self.decoded += frames as u64;
        self.cursor = self.skip.min(frames);
        self.skip -= self.cursor;
        self.len = frames;
        if packet.last_in_stream() {
            if packet.absgp_page() < start || packet.absgp_page() > self.decoded {
                return Err(AudioError::Codec("invalid Opus end granule".into()));
            }
            self.len = (packet.absgp_page() - start) as usize;
            self.eof = true;
        }
        Ok(true)
    }
}
impl<R: Read + Seek + Send> AudioDecoder for OpusDecoder<R> {
    fn sample_rate(&self) -> u32 {
        48000
    }
    fn read_frames(&mut self, out: &mut [Frame]) -> Result<usize> {
        let mut n = 0;
        while n < out.len() {
            if self.cursor >= self.len {
                if self.eof || !self.packet()? {
                    break;
                }
                if self.cursor >= self.len {
                    continue;
                }
            }
            let i = self.cursor * self.channels;
            out[n] = Frame::new(
                self.buffer[i] * self.gain,
                self.buffer[i + self.channels - 1] * self.gain,
            );
            n += 1;
            self.cursor += 1;
        }
        Ok(n)
    }
}

/// Decodes a short clip, refusing to grow past max_bytes. Long assets should stream.
pub fn decode_clip(decoder: &mut dyn AudioDecoder, max_bytes: usize) -> Result<Clip> {
    let mut frames = Vec::new();
    let mut scratch = [Frame::ZERO; 1024];
    loop {
        let n = decoder.read_frames(&mut scratch)?;
        if n == 0 {
            break;
        }
        if frames
            .len()
            .saturating_add(n)
            .saturating_mul(size_of::<Frame>())
            > max_bytes
        {
            return Err(AudioError::Capacity("resident decode budget"));
        }
        frames.extend_from_slice(&scratch[..n]);
    }
    Clip::new(decoder.sample_rate(), frames, max_bytes)
}
