//! Minimal owned Opus encoder/decoder handles over vendored libopus 1.5.2.
//! Construction allocates; packet processing uses caller-owned slices. No build tools beyond C.
#![warn(missing_docs)]
use std::{
    ffi::{c_int, c_void},
    fmt,
    ptr::NonNull,
};

unsafe extern "C" {
    fn opus_encoder_create(
        rate: c_int,
        channels: c_int,
        application: c_int,
        error: *mut c_int,
    ) -> *mut c_void;
    fn opus_encoder_destroy(state: *mut c_void);
    fn opus_encode_float(
        state: *mut c_void,
        pcm: *const f32,
        frames: c_int,
        output: *mut u8,
        capacity: c_int,
    ) -> c_int;
    fn opus_decoder_create(rate: c_int, channels: c_int, error: *mut c_int) -> *mut c_void;
    fn opus_decoder_destroy(state: *mut c_void);
    fn opus_decode_float(
        state: *mut c_void,
        packet: *const u8,
        bytes: c_int,
        pcm: *mut f32,
        capacity: c_int,
        fec: c_int,
    ) -> c_int;
    fn ara_opus_bitrate(state: *mut c_void, bitrate: c_int) -> c_int;
    fn ara_opus_lookahead(state: *mut c_void, frames: *mut c_int) -> c_int;
}

/// libopus error code; negative values follow the upstream Opus error constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error(
    /// Upstream negative Opus error code.
    pub i32,
);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "libopus error {}", self.0)
    }
}
impl std::error::Error for Error {}
fn checked(code: c_int) -> Result<usize, Error> {
    if code < 0 {
        Err(Error(code))
    } else {
        Ok(code as usize)
    }
}
fn length(len: usize) -> Result<c_int, Error> {
    c_int::try_from(len).map_err(|_| Error(-1))
}

/// Unique floating-point audio encoder, configured for the Opus audio application.
pub struct Encoder {
    state: NonNull<c_void>,
    channels: usize,
}
// SAFETY: Opus state is uniquely owned and has no thread affinity; mutation requires &mut self.
unsafe impl Send for Encoder {}
impl Encoder {
    /// Allocates an encoder for an Opus sample rate and one or two interleaved channels.
    pub fn new(rate: u32, channels: u8) -> Result<Self, Error> {
        if !(1..=2).contains(&channels) {
            return Err(Error(-1));
        }
        let mut error = 0;
        // SAFETY: writable error output; Opus validates rate/channels/application before allocating.
        let state = unsafe {
            opus_encoder_create(length(rate as usize)?, channels.into(), 2049, &mut error)
        };
        Ok(Self {
            state: NonNull::new(state).ok_or(Error(error))?,
            channels: channels.into(),
        })
    }
    /// Sets the total target bitrate in bits per second.
    pub fn set_bitrate(&mut self, bitrate: i32) -> Result<(), Error> {
        // SAFETY: exclusive live encoder; fixed-signature shim forwards the checked control request.
        checked(unsafe { ara_opus_bitrate(self.state.as_ptr(), bitrate) }).map(|_| ())
    }
    /// Returns the encoder delay in source frames for Ogg pre-skip.
    pub fn get_lookahead(&mut self) -> Result<usize, Error> {
        let mut frames = 0;
        // SAFETY: live encoder and writable output of the correct C integer type.
        checked(unsafe { ara_opus_lookahead(self.state.as_ptr(), &mut frames) })?;
        checked(frames)
    }
    /// Encodes one legal Opus frame into the supplied packet buffer.
    pub fn encode_float(&mut self, pcm: &[f32], output: &mut [u8]) -> Result<usize, Error> {
        if !pcm.len().is_multiple_of(self.channels) {
            return Err(Error(-1));
        }
        // SAFETY: both slices cover the exact lengths passed to Opus; exclusive state and output.
        checked(unsafe {
            opus_encode_float(
                self.state.as_ptr(),
                pcm.as_ptr(),
                length(pcm.len() / self.channels)?,
                output.as_mut_ptr(),
                length(output.len())?,
            )
        })
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: this handle owns the encoder and destroys it exactly once.
        unsafe { opus_encoder_destroy(self.state.as_ptr()) };
    }
}
/// Unique floating-point packet decoder; packet-loss concealment is deliberately not exposed.
pub struct Decoder {
    state: NonNull<c_void>,
    channels: usize,
}
// SAFETY: unique mutable Opus state can move between threads and has no thread affinity.
unsafe impl Send for Decoder {}
impl Decoder {
    /// Allocates a mono/stereo decoder at an Opus sample rate.
    pub fn new(rate: u32, channels: u8) -> Result<Self, Error> {
        if !(1..=2).contains(&channels) {
            return Err(Error(-1));
        }
        let mut error = 0;
        // SAFETY: writable error output; Opus validates its integer inputs.
        let state =
            unsafe { opus_decoder_create(length(rate as usize)?, channels.into(), &mut error) };
        Ok(Self {
            state: NonNull::new(state).ok_or(Error(error))?,
            channels: channels.into(),
        })
    }
    /// Decodes a packet into interleaved PCM, returning the number of frames per channel.
    pub fn decode_float(&mut self, packet: &[u8], output: &mut [f32]) -> Result<usize, Error> {
        if packet.is_empty() || !output.len().is_multiple_of(self.channels) {
            return Err(Error(-1));
        }
        // SAFETY: slices cover the supplied byte/frame capacities; decoder and output are exclusive.
        checked(unsafe {
            opus_decode_float(
                self.state.as_ptr(),
                packet.as_ptr(),
                length(packet.len())?,
                output.as_mut_ptr(),
                length(output.len() / self.channels)?,
                0,
            )
        })
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: this handle owns the decoder and destroys it exactly once.
        unsafe { opus_decoder_destroy(self.state.as_ptr()) };
    }
}
