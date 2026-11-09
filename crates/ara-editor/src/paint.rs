//! Mesh Paint (Unreal's vertex paint mode): brush, erase, fill, flood and pick over
//! the per-instance colours of the selected entity's primitives. Strokes edit a
//! working copy (linear floats, so slow brushes still accumulate) that the Scene
//! previews as transient paint; release commits one `vertexPaint.set` per changed
//! primitive in one transaction, so a stroke is one undo step.
use ara_runtime::model::Primitive;
use glam::{Mat4, Vec3, Vec4};
use std::sync::Arc;

/// What a press in the Scene does in Mesh Paint mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaintTool {
    /// Blend the paint colour in under the brush.
    #[default]
    Brush,
    /// Blend the erase colour in under the brush.
    Erase,
    /// Set every vertex of the clicked entity.
    Fill,
    /// Set the clicked connected island of triangles.
    Flood,
    /// Take the colour under the cursor as the paint colour.
    Pick,
}

impl PaintTool {
    /// Stable command/agent name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Brush => "brush",
            Self::Erase => "erase",
            Self::Fill => "fill",
            Self::Flood => "flood",
            Self::Pick => "pick",
        }
    }
    /// Parses [`Self::name`].
    pub fn parse(name: &str) -> Option<Self> {
        [
            Self::Brush,
            Self::Erase,
            Self::Fill,
            Self::Flood,
            Self::Pick,
        ]
        .into_iter()
        .find(|t| t.name() == name)
    }
}

/// Brush and colour settings (Unreal's Mesh Paint panel).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaintSettings {
    /// Active tool.
    pub tool: PaintTool,
    /// Linear RGBA painted by the brush, fill and flood.
    pub color: [f32; 4],
    /// Linear RGBA the erase brush blends towards.
    pub erase: [f32; 4],
    /// Brush radius in world metres.
    pub radius: f32,
    /// Share of the way to the target colour one dab moves a vertex at the centre.
    pub strength: f32,
    /// Share of the radius over which the brush fades out (0 hard edge, 1 all of it).
    pub falloff: f32,
    /// Channels written (R, G, B, A); the others keep their values.
    pub channels: [bool; 4],
}

impl Default for PaintSettings {
    fn default() -> Self {
        Self {
            tool: PaintTool::Brush,
            color: [1.0; 4],
            erase: [0.0, 0.0, 0.0, 1.0],
            radius: 0.5,
            strength: 0.5,
            falloff: 0.5,
            channels: [true; 4],
        }
    }
}

impl PaintSettings {
    /// The brush weight at distance `d` from the centre: `strength` inside the hard
    /// core, smoothly to zero at the radius over the falloff band.
    pub fn weight(&self, d: f32) -> f32 {
        if d >= self.radius || self.radius <= 0.0 {
            return 0.0;
        }
        let t = d / self.radius;
        let core = 1.0 - self.falloff.clamp(0.0, 1.0);
        let fade = if t <= core {
            1.0
        } else {
            let x = ((t - core) / (1.0 - core).max(1e-6)).clamp(0.0, 1.0);
            1.0 - x * x * (3.0 - 2.0 * x)
        };
        self.strength.clamp(0.0, 1.0) * fade
    }
}

/// One painted primitive of the entity: its drawn geometry, world pose and the
/// working colours per source vertex.
pub struct Layer {
    /// Draw identity in the Scene's frame (`Draw::entity`).
    pub draw: u64,
    /// Subtree-local slot (`PaintedPrimitive::primitive`).
    pub slot: u32,
    /// The drawn model generation (cooked vertices, LOD 0 indices, source map).
    pub model: Arc<ara_runtime::model::Model>,
    /// Index into the model's primitives.
    pub index: usize,
    /// Model to world.
    pub world: Mat4,
    /// Working linear colours per source vertex.
    pub colors: Vec<Vec4>,
    /// Whether the stroke changed them.
    pub changed: bool,
}

/// Where a ray met a painted primitive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// Layer index.
    pub layer: usize,
    /// Triangle (first index in LOD 0).
    pub triangle: usize,
    /// World position.
    pub point: Vec3,
    /// Barycentric weights of the triangle's corners.
    pub barycentric: [f32; 3],
    /// Ray distance.
    pub distance: f32,
}

impl Layer {
    fn primitive(&self) -> &Primitive {
        &self.model.primitives[self.index]
    }
    /// Initial working colours: `existing` paint, else the imported vertex colours.
    pub fn seed(&mut self, existing: Option<&[[u8; 4]]>) {
        let primitive = &self.model.primitives[self.index];
        let count = primitive.source_vertex_count();
        self.colors = match existing.filter(|c| c.len() == count) {
            Some(colors) => colors.iter().map(|c| unpack(*c)).collect(),
            None => {
                let mut colors = vec![Vec4::ONE; count];
                for (i, v) in primitive.vertices.iter().enumerate() {
                    colors[primitive.source_vertex(i)] = unpack(v.color);
                }
                colors
            }
        };
    }
    /// The working colours, quantized per source vertex.
    pub fn quantized(&self) -> Vec<[u8; 4]> {
        self.colors.iter().map(|c| pack(*c)).collect()
    }
    /// The working colours per drawn vertex (`Draw::paint`).
    pub fn cooked(&self) -> Arc<[u32]> {
        let primitive = self.primitive();
        (0..primitive.vertices.len())
            .map(|i| u32::from_le_bytes(pack(self.colors[primitive.source_vertex(i)])))
            .collect()
    }
}

fn unpack(c: [u8; 4]) -> Vec4 {
    Vec4::from_array(c.map(|c| f32::from(c) / 255.0))
}
fn pack(c: Vec4) -> [u8; 4] {
    c.to_array()
        .map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

/// The nearest hit of the ray (`origin`, unit `direction`) on any layer's LOD 0.
pub fn hit(layers: &[Layer], origin: Vec3, direction: Vec3) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    for (index, layer) in layers.iter().enumerate() {
        let primitive = layer.primitive();
        let world: Vec<Vec3> = primitive
            .vertices
            .iter()
            .map(|v| layer.world.transform_point3(Vec3::from_array(v.position)))
            .collect();
        for (t, tri) in primitive.indices.as_chunks::<3>().0.iter().enumerate() {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| world[i as usize]);
            let (e1, e2) = (b - a, c - a);
            let p = direction.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-12 {
                continue;
            }
            let inv = 1.0 / det;
            let s = origin - a;
            let u = s.dot(p) * inv;
            let q = s.cross(e1);
            let v = direction.dot(q) * inv;
            let distance = e2.dot(q) * inv;
            if u < 0.0 || v < 0.0 || u + v > 1.0 || distance <= 0.0 {
                continue;
            }
            if best.is_none_or(|h| distance < h.distance) {
                best = Some(Hit {
                    layer: index,
                    triangle: t * 3,
                    point: origin + direction * distance,
                    barycentric: [1.0 - u - v, u, v],
                    distance,
                });
            }
        }
    }
    best
}

/// Blends `target` into every vertex within the brush of `centre` (world), on the
/// selected channels. Returns whether any colour moved.
pub fn dab(layers: &mut [Layer], centre: Vec3, settings: &PaintSettings, target: [f32; 4]) -> bool {
    let target = Vec4::from_array(target);
    let mask = Vec4::from_array(settings.channels.map(f32::from));
    let mut any = false;
    for layer in layers {
        let world = layer.world;
        let primitive = &layer.model.primitives[layer.index];
        // Each source vertex once (seam copies share a source vertex).
        let mut done = vec![false; layer.colors.len()];
        for (i, v) in primitive.vertices.iter().enumerate() {
            let source = primitive.source_vertex(i);
            if done[source] {
                continue;
            }
            done[source] = true;
            let d = world
                .transform_point3(Vec3::from_array(v.position))
                .distance(centre);
            let w = settings.weight(d);
            if w <= 0.0 {
                continue;
            }
            let c = &mut layer.colors[source];
            let next = *c + (target - *c) * w * mask;
            if next != *c {
                *c = next;
                layer.changed = true;
                any = true;
            }
        }
    }
    any
}

/// Sets the selected channels of every vertex of every layer.
pub fn fill(layers: &mut [Layer], color: [f32; 4], channels: [bool; 4]) {
    for layer in layers {
        for c in &mut layer.colors {
            for (k, on) in channels.iter().enumerate() {
                if *on {
                    c[k] = color[k];
                }
            }
        }
        layer.changed = true;
    }
}

/// Sets the selected channels of the triangle island (connected through shared
/// source vertices) containing `hit`'s triangle.
pub fn flood(layers: &mut [Layer], hit: &Hit, color: [f32; 4], channels: [bool; 4]) {
    let layer = &mut layers[hit.layer];
    let primitive = &layer.model.primitives[layer.index];
    let count = layer.colors.len();
    // Union-find over source vertices joined by triangles.
    let mut parent: Vec<usize> = (0..count).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for tri in primitive.indices.as_chunks::<3>().0 {
        let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| primitive.source_vertex(i as usize));
        for (x, y) in [(a, b), (b, c)] {
            let (rx, ry) = (root(&mut parent, x), root(&mut parent, y));
            parent[rx] = ry;
        }
    }
    let seed = root(
        &mut parent,
        primitive.source_vertex(primitive.indices[hit.triangle] as usize),
    );
    for i in 0..count {
        if root(&mut parent, i) == seed {
            for (k, on) in channels.iter().enumerate() {
                if *on {
                    layer.colors[i][k] = color[k];
                }
            }
        }
    }
    layer.changed = true;
}

/// The colour at `hit` (barycentric blend of its corners).
pub fn pick(layers: &[Layer], hit: &Hit) -> [f32; 4] {
    let layer = &layers[hit.layer];
    let primitive = layer.primitive();
    let corners = [0, 1, 2].map(|k| {
        layer.colors[primitive.source_vertex(primitive.indices[hit.triangle + k] as usize)]
    });
    (corners[0] * hit.barycentric[0]
        + corners[1] * hit.barycentric[1]
        + corners[2] * hit.barycentric[2])
        .to_array()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_runtime::model::{Model, ModelVertex};
    use std::collections::BTreeMap;

    /// Two separate triangles (an island each) on the y = 0 plane.
    fn layer() -> Layer {
        let positions = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [5.0, 0.0, 0.0],
            [6.0, 0.0, 0.0],
            [5.0, 0.0, 1.0],
        ];
        let model = Model {
            asset: "as_paint".into(),
            digest: ara_asset::Digest::of(b"paint"),
            primitives: vec![Primitive {
                vertices: positions
                    .map(|position| ModelVertex {
                        position,
                        normal: [0.0, 1.0, 0.0],
                        ..Default::default()
                    })
                    .to_vec(),
                indices: vec![0, 2, 1, 3, 5, 4],
                lods: Vec::new(),
                material: None,
                skin: None,
                casts_shadows: true,
                source_vertices: Vec::new(),
                vertex_colors: false,
            }],
            nodes: BTreeMap::new(),
            node_names: BTreeMap::new(),
            materials: Vec::new(),
            textures: Vec::new(),
            skins: Vec::new(),
            bounds: [[0.0; 3], [6.0, 0.0, 1.0]],
        };
        let mut layer = Layer {
            draw: 1,
            slot: 0,
            model: Arc::new(model),
            index: 0,
            world: Mat4::IDENTITY,
            colors: Vec::new(),
            changed: false,
        };
        layer.seed(None);
        layer
    }

    #[test]
    fn the_brush_fades_over_its_falloff_and_writes_only_masked_channels() {
        let settings = PaintSettings {
            radius: 1.0,
            strength: 1.0,
            falloff: 0.5,
            channels: [true, false, false, false],
            ..Default::default()
        };
        assert_eq!(settings.weight(0.2), 1.0);
        assert!(settings.weight(0.75) > 0.0 && settings.weight(0.75) < 1.0);
        assert_eq!(settings.weight(1.0), 0.0);
        let mut layers = [layer()];
        assert!(dab(
            &mut layers,
            Vec3::ZERO,
            &settings,
            [0.0, 0.0, 0.0, 0.0]
        ));
        let c = layers[0].quantized();
        assert_eq!(c[0], [0, 255, 255, 255], "red only, at the centre");
        assert_eq!(c[3], [255; 4], "out of reach");
        assert!(!dab(
            &mut layers,
            Vec3::new(20.0, 0.0, 0.0),
            &settings,
            [0.0; 4]
        ));
    }

    #[test]
    fn rays_hit_flood_fills_one_island_and_pick_reads_it_back() {
        let mut layers = [layer()];
        let hit =
            hit(&layers, Vec3::new(5.2, 1.0, 0.2), Vec3::NEG_Y).expect("over the second triangle");
        assert_eq!((hit.triangle, hit.distance), (3, 1.0));
        flood(&mut layers, &hit, [0.0, 0.0, 1.0, 1.0], [true; 4]);
        let c = layers[0].quantized();
        assert_eq!(&c[3..], &[[0, 0, 255, 255]; 3]);
        assert_eq!(&c[..3], &[[255; 4]; 3], "the other island keeps its colour");
        assert_eq!(pick(&layers, &hit), [0.0, 0.0, 1.0, 1.0]);
        fill(
            &mut layers,
            [0.5, 0.5, 0.5, 0.5],
            [false, false, false, true],
        );
        assert_eq!(layers[0].quantized()[0], [255, 255, 255, 128]);
        assert!(super::hit(&layers, Vec3::new(3.0, 1.0, 0.2), Vec3::NEG_Y).is_none());
    }
}
