//! File identities: record IDs (unique per document) and asset IDs (unique per
//! project), plus a seedable ID minter.
//!
//! IDs are short, prefixed and random (`e_8b21`, `pf_91fd02ab`), never 128-bit GUIDs:
//! agents copy them reliably and diffs stay readable.

use std::borrow::Borrow;
use std::fmt;
use std::hash::{BuildHasher, RandomState};

use ara_schema::grammar::split_id;
use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($(#[doc = $doc:literal])+ $name:ident) => {
        $(#[doc = $doc])+
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// Parses and validates an ID.
            pub fn parse(s: &str) -> Result<Self, String> {
                split_id(s).map(|_| $name(s.to_owned())).ok_or_else(|| format!("`{s}` is not a valid ID (expected `<prefix>_<base32>`, e.g. `e_8b21`)"))
            }

            /// The ID text.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// The prefix before `_`.
            pub fn prefix(&self) -> &str {
                self.0.split_once('_').map_or("", |(p, _)| p)
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(s: String) -> Result<Self, String> {
                $name::parse(&s)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type! {
    /// A record ID, unique within its document (`e_8b21`). The prefix selects the
    /// record type.
    RecordId
}

id_type! {
    /// An asset ID, unique within the project (`pf_91fd02ab`). The prefix names the
    /// asset kind.
    AssetId
}

/// Base32 without `i`, `l`, `o`, `u`, so IDs survive being read aloud or retyped.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Record ID body length. Collisions grow it (see [`IdGen::record_id`]).
const RECORD_LEN: usize = 4;
/// Asset ID body length: 40 bits, so branches minting in parallel rarely collide.
const ASSET_LEN: usize = 8;

/// Mints IDs from a SplitMix64 stream. Seed it for deterministic tests and replays.
#[derive(Debug, Clone)]
pub struct IdGen {
    state: u64,
}

impl IdGen {
    /// A deterministic generator.
    pub fn seeded(seed: u64) -> Self {
        IdGen { state: seed }
    }

    /// A generator seeded from the process's random hasher keys.
    pub fn from_entropy() -> Self {
        IdGen::seeded(RandomState::new().hash_one(0x0a7a_u64))
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// `prefix_` plus `len` random base32 characters.
    pub fn token(&mut self, prefix: &str, len: usize) -> String {
        let mut s = format!("{prefix}_");
        for _ in 0..len {
            s.push(char::from(ALPHABET[(self.next_u64() % 32) as usize]));
        }
        s
    }

    /// A new record ID that `taken` rejects as unused. Starts at 4 characters and
    /// grows by one after repeated collisions.
    pub fn record_id(&mut self, prefix: &str, taken: impl Fn(&str) -> bool) -> RecordId {
        let mut len = RECORD_LEN;
        loop {
            for _ in 0..8 {
                let id = self.token(prefix, len);
                if !taken(&id) {
                    return RecordId(id);
                }
            }
            len += 1;
        }
    }

    /// A new asset ID.
    pub fn asset_id(&mut self, prefix: &str) -> AssetId {
        AssetId(self.token(prefix, ASSET_LEN))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn minted_ids_are_valid_and_avoid_collisions() {
        let mut g = IdGen::seeded(7);
        let mut taken = BTreeSet::new();
        for _ in 0..2000 {
            let id = g.record_id("e", |s| taken.contains(s));
            assert!(RecordId::parse(id.as_str()).is_ok());
            assert!(taken.insert(id.as_str().to_owned()));
        }
        assert!(AssetId::parse(g.asset_id("pf").as_str()).is_ok());
        assert_eq!(
            IdGen::seeded(1).token("e", 4),
            IdGen::seeded(1).token("e", 4)
        );
    }
}
