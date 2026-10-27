//! Frozen importer and document capabilities; no source parsing runs in the host.
use crate::import::Importers;
use ara_asset::{Error, ImportSettings, Result, SourceAsset, TargetPlatform};
use ara_doc::Workspace;
use ara_schema::SchemaDb;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) type AssetHashes = BTreeMap<String, (String, String, ara_asset::Digest)>;

#[derive(Clone)]
pub(super) struct Bindings {
    pub schema: SchemaDb,
    pub workspace: Workspace,
    pub versions: BTreeMap<String, String>,
    pub preset: ara_asset::CookPreset,
    pub asset_hashes: AssetHashes,
}

impl Bindings {
    pub fn existing(&self, path: &str) -> Result<Option<SourceAsset>> {
        let sidecar = self.workspace.storage_path(&format!("{path}.import.json"));
        let document = self
            .workspace
            .docs()
            .find_map(|(name, doc)| (name == sidecar).then_some(doc));
        let Some(document) = document else {
            return self.source(path, None);
        };
        let record = document.record("i_settings").ok_or_else(|| {
            Error::Invalid(format!("import sidecar for `{path}` has no settings"))
        })?;
        let settings: ImportSettings = serde_json::from_value(Value::Object(record.clone()))?;
        self.binding(path, document.id.to_string(), settings)
            .map(Some)
    }

    pub fn capture_assets(
        &mut self,
        inventory: &super::inventory::Inventory,
    ) -> std::result::Result<(), Vec<ara_diag::Diagnostic>> {
        for (storage, document) in self.workspace.docs() {
            if self.workspace.is_project_document(document.id.as_str()) {
                continue;
            }
            let path = self.workspace.reference_path(storage);
            let (path, bytes) = if document.kind == "import" {
                let media = path.strip_suffix(".import.json").ok_or_else(|| {
                    vec![ara_diag::Diagnostic::error(
                        ara_diag::codes::INVALID_PARAMS,
                        "import document has no media path",
                    )]
                })?;
                if inventory
                    .prepared
                    .entries
                    .get(&self.workspace.storage_path(media))
                    .and_then(Option::as_ref)
                    .is_none()
                {
                    continue;
                }
                (
                    media.to_owned(),
                    inventory.content(&self.workspace.storage_path(media))?,
                )
            } else {
                (
                    path.to_owned(),
                    ara_host::project::write_document(document, &self.schema)
                        .map_err(|diagnostic| vec![diagnostic])?
                        .into_bytes(),
                )
            };
            let proof = (
                path.clone(),
                document.id.to_string(),
                ara_asset::Digest::of(&bytes),
            );
            self.asset_hashes.insert(path, proof.clone());
            self.asset_hashes.insert(document.id.to_string(), proof);
        }
        Ok(())
    }

    fn binding(&self, path: &str, id: String, settings: ImportSettings) -> Result<SourceAsset> {
        let binding = self.template(path, id, &settings)?;
        self.resolve(binding, &settings, &self.asset_hashes)
    }

    pub fn template(
        &self,
        path: &str,
        id: String,
        settings: &ImportSettings,
    ) -> Result<SourceAsset> {
        let installed = self.versions.get(&settings.importer).ok_or_else(|| {
            Error::Invalid(format!(
                "importer `{}` is unavailable for `{path}`",
                settings.importer
            ))
        })?;
        if settings.check_version(installed) == ara_asset::VersionCheck::Newer {
            return Err(Error::Invalid(format!(
                "the sidecar for `{path}` was authored by a newer importer; update Ara or install that importer before preparing this asset"
            )));
        }
        self.validate_settings(&settings.importer, &settings.settings)?;
        Ok(SourceAsset {
            preset: Some(settings.preset.unwrap_or(self.preset)),
            thread_budget: 2,
            id,
            path: path.to_owned(),
            importer: settings.importer.clone(),
            version: installed.clone(),
            settings: settings.settings.clone(),
            target: if cfg!(target_os = "windows") {
                TargetPlatform::Windows
            } else if cfg!(target_os = "macos") {
                TargetPlatform::MacOs
            } else {
                TargetPlatform::Linux
            },
            asset_dependencies: BTreeMap::new(),
        })
    }

    pub fn resolve(
        &self,
        mut binding: SourceAsset,
        settings: &ImportSettings,
        hashes: &AssetHashes,
    ) -> Result<SourceAsset> {
        for dependency in &settings.dependencies {
            let reference =
                ara_schema::grammar::parse_ref(&dependency.0).map_err(Error::Invalid)?;
            let proof=reference.id.and_then(|id|hashes.get(id)).or_else(||reference.path.and_then(|path|hashes.get(path))).ok_or_else(||Error::Invalid(format!("unresolved importer dependency `{}`; include its native asset in the same selection or import it first",dependency.0)))?;
            binding.asset_dependencies.insert(proof.0.clone(), proof.2);
            if !proof.1.is_empty() {
                binding.asset_dependencies.insert(proof.1.clone(), proof.2);
            }
        }
        Ok(binding)
    }
    fn validate_settings(&self, importer: &str, settings: &Value) -> Result<()> {
        let name = match importer {
            "ara.texture" => "TextureSettings",
            "ara.gltf" | "ara.fbx" => "MeshSettings",
            "ara.audio" => "AudioImportSettings",
            _ => return Ok(()),
        };
        let description = self.schema.get(name).ok_or_else(|| {
            Error::Invalid(format!(
                "installed importer settings schema `{name}` is unavailable"
            ))
        })?;
        let object = settings
            .as_object()
            .ok_or_else(|| Error::Invalid("importer settings must be an object".into()))?;
        let mut diagnostics = Vec::new();
        ara_schema::Validator::new(&self.schema, &mut diagnostics).fields(
            description,
            object,
            "settings",
        );
        let errors: Vec<_> = diagnostics
            .into_iter()
            .filter(ara_diag::Diagnostic::is_error)
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Error::Invalid(
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ))
        }
    }
    pub fn description(
        &self,
        destination: &str,
        foreign_sidecar: Option<&[u8]>,
    ) -> Result<Option<(String, ImportSettings)>> {
        if self.native_document(destination)
            || destination.ends_with(".cs")
            || destination.ends_with(".json")
            || destination.ends_with(".uss")
        {
            return Ok(None);
        }
        if let Some(bytes) = foreign_sidecar {
            let text =
                std::str::from_utf8(bytes).map_err(|error| Error::Invalid(error.to_string()))?;
            let sidecar = ara_asset::Sidecar::read(
                text,
                &format!("{destination}.import.json"),
                &self.schema,
            )?;
            return Ok(Some((
                sidecar.document().id.to_string(),
                sidecar.settings()?,
            )));
        }
        let Ok(importer) = ara_importers::commands::importer_for_source(destination) else {
            return Ok(None);
        };
        let version = self
            .versions
            .get(importer)
            .ok_or_else(|| Error::Invalid(format!("importer `{importer}` is unavailable")))?;
        let id = format!(
            "as_{}",
            &ara_asset::Digest::of(destination.as_bytes()).to_string()[..16]
        );
        Ok(Some((
            id,
            ImportSettings {
                importer: importer.to_owned(),
                version: version.clone(),
                settings: json!({}),
                preset: None,
                dependencies: Vec::new(),
            },
        )))
    }
}

impl Importers for Bindings {
    fn source(
        &self,
        destination: &str,
        foreign_sidecar: Option<&[u8]>,
    ) -> Result<Option<SourceAsset>> {
        self.description(destination, foreign_sidecar)?
            .map(|(id, settings)| self.binding(destination, id, settings))
            .transpose()
    }
    fn native_document(&self, destination: &str) -> bool {
        self.schema
            .kinds()
            .any(|kind| destination.ends_with(&format!(".{}", kind.extension)))
    }
}
