//! One bounded chunk decoder; both backends feed the same integrity checks.
use crate::{Error, invalid};
use std::io::Read;

#[cfg(all(test, feature = "write"))]
mod tests;

pub(crate) fn chunk(compressed: &[u8], size: u64) -> Result<Vec<u8>, Error> {
    #[cfg(feature = "native-decode")]
    let decoder = {
        let mut decoder = zstd::stream::read::Decoder::with_buffer(compressed)?;
        // Pack chunks are at most 16 MiB. A hostile frame must not request a
        // larger history window before the output byte limit can take effect.
        decoder.window_log_max(24)?;
        decoder.single_frame()
    };
    #[cfg(not(feature = "native-decode"))]
    let decoder = ruzstd::decoding::StreamingDecoder::new(compressed)
        .map_err(|error| invalid(error.to_string()))?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(size as usize)
        .map_err(|error| invalid(format!("chunk allocation: {error}")))?;
    decoder.take(size + 1).read_to_end(&mut raw)?;
    Ok(raw)
}
