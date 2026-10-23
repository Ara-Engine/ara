use crate::*;
use anyhow::{Context, Result, ensure};

/// Quantized vertex on an asset-wide grid. Shared borders decode bit-identically.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct PackedVertex {
    /// UNORM16 position on Geometry::{min,scale}.
    pub position: [u16; 3],
    /// SNORM16 shading normal.
    pub normal: [i16; 3],
    /// Half-float UV coordinates.
    pub uv: [u16; 2],
}
impl PackedVertex {
    pub(crate) fn pack(v: Vertex, min: [f32; 3], scale: [f32; 3]) -> Self {
        let length = v
            .normal
            .iter()
            .map(|v| v * v)
            .sum::<f32>()
            .sqrt()
            .max(1e-20);
        Self {
            position: std::array::from_fn(|i| {
                if scale[i] > 0.0 {
                    ((v.position[i] - min[i]) / scale[i])
                        .round()
                        .clamp(0.0, 65535.0) as u16
                } else {
                    0
                }
            }),
            normal: v
                .normal
                .map(|v| meshopt::quantize_snorm(v / length, 16) as i16),
            uv: v.uv.map(|v| half::f16::from_f32(v).to_bits()),
        }
    }
    /// Decodes a position using the shared grid, independent of cluster bounds.
    pub fn unpack(self, min: [f32; 3], scale: [f32; 3]) -> Vertex {
        Vertex {
            position: std::array::from_fn(|i| min[i] + f32::from(self.position[i]) * scale[i]),
            normal: self.normal.map(|v| f32::from(v) / 32767.0),
            uv: self.uv.map(|v| half::f16::from_bits(v).to_f32()),
        }
    }
}
/// Decoded page prepared off-thread for upload into a resident cluster slot.
pub struct Page {
    /// At most 64 local vertices.
    pub vertices: Vec<PackedVertex>,
    /// At most 128 local U8 triangle triplets.
    pub triangles: Vec<u8>,
}
/// Versioned geometry with one independently decodable page per cluster.
pub struct Geometry {
    /// Quantization origin in local metres.
    pub min: [f32; 3],
    /// Metres per UNORM16 position step.
    pub scale: [f32; 3],
    /// Original and replacement meshlets, ordered fine to coarse.
    pub clusters: Vec<Cluster>,
    /// DAG replacement operations.
    pub groups: Vec<Group>,
    /// Group child lists.
    pub children: Vec<u32>,
    /// Independently meshopt-compressed vertex and index streams.
    pub payload: Vec<u8>,
    /// Cook diagnostics; no timing or machine data.
    pub diagnostics: Diagnostics,
}
impl Geometry {
    /// Content identity used to reject streaming pages from another cooked asset.
    pub fn identity(&self) -> Result<[u8; 32]> {
        Ok(ara_asset::Digest::of(&self.encode()?).0)
    }
    /// Decodes a single cluster; malformed streams never reach GPU upload.
    pub fn cluster(&self, id: u32) -> Result<Page> {
        let c = self
            .clusters
            .get(id as usize)
            .context("ARA_GEO_PAGE: invalid cluster")?;
        let a = c.offset as usize;
        let b = a
            .checked_add(c.vertex_bytes as usize)
            .context("page overflow")?;
        let end = b
            .checked_add(c.index_bytes as usize)
            .context("page overflow")?;
        let vertices = meshopt::decode_vertex_buffer(
            self.payload
                .get(a..b)
                .context("vertex stream outside payload")?,
            c.vertex_count as usize,
        )?;
        let indices: Vec<u16> = meshopt::decode_index_buffer(
            self.payload
                .get(b..end)
                .context("index stream outside payload")?,
            c.triangle_count as usize * 3,
        )?;
        ensure!(
            indices.iter().all(|&i| u32::from(i) < c.vertex_count),
            "ARA_GEO_PAGE: invalid local index"
        );
        Ok(Page {
            vertices,
            triangles: indices.into_iter().map(|i| i as u8).collect(),
        })
    }
    /// Checks metadata and DAG links before persistence/publication.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.clusters.is_empty()
                && self.min.iter().chain(&self.scale).all(|v| v.is_finite())
                && self.scale.iter().all(|v| *v >= 0.0),
            "ARA_GEO_FORMAT: invalid bounds/empty geometry"
        );
        let sphere = |b: Sphere| {
            b.center.iter().all(|v| v.is_finite()) && b.radius.is_finite() && b.radius >= 0.0
        };
        let mut parents = vec![ROOT; self.clusters.len()];
        for (i, g) in self.groups.iter().enumerate() {
            ensure!(
                sphere(g.bounds)
                    && g.error.is_finite()
                    && g.error >= 0.0
                    && g.level > 0
                    && g.child_count > 0,
                "ARA_GEO_FORMAT: invalid group"
            );
            let end = (g.first_child as usize)
                .checked_add(g.child_count as usize)
                .context("child range overflow")?;
            let children = self
                .children
                .get(g.first_child as usize..end)
                .context("invalid group child range")?;
            for &id in children {
                let c = self.clusters.get(id as usize).context("invalid child")?;
                ensure!(
                    c.parent == i as u32 && c.level < g.level && parents[id as usize] == ROOT,
                    "ARA_GEO_FORMAT: duplicate/cyclic parent"
                );
                parents[id as usize] = i as u32;
                if c.group != ROOT {
                    let child = self
                        .groups
                        .get(c.group as usize)
                        .context("invalid child group")?;
                    ensure!(
                        c.group < i as u32 && child.error <= g.error,
                        "ARA_GEO_FORMAT: nonmonotonic DAG"
                    );
                }
            }
        }
        for (i, c) in self.clusters.iter().enumerate() {
            ensure!(
                sphere(c.bounds)
                    && (1..=64).contains(&c.vertex_count)
                    && (1..=128).contains(&c.triangle_count)
                    && c.vertex_bytes > 0
                    && c.index_bytes > 0
                    && u64::from(c.offset) + u64::from(c.vertex_bytes) + u64::from(c.index_bytes)
                        <= self.payload.len() as u64
                    && c.parent == parents[i],
                "ARA_GEO_FORMAT: invalid cluster"
            );
            if c.group == ROOT {
                ensure!(c.level == 0, "invalid leaf level");
            } else {
                let g = self.groups.get(c.group as usize).context("invalid group")?;
                ensure!(g.level == c.level, "invalid replacement level");
            }
        }
        Ok(())
    }
    /// Encodes fixed little-endian metadata, diagnostics and compressed payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let diagnostics = ara_json::to_canonical_string(&ara_json::sort_keys(
            &serde_json::to_value(&self.diagnostics)?,
        ));
        let mut bytes = b"ARAG".to_vec();
        for word in [
            VERSION as usize,
            self.clusters.len(),
            self.groups.len(),
            self.children.len(),
            self.payload.len(),
            diagnostics.len(),
        ]
        .into_iter()
        .map(u32::try_from)
        {
            bytes.extend(word?.to_le_bytes());
        }
        bytes.extend(bytemuck::cast_slice(&self.min));
        bytes.extend(bytemuck::cast_slice(&self.scale));
        bytes.extend(bytemuck::cast_slice(&self.clusters));
        bytes.extend(bytemuck::cast_slice(&self.groups));
        bytes.extend(bytemuck::cast_slice(&self.children));
        bytes.extend(&self.payload);
        bytes.extend(diagnostics.as_bytes());
        Ok(bytes)
    }
    /// Validates byte extents before allocating tables. Does not eagerly decode pages.
    pub fn decode(mut bytes: &[u8]) -> Result<Self> {
        ensure!(
            cfg!(target_endian = "little"),
            "little-endian runtime required"
        );
        ensure!(
            take(&mut bytes, 4)? == b"ARAG",
            "ARA_GEO_FORMAT: wrong magic"
        );
        let header: Vec<u32> = read(&mut bytes, 6)?;
        ensure!(header[0] == VERSION, "ARA_GEO_VERSION: recook asset");
        let min: Vec<f32> = read(&mut bytes, 3)?;
        let scale: Vec<f32> = read(&mut bytes, 3)?;
        let clusters = read(&mut bytes, header[1] as usize)?;
        let groups = read(&mut bytes, header[2] as usize)?;
        let children = read(&mut bytes, header[3] as usize)?;
        let payload = take(&mut bytes, header[4] as usize)?.to_vec();
        let diagnostics = serde_json::from_slice(take(&mut bytes, header[5] as usize)?)?;
        ensure!(bytes.is_empty(), "ARA_GEO_FORMAT: trailing bytes");
        let result = Self {
            min: min
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid origin"))?,
            scale: scale
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid scale"))?,
            clusters,
            groups,
            children,
            payload,
            diagnostics,
        };
        result.validate()?;
        Ok(result)
    }
}
fn take<'a>(bytes: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    let (a, b) = bytes
        .split_at_checked(n)
        .context("ARA_GEO_FORMAT: truncated data")?;
    *bytes = b;
    Ok(a)
}
fn read<T: Pod>(bytes: &mut &[u8], count: usize) -> Result<Vec<T>> {
    let size = count
        .checked_mul(size_of::<T>())
        .context("table overflow")?;
    Ok(take(bytes, size)?
        .chunks_exact(size_of::<T>())
        .map(bytemuck::pod_read_unaligned)
        .collect())
}
