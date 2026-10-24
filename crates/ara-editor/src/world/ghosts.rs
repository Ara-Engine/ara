use super::*;
impl DocumentView {
    pub(super) fn append_ghosts(
        &self,
        _root: &Path,
        state: &EditorState,
        transforms: &BTreeMap<String, Mat4>,
        scene: &mut Scene,
        models: &mut gltf::ModelCache,
    ) -> anyhow::Result<()> {
        let tokens = if state.light {
            ara_ui::theme::EditorTheme::Light
        } else {
            ara_ui::theme::EditorTheme::Dark
        }
        .tokens()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let n = u32::from_str_radix(
            tokens
                .get("--destructive")
                .ok_or_else(|| anyhow::anyhow!("destructive token missing"))?
                .trim_start_matches('#'),
            16,
        )?;
        let ghost_color = [
            (((n >> 16) & 255) as f32 / 255.).powf(2.2),
            (((n >> 8) & 255) as f32 / 255.).powf(2.2),
            ((n & 255) as f32 / 255.).powf(2.2),
            0.3,
        ];
        let mut ghost_worlds: BTreeMap<String, Mat4> = BTreeMap::new();
        for id in self.ghosts.keys() {
            let mut chain = Vec::new();
            let mut current = Some(id.as_str());
            let mut seen = BTreeSet::new();
            while let Some(id) = current {
                if ghost_worlds.contains_key(id) || !seen.insert(id) {
                    break;
                }
                let Some(record) = self.ghosts.get(id) else {
                    break;
                };
                chain.push(id);
                current = ara_doc::parent_of(record);
            }
            while let Some(id) = chain.pop() {
                let record = &self.ghosts[id];
                let parent = ara_doc::parent_of(record)
                    .and_then(|p| ghost_worlds.get(p).or_else(|| transforms.get(p)))
                    .copied()
                    .unwrap_or(Mat4::IDENTITY);
                ghost_worlds.insert(id.to_owned(), parent * local_transform(record));
            }
        }
        for (id, record) in &self.ghosts {
            let Some(mesh_renderer) = record.get("MeshRenderer") else {
                continue;
            };
            let renderer: ara_runtime::components::MeshRenderer =
                serde_json::from_value(mesh_renderer.clone()).unwrap_or_default();
            let material = scene.materials.len();
            scene.materials.push(Material {
                base_color: ghost_color,
                flags: [2, 0, 0, 0],
                ..Material::default()
            });
            let name = format!(
                "{} (deleted)",
                record.get("name").and_then(Value::as_str).unwrap_or(id)
            );
            let mut mesh = if let Some(reference) = &renderer.model {
                let parts =
                    ara_schema::grammar::parse_ref(&reference.0).map_err(anyhow::Error::msg)?;
                models.load(
                    parts
                        .id
                        .ok_or_else(|| anyhow::anyhow!("mesh asset ID missing"))?,
                    &renderer,
                    render_id(id),
                    name,
                    material,
                )?
            } else {
                super::shape(&renderer, render_id(id), name, material)
            };
            let matrix = ghost_worlds.get(id).copied().unwrap_or(Mat4::IDENTITY);
            let normal = matrix.inverse().transpose();
            for v in &mut mesh.vertices {
                v.position = matrix.transform_point3(v.position.into()).into();
                v.normal = normal
                    .transform_vector3(v.normal.into())
                    .normalize_or_zero()
                    .into();
            }
            scene.meshes.push(mesh);
        }
        Ok(())
    }
}
