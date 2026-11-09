//! Explicit portable encoding; never serialize Rust layout or `usize`.
use super::*;
use anyhow::{Context, Result, ensure};
use std::io::{Cursor, Read, Write};

const MAGIC: &[u8; 8] = b"ARACHG01";
trait Wire: Sized {
    fn put(&self, out: &mut Vec<u8>);
    fn get(input: &mut Cursor<&[u8]>) -> Result<Self>;
}
macro_rules! integer {
    ($t:ty, $n:expr) => {
        impl Wire for $t {
            fn put(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
            fn get(input: &mut Cursor<&[u8]>) -> Result<Self> {
                let mut bytes = [0; $n];
                input.read_exact(&mut bytes)?;
                Ok(Self::from_le_bytes(bytes))
            }
        }
    };
}
integer!(u8, 1);
integer!(u32, 4);
integer!(u64, 8);
integer!(f32, 4);
impl<T: Wire + Copy + Default, const N: usize> Wire for [T; N] {
    fn put(&self, out: &mut Vec<u8>) {
        for v in self {
            v.put(out);
        }
    }
    fn get(input: &mut Cursor<&[u8]>) -> Result<Self> {
        let mut out = [T::default(); N];
        for v in &mut out {
            *v = T::get(input)?;
        }
        Ok(out)
    }
}
impl Wire for Vec3 {
    fn put(&self, out: &mut Vec<u8>) {
        self.to_array().put(out);
    }
    fn get(input: &mut Cursor<&[u8]>) -> Result<Self> {
        Ok(Self::from_array(<[f32; 3]>::get(input)?))
    }
}
impl Wire for Aabb {
    fn put(&self, out: &mut Vec<u8>) {
        self.min.put(out);
        self.max.put(out);
    }
    fn get(input: &mut Cursor<&[u8]>) -> Result<Self> {
        Ok(Self::new(Vec3::get(input)?, Vec3::get(input)?))
    }
}
fn boolean(input: &mut Cursor<&[u8]>) -> Result<bool> {
    let v = u8::get(input)?;
    ensure!(v <= 1, "invalid replay boolean");
    Ok(v != 0)
}
impl Wire for InstanceState {
    fn put(&self, out: &mut Vec<u8>) {
        self.entity.put(out);
        self.geometry.put(out);
        self.material.put(out);
        self.world.to_cols_array().put(out);
        self.bounds.put(out);
        (self.body as u8).put(out);
        u8::from(self.casts_shadows).put(out);
        self.wind.put(out);
    }
    fn get(input: &mut Cursor<&[u8]>) -> Result<Self> {
        let entity = u64::get(input)?;
        let geometry = u64::get(input)?;
        let material = u64::get(input)?;
        let world = Mat4::from_cols_array(&<[f32; 16]>::get(input)?);
        let bounds = Aabb::get(input)?;
        let body = match u8::get(input)? {
            0 => BodyClass::Static,
            1 => BodyClass::Kinematic,
            2 => BodyClass::PhysicsActive,
            3 => BodyClass::Sleeping,
            _ => anyhow::bail!("invalid replay body class"),
        };
        Ok(Self {
            entity,
            geometry,
            material,
            world,
            bounds,
            body,
            casts_shadows: boolean(input)?,
            wind: f32::get(input)?,
        })
    }
}
impl Wire for GeometryState {
    fn put(&self, out: &mut Vec<u8>) {
        self.key.put(out);
        self.revision.put(out);
        self.bounds.put(out);
        self.displacement.put(out);
    }
    fn get(input: &mut Cursor<&[u8]>) -> Result<Self> {
        Ok(Self {
            key: <[u8; 32]>::get(input)?,
            revision: u64::get(input)?,
            bounds: Aabb::get(input)?,
            displacement: f32::get(input)?,
        })
    }
}
impl Wire for MaterialState {
    fn put(&self, out: &mut Vec<u8>) {
        self.0.put(out);
    }
    fn get(input: &mut Cursor<&[u8]>) -> Result<Self> {
        Ok(Self(<[u32; 80]>::get(input)?))
    }
}
impl Wire for LightState {
    fn put(&self, out: &mut Vec<u8>) {
        self.0.put(out);
    }
    fn get(input: &mut Cursor<&[u8]>) -> Result<Self> {
        Ok(Self(<[u32; 28]>::get(input)?))
    }
}
impl Wire for PortalState {
    fn put(&self, out: &mut Vec<u8>) {
        self.openness.put(out);
    }
    fn get(input: &mut Cursor<&[u8]>) -> Result<Self> {
        Ok(Self {
            openness: f32::get(input)?,
        })
    }
}
impl Wire for SunState {
    fn put(&self, out: &mut Vec<u8>) {
        self.direction.put(out);
        self.lux.put(out);
    }
    fn get(input: &mut Cursor<&[u8]>) -> Result<Self> {
        Ok(Self {
            direction: Vec3::get(input)?,
            lux: Vec3::get(input)?,
        })
    }
}
impl<T: Wire> Wire for Change<T> {
    fn put(&self, out: &mut Vec<u8>) {
        self.id.put(out);
        self.previous_generation.put(out);
        self.generation.put(out);
        self.flags.put(out);
        (u8::from(self.previous.is_some()) | (u8::from(self.current.is_some()) << 1)).put(out);
        if let Some(v) = &self.previous {
            v.put(out);
        }
        if let Some(v) = &self.current {
            v.put(out);
        }
    }
    fn get(input: &mut Cursor<&[u8]>) -> Result<Self> {
        let id = u64::get(input)?;
        let previous_generation = u64::get(input)?;
        let generation = u64::get(input)?;
        let flags = u32::get(input)?;
        let presence = u8::get(input)?;
        ensure!(presence <= 3, "invalid replay presence");
        Ok(Self {
            id,
            previous_generation,
            generation,
            flags,
            previous: if presence & 1 != 0 {
                Some(T::get(input)?)
            } else {
                None
            },
            current: if presence & 2 != 0 {
                Some(T::get(input)?)
            } else {
                None
            },
        })
    }
}
impl Wire for InstanceChange {
    fn put(&self, out: &mut Vec<u8>) {
        self.change.put(out);
        self.swept.put(out);
        self.displacement.put(out);
    }
    fn get(input: &mut Cursor<&[u8]>) -> Result<Self> {
        Ok(Self {
            change: Change::get(input)?,
            swept: Aabb::get(input)?,
            displacement: f32::get(input)?,
        })
    }
}
impl Wire for SunChange {
    fn put(&self, out: &mut Vec<u8>) {
        self.change.put(out);
        self.angular_delta.put(out);
        self.elevation.put(out);
    }
    fn get(input: &mut Cursor<&[u8]>) -> Result<Self> {
        Ok(Self {
            change: Change::get(input)?,
            angular_delta: f32::get(input)?,
            elevation: f32::get(input)?,
        })
    }
}

/// Versioned streaming feed recorder; its byte buffer is reused per frame.
pub struct ReplayWriter<W> {
    output: W,
    bytes: Vec<u8>,
    previous: Option<u64>,
}
impl<W: Write> ReplayWriter<W> {
    /// Write the v1 header immediately.
    pub fn new(mut output: W) -> Result<Self> {
        output.write_all(MAGIC)?;
        output.write_all(&1u32.to_le_bytes())?;
        output.write_all(&0u32.to_le_bytes())?;
        Ok(Self {
            output,
            bytes: Vec::new(),
            previous: None,
        })
    }
    /// Append a complete sorted frame; the first record must be a full reset.
    pub fn write_frame(&mut self, frame: &TransportChanges) -> Result<()> {
        validate_frame(frame)?;
        ensure!(
            self.previous.is_none_or(|n| frame.frame > n),
            "recorded frames must increase"
        );
        ensure!(
            self.previous.is_some() || frame.reset,
            "recording must start with a reset snapshot"
        );
        self.bytes.clear();
        u8::from(frame.reset).put(&mut self.bytes);
        for count in [
            frame.instances.len(),
            frame.geometries.len(),
            frame.materials.len(),
            frame.lights.len(),
            frame.portals.len(),
        ] {
            u32::try_from(count)
                .context("too many replay events")?
                .put(&mut self.bytes);
        }
        u8::from(frame.sun.is_some()).put(&mut self.bytes);
        for c in &frame.instances {
            c.put(&mut self.bytes);
        }
        for c in &frame.geometries {
            c.put(&mut self.bytes);
        }
        for c in &frame.materials {
            c.put(&mut self.bytes);
        }
        for c in &frame.lights {
            c.put(&mut self.bytes);
        }
        for c in &frame.portals {
            c.put(&mut self.bytes);
        }
        if let Some(c) = &frame.sun {
            c.put(&mut self.bytes);
        }
        self.output.write_all(&frame.frame.to_le_bytes())?;
        self.output
            .write_all(&(self.bytes.len() as u64).to_le_bytes())?;
        self.output.write_all(&self.bytes)?;
        self.previous = Some(frame.frame);
        Ok(())
    }
    /// Flush the underlying stream and return it.
    pub fn finish(mut self) -> Result<W> {
        self.output.flush()?;
        Ok(self.output)
    }
}

/// Bounded streaming reader with reusable byte and staging buffers.
pub struct ReplayReader<R> {
    input: R,
    bytes: Vec<u8>,
    staging: TransportChanges,
    limit: usize,
    previous: Option<u64>,
}
impl<R: Read> ReplayReader<R> {
    /// Verify the header. `max_frame_bytes` bounds encoded input per frame, before allocation.
    pub fn new(mut input: R, max_frame_bytes: usize) -> Result<Self> {
        let mut header = [0u8; 16];
        input.read_exact(&mut header)?;
        ensure!(
            &header[..8] == MAGIC && header[8..12] == 1u32.to_le_bytes() && header[12..] == [0; 4],
            "unsupported change-feed replay header"
        );
        Ok(Self {
            input,
            bytes: Vec::new(),
            staging: TransportChanges::default(),
            limit: max_frame_bytes,
            previous: None,
        })
    }
    /// Read one frame into reused storage. False means clean EOF. Errors leave `out` unchanged.
    pub fn read_frame(&mut self, out: &mut TransportChanges) -> Result<bool> {
        let mut header = [0u8; 16];
        loop {
            match self.input.read(&mut header[..1]) {
                Ok(0) => return Ok(false),
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        self.input.read_exact(&mut header[1..])?;
        let mut cursor = Cursor::new(header.as_slice());
        let frame = u64::get(&mut cursor)?;
        let length =
            usize::try_from(u64::get(&mut cursor)?).context("frame exceeds address space")?;
        ensure!(
            (22..=self.limit).contains(&length),
            "replay frame exceeds byte limit or is too short"
        );
        ensure!(
            self.previous.is_none_or(|n| frame > n),
            "replayed frames must increase"
        );
        self.bytes.clear();
        self.bytes.try_reserve(length)?;
        self.bytes.resize(length, 0);
        self.input.read_exact(&mut self.bytes)?;
        let mut input = Cursor::new(self.bytes.as_slice());
        self.staging.clear(frame);
        self.staging.reset = boolean(&mut input)?;
        ensure!(
            self.previous.is_some() || self.staging.reset,
            "replay must start with reset snapshot"
        );
        let counts = <[u32; 5]>::get(&mut input)?;
        let sun = boolean(&mut input)?;
        let minimum: u64 = counts
            .iter()
            .zip([57u64, 29, 29, 29, 29])
            .map(|(&c, bytes)| u64::from(c) * bytes)
            .sum();
        ensure!(
            minimum + if sun { 37 } else { 0 } <= (length - 22) as u64,
            "replay counts exceed frame payload"
        );
        read_rows(&mut input, &mut self.staging.instances, counts[0])?;
        read_rows(&mut input, &mut self.staging.geometries, counts[1])?;
        read_rows(&mut input, &mut self.staging.materials, counts[2])?;
        read_rows(&mut input, &mut self.staging.lights, counts[3])?;
        read_rows(&mut input, &mut self.staging.portals, counts[4])?;
        if sun {
            self.staging.sun = Some(SunChange::get(&mut input)?);
        }
        ensure!(
            input.position() == length as u64,
            "trailing bytes in replay frame"
        );
        validate_frame(&self.staging)?;
        std::mem::swap(out, &mut self.staging);
        self.previous = Some(frame);
        Ok(true)
    }
}
fn read_rows<T: Wire>(input: &mut Cursor<&[u8]>, out: &mut Vec<T>, count: u32) -> Result<()> {
    // Grow only as valid records arrive, not from an untrusted advertised count.
    for _ in 0..count {
        out.push(T::get(input)?);
    }
    Ok(())
}
fn valid_change<T>(c: &Change<T>) -> Result<()> {
    ensure!(
        c.generation > c.previous_generation && c.flags != 0 && c.flags & !flags::ALL == 0,
        "invalid replay generation/flags"
    );
    ensure!(
        c.previous.is_none() || c.previous_generation > 0,
        "live predecessor has zero generation"
    );
    ensure!(
        c.previous.is_some() || c.flags & flags::ADDED != 0,
        "missing addition flag"
    );
    ensure!(
        c.current.is_some() || c.flags & flags::REMOVED != 0,
        "missing removal flag"
    );
    Ok(())
}
fn ordered(ids: impl Iterator<Item = u64>) -> Result<()> {
    let mut last = None;
    for id in ids {
        ensure!(
            last.is_none_or(|n| n < id),
            "replay rows must be strictly ordered"
        );
        last = Some(id);
    }
    Ok(())
}
pub(super) fn validate_frame(frame: &TransportChanges) -> Result<()> {
    ordered(frame.instances.iter().map(|c| c.change.id))?;
    ordered(frame.geometries.iter().map(|c| c.id))?;
    ordered(frame.materials.iter().map(|c| c.id))?;
    ordered(frame.lights.iter().map(|c| c.id))?;
    ordered(frame.portals.iter().map(|c| c.id))?;
    for c in &frame.instances {
        valid_change(&c.change)?;
        for v in [c.change.previous, c.change.current].into_iter().flatten() {
            feed::validate_instance(v)?;
        }
        ensure!(
            c.swept.valid() && c.displacement.is_finite() && c.displacement >= 0.,
            "invalid replay motion bounds"
        );
    }
    for c in &frame.geometries {
        valid_change(c)?;
        for v in [c.previous, c.current].into_iter().flatten() {
            feed::validate_geometry(v)?;
        }
    }
    for c in &frame.materials {
        valid_change(c)?;
    }
    for c in &frame.lights {
        valid_change(c)?;
        for v in [c.previous, c.current].into_iter().flatten() {
            ensure!(
                v.0[..24].iter().all(|v| f32::from_bits(*v).is_finite()),
                "non-finite replay light"
            );
        }
    }
    for c in &frame.portals {
        valid_change(c)?;
        for v in [c.previous, c.current].into_iter().flatten() {
            ensure!(
                v.openness.is_finite() && (0. ..=1.).contains(&v.openness),
                "invalid replay portal"
            );
        }
    }
    if let Some(c) = &frame.sun {
        valid_change(&c.change)?;
        ensure!(c.change.id == 0, "sun replay id must be zero");
        for v in [c.change.previous, c.change.current].into_iter().flatten() {
            feed::validate_sun(v)?;
        }
        ensure!(
            c.angular_delta.is_finite()
                && c.angular_delta >= 0.
                && c.elevation.is_finite()
                && c.elevation.abs() <= std::f32::consts::FRAC_PI_2,
            "invalid replay sun angle"
        );
    }
    if frame.reset {
        ensure!(
            frame.instances.iter().all(|c| c.change.previous.is_none())
                && frame.geometries.iter().all(|c| c.previous.is_none())
                && frame.materials.iter().all(|c| c.previous.is_none())
                && frame.lights.iter().all(|c| c.previous.is_none())
                && frame.portals.iter().all(|c| c.previous.is_none())
                && frame.sun.is_none_or(|c| c.change.previous.is_none()),
            "reset has live predecessor"
        );
    }
    Ok(())
}
