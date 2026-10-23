use std::fmt;

/// A malformed or unsupported cooked asset; readers never trust offsets or counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError(pub &'static str);
impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for FormatError {}

/// The payload schema selected by a cooked file's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Kind {
    /// Quantized geometry, LODs and meshlets.
    Mesh = 1,
    /// Joint bindings and inverse bind matrices.
    Skeleton = 2,
    /// Node animation channels and keyframes.
    Clip = 3,
}

/// One borrowed section, directly usable as a GPU upload source.
#[derive(Debug, Clone, Copy)]
pub struct Section<'a> {
    /// Four-byte section name.
    pub tag: [u8; 4],
    /// Bytes per record, or one for opaque data.
    pub stride: u32,
    /// Validated payload range in the original file.
    pub bytes: &'a [u8],
}

/// An allocation-free view of an ARAF v1 file, including its checked section table.
#[derive(Debug, Clone, Copy)]
pub struct Container<'a> {
    bytes: &'a [u8],
    count: usize,
    kind: Kind,
}

pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, FormatError> {
    let b = bytes
        .get(offset..offset + 4)
        .ok_or(FormatError("truncated scalar"))?;
    Ok(u32::from_le_bytes(
        b.try_into().map_err(|_| FormatError("invalid scalar"))?,
    ))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, FormatError> {
    let b = bytes
        .get(offset..offset + 8)
        .ok_or(FormatError("truncated scalar"))?;
    Ok(u64::from_le_bytes(
        b.try_into().map_err(|_| FormatError("invalid scalar"))?,
    ))
}
pub(crate) fn floats(bytes: &[u8]) -> impl Iterator<Item = f32> + '_ {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

impl<'a> Container<'a> {
    /// Checks version, length, unique tags, strides, alignment and disjoint ranges.
    pub fn read(bytes: &'a [u8]) -> Result<Self, FormatError> {
        if bytes.get(..4) != Some(b"ARAF") || u32_at(bytes, 4)? != 1 {
            return Err(FormatError("unsupported ARAF version"));
        }
        let kind = match u32_at(bytes, 8)? {
            1 => Kind::Mesh,
            2 => Kind::Skeleton,
            3 => Kind::Clip,
            _ => return Err(FormatError("unknown asset kind")),
        };
        let count = u32_at(bytes, 12)? as usize;
        if u64_at(bytes, 16)? != bytes.len() as u64 || u64_at(bytes, 24)? != 0 {
            return Err(FormatError("invalid file size or reserved header"));
        }
        let table_end = 32usize
            .checked_add(
                count
                    .checked_mul(24)
                    .ok_or(FormatError("section count overflow"))?,
            )
            .ok_or(FormatError("table overflow"))?;
        if table_end > bytes.len() {
            return Err(FormatError("truncated section table"));
        }
        let this = Self { bytes, count, kind };
        let mut previous_end = table_end;
        for i in 0..count {
            let pos = 32 + i * 24;
            let start = usize::try_from(u64_at(bytes, pos + 8)?)
                .map_err(|_| FormatError("section overflow"))?;
            let len = usize::try_from(u64_at(bytes, pos + 16)?)
                .map_err(|_| FormatError("section overflow"))?;
            let end = start
                .checked_add(len)
                .ok_or(FormatError("section overflow"))?;
            let stride = u32_at(bytes, pos + 4)? as usize;
            if start < previous_end
                || start % 16 != 0
                || end > bytes.len()
                || stride == 0
                || len % stride != 0
            {
                return Err(FormatError("invalid section range or stride"));
            }
            for j in 0..i {
                if bytes[32 + j * 24..36 + j * 24] == bytes[pos..pos + 4] {
                    return Err(FormatError("duplicate section"));
                }
            }
            previous_end = end;
        }
        Ok(this)
    }
    /// The validated cooked schema.
    pub fn kind(&self) -> Kind {
        self.kind
    }
    /// Finds a section without allocating or converting its payload.
    pub fn section(&self, tag: &[u8; 4]) -> Result<Section<'a>, FormatError> {
        for i in 0..self.count {
            let p = 32 + i * 24;
            if &self.bytes[p..p + 4] == tag {
                let start = u64_at(self.bytes, p + 8)? as usize;
                let len = u64_at(self.bytes, p + 16)? as usize;
                return Ok(Section {
                    tag: *tag,
                    stride: u32_at(self.bytes, p + 4)?,
                    bytes: &self.bytes[start..start + len],
                });
            }
        }
        Err(FormatError("missing required section"))
    }
    /// Checks a required section's record size.
    pub fn records(&self, tag: &[u8; 4], stride: u32) -> Result<&'a [u8], FormatError> {
        let section = self.section(tag)?;
        if section.stride != stride {
            return Err(FormatError("invalid record size"));
        }
        Ok(section.bytes)
    }
}

/// Builds deterministic section tables; input order is the format's declared order.
pub struct Builder {
    kind: Kind,
    sections: Vec<([u8; 4], u32, Vec<u8>)>,
}
impl Builder {
    /// Starts an ARAF v1 file.
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            sections: Vec::new(),
        }
    }
    /// Adds a section; the final reader validates tag uniqueness and strides.
    pub fn section(&mut self, tag: [u8; 4], stride: u32, bytes: Vec<u8>) -> &mut Self {
        self.sections.push((tag, stride, bytes));
        self
    }
    /// Serializes a file with zero padding and checks the resulting container.
    pub fn finish(self) -> Result<Vec<u8>, FormatError> {
        let count =
            u32::try_from(self.sections.len()).map_err(|_| FormatError("too many sections"))?;
        let mut bytes = vec![0; 32 + self.sections.len() * 24];
        bytes[..4].copy_from_slice(b"ARAF");
        bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&(self.kind as u32).to_le_bytes());
        bytes[12..16].copy_from_slice(&count.to_le_bytes());
        for (i, (tag, stride, data)) in self.sections.into_iter().enumerate() {
            bytes.resize(bytes.len().next_multiple_of(16), 0);
            let offset = bytes.len() as u64;
            let len = data.len() as u64;
            let p = 32 + i * 24;
            bytes[p..p + 4].copy_from_slice(&tag);
            bytes[p + 4..p + 8].copy_from_slice(&stride.to_le_bytes());
            bytes[p + 8..p + 16].copy_from_slice(&offset.to_le_bytes());
            bytes[p + 16..p + 24].copy_from_slice(&len.to_le_bytes());
            bytes.extend(data);
        }
        let len = bytes.len() as u64;
        bytes[16..24].copy_from_slice(&len.to_le_bytes());
        Container::read(&bytes)?;
        Ok(bytes)
    }
}
