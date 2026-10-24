//! An immutable owned pack validates stored bytes once, then lends cheap read views.
use crate::{Error, Manifest, Pack};
use std::borrow::Cow;

/// An owned, validated content pack for runtimes that load many cooked generations.
/// Read views borrow its immutable bytes and manifest without rehashing the whole
/// pack for every asset. Individual reads still validate decompressed file hashes.
pub struct OwnedPack {
    bytes: Box<[u8]>,
    data: usize,
    manifest: Manifest,
}

impl OwnedPack {
    /// Validates the header, manifest, ranges and stored chunk identities once.
    pub fn open(bytes: Vec<u8>) -> Result<Self, Error> {
        let (data, manifest) = {
            let view = Pack::open(&bytes)?;
            (view.data, view.manifest.into_owned())
        };
        Ok(Self {
            bytes: bytes.into_boxed_slice(),
            data,
            manifest,
        })
    }

    /// Lends a read view whose backing bytes cannot change or outlive this owner.
    pub fn view(&self) -> Pack<'_> {
        Pack {
            bytes: &self.bytes,
            data: self.data,
            manifest: Cow::Borrowed(&self.manifest),
        }
    }
}

#[cfg(all(test, feature = "write"))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn owned_views_share_the_checked_manifest_and_verify_reads() {
        let built = crate::build(
            [crate::Input {
                path: "mesh.bin".into(),
                bytes: vec![7; 8192],
            }],
            crate::CookKey {
                platform: "windows".into(),
                arch: "x64".into(),
                config: "dev".into(),
                cooker: "test".into(),
                settings: Default::default(),
            },
            1024,
            None,
        )
        .unwrap();
        let owner = OwnedPack::open(built.bytes).unwrap();
        let a = owner.view();
        let b = owner.view();
        assert!(std::ptr::eq(a.manifest(), b.manifest()));
        assert_eq!(a.read("mesh.bin", 8192, None).unwrap(), vec![7; 8192]);
        assert!(b.read("mesh.bin", 8191, None).is_err());
        assert!(OwnedPack::open(vec![0; 128]).is_err());
    }
}
