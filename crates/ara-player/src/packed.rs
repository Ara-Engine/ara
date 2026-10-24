//! Cooked asset reads from an immutable exported pack; no source files or importers.
use anyhow::{Context, Result, ensure};
use ara_asset::{CachedImport, Digest, ImportOutput};
use ara_pack::{EncryptionKey, OwnedPack};
use ara_runtime::{RuntimeError, assets::AssetSource};
use std::{collections::BTreeMap, sync::Arc};

pub(super) struct SharedAssets(pub(super) Arc<dyn AssetSource>);
impl AssetSource for SharedAssets {
    fn load(&self, asset: &str) -> ara_runtime::Result<Option<(Digest, CachedImport)>> {
        self.0.load(asset)
    }
}

pub(super) struct PackedAssets {
    pack: Arc<OwnedPack>,
    key: Option<EncryptionKey>,
    pins: BTreeMap<String, Digest>,
}

impl PackedAssets {
    pub(super) fn new(
        pack: Arc<OwnedPack>,
        key: Option<EncryptionKey>,
        scene: &ara_play::PlayScene,
    ) -> Result<Self> {
        let pins = scene
            .assets
            .iter()
            .map(|(id, value)| {
                let digest = Digest(ara_play::unhex(value).map_err(anyhow::Error::msg)?);
                ensure!(
                    pack.view()
                        .manifest()
                        .entries
                        .contains_key(&Self::path(digest)),
                    "exported generation missing for {id}: {digest}"
                );
                Ok((id.to_owned(), digest))
            })
            .collect::<Result<_>>()?;
        Ok(Self { pack, key, pins })
    }

    fn path(digest: Digest) -> String {
        format!(".ara/cooked/{digest}.cbor")
    }

    fn generation(&self, asset: &str) -> Result<Option<(Digest, CachedImport)>> {
        let Some(&digest) = self.pins.get(asset) else {
            return Ok(None);
        };
        let bytes = self
            .pack
            .view()
            .read(&Self::path(digest), 1024 * 1024 * 1024, self.key.as_ref())
            .with_context(|| format!("cooked asset {asset}"))?;
        let output: ImportOutput = ciborium::de::from_reader(bytes.as_slice())?;
        output.validate()?;
        Ok(Some((
            digest,
            CachedImport {
                key: digest,
                output,
            },
        )))
    }
}

impl AssetSource for PackedAssets {
    fn load(&self, asset: &str) -> ara_runtime::Result<Option<(Digest, CachedImport)>> {
        self.generation(asset)
            .map_err(|error| RuntimeError(format!("{error:#}")))
    }
}
