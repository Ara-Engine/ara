//! One disposable worker connection, reusing pooled devices across processes.
use super::wire::{self, Descriptor, Reply};
use ara_asset::{BlockEncoder, Cancellation, CookPreset, EncoderEndpoint, Error, Result};
use ara_asset_format::TextureFormat;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpStream,
    sync::Mutex,
    time::Instant,
};

struct Client {
    stream: TcpStream,
    timings: BTreeMap<String, f64>,
}
pub(super) struct Remote {
    descriptor: Descriptor,
    client: Mutex<Client>,
}
impl Remote {
    pub(super) fn connect(descriptor: Descriptor) -> Result<Self> {
        let mut stream = descriptor.endpoint.connect()?;
        let identity: String = wire::read_json(&mut stream)?;
        if identity != descriptor.identity {
            return Err(Error::EncoderUnavailable(
                "codec service revision changed".into(),
            ));
        }
        Ok(Self {
            descriptor,
            client: Mutex::new(Client {
                stream,
                timings: BTreeMap::new(),
            }),
        })
    }
    fn request(
        &self,
        format: Option<TextureFormat>,
        preset: CookPreset,
        words: &[u32],
    ) -> Result<Vec<u8>> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| Error::EncoderUnavailable("codec client poisoned".into()))?;
        let start = Instant::now();
        client.stream.write_all(&[u8::from(format.is_some())])?;
        let expected = if let Some(format) = format {
            let expected = wire::validate_count(format, words.len())?;
            client.stream.write_all(&format.vk_format().to_le_bytes())?;
            client.stream.write_all(&[wire::preset_id(preset)])?;
            client
                .stream
                .write_all(&(words.len() as u32).to_le_bytes())?;
            wire::write_words(&mut client.stream, words)?;
            expected
        } else {
            0
        };
        client.stream.flush()?;
        let reply: Reply = wire::read_json(&mut client.stream)?;
        if let Some(error) = reply.error {
            return Err(Error::EncoderUnavailable(error));
        }
        if reply.bytes != expected {
            return Err(Error::EncoderUnavailable(
                "codec service returned the wrong block count".into(),
            ));
        }
        let mut output = vec![0; expected];
        client.stream.read_exact(&mut output)?;
        for (key, value) in reply.metrics {
            if key.starts_with("service.") {
                client.timings.insert(key, value);
            } else {
                *client.timings.entry(key).or_default() += value;
            }
        }
        *client
            .timings
            .entry("service.roundtrip.wall_ms".into())
            .or_default() += start.elapsed().as_secs_f64() * 1000.0;
        Ok(output)
    }
}
impl BlockEncoder for Remote {
    fn backend(&self) -> &str {
        &self.descriptor.identity
    }
    fn service_endpoint(&self) -> Option<EncoderEndpoint> {
        Some(self.descriptor.endpoint.clone())
    }
    fn prepare(&self) -> Result<()> {
        self.request(None, CookPreset::Balanced, &[]).map(|_| ())
    }
    fn metrics(&self) -> BTreeMap<String, f64> {
        self.client
            .lock()
            .map(|c| c.timings.clone())
            .unwrap_or_default()
    }
    fn encode(
        &self,
        format: TextureFormat,
        preset: CookPreset,
        blocks: &[u32],
        cancellation: &Cancellation,
    ) -> Result<Vec<u8>> {
        let format = wire::format(format.vk_format())?;
        let words = wire::words(format);
        if !blocks.len().is_multiple_of(words) {
            return Err(Error::Invalid("truncated block-major texture input".into()));
        }
        let mut output = Vec::with_capacity(blocks.len() / words * format.block_bytes());
        for chunk in blocks.chunks(wire::MAX_BLOCKS * words) {
            cancellation.check()?;
            output.extend(
                self.request(Some(format), preset, chunk)
                    .map_err(|e| Error::EncoderUnavailable(e.to_string()))?,
            );
        }
        cancellation.check()?;
        Ok(output)
    }
}
