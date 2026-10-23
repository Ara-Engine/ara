use crate::container::{floats, u32_at};
use crate::{Container, FormatError, Kind};
use bytemuck::{Pod, Zeroable};

/// A 112-byte joint binding; joint order matches vertex joint indices.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct Joint {
    /// Parent joint, or -1 for a root. Parents precede children only if the source does.
    pub parent: i32,
    /// Source scene node index, also used by animation channels.
    pub node: u32,
    /// Column-major inverse bind matrix.
    pub inverse_bind: [f32; 16],
    /// Rest translation, rotation xyzw, scale xyz.
    pub rest: [f32; 10],
}
/// A 32-byte animation channel pointing into TIME and VALU.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct Channel {
    /// Scene node index.
    pub node: u32,
    /// 0=translation, 1=rotation, 2=scale, 3=morph weights.
    pub path: u32,
    /// 0=step, 1=linear, 2=cubic spline (in tangent, value, out tangent).
    pub interpolation: u32,
    /// First scalar time in TIME.
    pub time_offset: u32,
    /// Number of distinct keyframe times.
    pub key_count: u32,
    /// First scalar in VALU.
    pub value_offset: u32,
    /// Scalar components per value.
    pub components: u32,
    /// Reserved, zero.
    pub reserved: u32,
}
/// Borrowed joint bindings and UTF-8 joint names.
#[derive(Debug, Clone, Copy)]
pub struct Skeleton<'a> {
    /// Joint records, no decoding allocation required.
    pub joints: &'a [u8],
    /// Concatenated UTF-8 joint names in joint order.
    names: &'a [u8],
    /// Little-endian u32 byte offsets, including a final sentinel.
    name_offsets: &'a [u8],
}
impl<'a> Skeleton<'a> {
    /// Checks joint parents, cycles and finite matrices/transforms.
    pub fn read(bytes: &'a [u8]) -> Result<Self, FormatError> {
        let c = Container::read(bytes)?;
        if c.kind() != Kind::Skeleton {
            return Err(FormatError("expected skeleton"));
        }
        let s = Self {
            joints: c.records(b"JNTS", 112)?,
            names: c.records(b"NAME", 1)?,
            name_offsets: c.records(b"NOFF", 4)?,
        };
        let n = s.joints.len() / 112;
        let names = std::str::from_utf8(s.names).map_err(|_| FormatError("invalid name UTF-8"))?;
        if s.name_offsets.len() / 4 != n + 1
            || u32_at(s.name_offsets, 0)? != 0
            || u32_at(s.name_offsets, n * 4)? as usize != s.names.len()
        {
            return Err(FormatError("invalid joint name offsets"));
        }
        let mut previous = 0;
        for offset in s.name_offsets.as_chunks::<4>().0 {
            let offset = u32::from_le_bytes(*offset) as usize;
            if offset < previous || !names.is_char_boundary(offset) {
                return Err(FormatError("invalid joint name boundary"));
            }
            previous = offset;
        }
        for (i, j) in s.joints.as_chunks::<112>().0.iter().enumerate() {
            let parent = u32_at(j, 0)? as i32;
            if parent < -1 || parent >= n as i32 || floats(&j[8..]).any(|v| !v.is_finite()) {
                return Err(FormatError("invalid joint"));
            }
            let mut p = parent;
            let mut depth = 0;
            while p >= 0 {
                if p as usize == i || depth >= n {
                    return Err(FormatError("joint cycle"));
                }
                p = u32_at(s.joints, p as usize * 112)? as i32;
                if p < -1 || p >= n as i32 {
                    return Err(FormatError("invalid parent"));
                }
                depth += 1;
            }
        }
        Ok(s)
    }
    /// Iterates names directly from the original file, without allocating strings.
    pub fn joint_names(&self) -> impl Iterator<Item = &'a str> {
        self.name_offsets.as_chunks::<4>().0.windows(2).map(|pair| {
            let start = u32::from_le_bytes(pair[0]) as usize;
            let end = u32::from_le_bytes(pair[1]) as usize;
            std::str::from_utf8(&self.names[start..end])
                .expect("Skeleton::read validated every UTF-8 name boundary")
        })
    }
}
/// Writes native joint records and an allocation-free-loadable UTF-8 name table.
pub fn write_skeleton(joints: &[Joint], names: &[&str]) -> Result<Vec<u8>, FormatError> {
    if !cfg!(target_endian = "little") || joints.len() != names.len() {
        return Err(FormatError(
            "skeleton writer expects matching names on a little-endian host",
        ));
    }
    let mut offsets = vec![0u32];
    let mut text = Vec::new();
    for name in names {
        text.extend_from_slice(name.as_bytes());
        offsets
            .push(u32::try_from(text.len()).map_err(|_| FormatError("joint names exceed 4 GiB"))?);
    }
    let mut b = crate::Builder::new(Kind::Skeleton);
    b.section(*b"JNTS", 112, bytemuck::cast_slice(joints).to_vec())
        .section(*b"NOFF", 4, bytemuck::cast_slice(&offsets).to_vec())
        .section(*b"NAME", 1, text);
    let bytes = b.finish()?;
    Skeleton::read(&bytes)?;
    Ok(bytes)
}
/// Borrowed keyframes. Cubic tangents retain glTF's units per second.
#[derive(Debug, Clone, Copy)]
pub struct Clip<'a> {
    /// Channel records.
    pub channels: &'a [u8],
    /// Little-endian f32 keyframe seconds.
    pub times: &'a [u8],
    /// Little-endian f32 scalar values/tangents.
    pub values: &'a [u8],
}
impl<'a> Clip<'a> {
    /// Validates every channel span, component count and increasing finite times.
    pub fn read(bytes: &'a [u8]) -> Result<Self, FormatError> {
        let c = Container::read(bytes)?;
        if c.kind() != Kind::Clip {
            return Err(FormatError("expected clip"));
        }
        let clip = Self {
            channels: c.records(b"CHAN", 32)?,
            times: c.records(b"TIME", 4)?,
            values: c.records(b"VALU", 4)?,
        };
        if floats(clip.values).any(|v| !v.is_finite()) {
            return Err(FormatError("nonfinite animation value"));
        }
        for ch in clip.channels.as_chunks::<32>().0 {
            let path = u32_at(ch, 4)?;
            let mode = u32_at(ch, 8)?;
            let start = u32_at(ch, 12)? as u64;
            let count = u32_at(ch, 16)? as u64;
            let vo = u32_at(ch, 20)? as u64;
            let width = u32_at(ch, 24)? as u64;
            let end = count
                .checked_mul(width)
                .and_then(|v| v.checked_mul(if mode == 2 { 3 } else { 1 }))
                .and_then(|v| v.checked_add(vo))
                .ok_or(FormatError("animation span overflow"))?;
            if path > 3
                || mode > 2
                || count == 0
                || width == 0
                || (path < 3 && width != if path == 1 { 4 } else { 3 })
                || start + count > clip.times.len() as u64 / 4
                || end > clip.values.len() as u64 / 4
                || u32_at(ch, 28)? != 0
            {
                return Err(FormatError("invalid channel"));
            }
            let mut prev = -1.0;
            for t in floats(&clip.times[start as usize * 4..(start + count) as usize * 4]) {
                if !t.is_finite() || t < 0.0 || t <= prev {
                    return Err(FormatError("invalid channel times"));
                }
                prev = t;
            }
        }
        Ok(clip)
    }
}
