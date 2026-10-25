//! Template IDs resolve to the same engine for listing, validation and copying.
use super::*;
use crate::Template;

pub(super) struct Choice {
    pub(super) tile: Template,
    pub(super) directory: Option<PathBuf>,
}

impl Hub {
    pub(super) fn template_catalog(&self, version: Option<&str>) -> Result<Vec<Choice>, RpcError> {
        let mut engines: Vec<_> = self
            .registry
            .engines
            .iter()
            .filter(|engine| {
                version.is_none_or(|v| v == engine.version) && engine.executable.is_file()
            })
            .collect();
        engines.sort_by(|a, b| {
            ara_update::version(&b.version)
                .ok()
                .cmp(&ara_update::version(&a.version).ok())
                .then_with(|| b.version.cmp(&a.version))
        });
        let default_version = version
            .map(str::to_owned)
            .or_else(|| {
                self.registry
                    .engines
                    .iter()
                    .find(|e| e.executable == self.config.engine.executable)
                    .map(|e| e.version.clone())
            })
            .or_else(|| engines.first().map(|e| e.version.clone()))
            .unwrap_or_else(|| self.config.engine.version.clone());
        let mut choices = Vec::new();
        for engine in engines {
            let Some(root) = engine
                .executable
                .parent()
                .map(|p| p.join("templates"))
                .filter(|p| p.is_dir())
            else {
                continue;
            };
            for mut tile in DirectoryTemplates(root.clone()).list()? {
                if choices
                    .iter()
                    .any(|choice: &Choice| choice.tile.id == tile.id)
                {
                    continue;
                }
                tile.engine_version = Some(engine.version.clone());
                choices.push(Choice {
                    tile,
                    directory: Some(root.clone()),
                });
            }
        }
        for mut tile in self.config.templates.list()? {
            if choices.iter().any(|choice| choice.tile.id == tile.id) {
                continue;
            }
            tile.engine_version = Some(default_version.clone());
            choices.push(Choice {
                tile,
                directory: None,
            });
        }
        Ok(choices)
    }
}
