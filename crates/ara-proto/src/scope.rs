//! Capability scopes (R03 §4.10). Sessions are granted a set; every method and
//! command names the one it needs, and the host checks it before running anything.

use std::fmt;
use std::str::FromStr;

use ara_diag::{Diagnostic, codes, did_you_mean};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

macro_rules! scopes {
    ($($(#[doc = $doc:literal])+ $variant:ident = $text:literal,)*) => {
        /// A capability a session may hold.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Scope {
            $($(#[doc = $doc])+ $variant,)*
        }

        impl Scope {
            /// Every scope, in vocabulary order.
            pub const ALL: &[Scope] = &[$(Scope::$variant),*];

            /// The wire name (`edit:scene`).
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Scope::$variant => $text,)*
                }
            }

            /// What the scope allows, for docs and approval prompts.
            pub fn doc(self) -> &'static str {
                match self {
                    $(Scope::$variant => concat!($($doc),+).trim_ascii(),)*
                }
            }
        }
    };
}

scopes! {
    /// Read documents, schemas, history, logs and docs.
    Read = "read",
    /// Edit scenes and prefabs through commands.
    EditScene = "edit:scene",
    /// Edit other authored assets (materials, UI, settings).
    EditAssets = "edit:assets",
    /// Edit game and editor code.
    EditCode = "edit:code",
    /// Start, step and control play worlds.
    Play = "play",
    /// Run scripts against the typed SDK (code mode).
    Run = "run",
    /// Import source media.
    Import = "import",
    /// Bake derived data (lighting, navigation).
    Bake = "bake",
    /// Build and export the game.
    Build = "build",
    /// Run generation jobs that spend money.
    GenerateSpend = "generate:$",
    /// Read or write files outside the project.
    FsOutsideProject = "fs:outside-project",
    /// Use the network.
    Net = "net",
}

impl Scope {
    /// What the default approval policy grants agents (MCP clients): everything
    /// except spending money, leaving the project and the network.
    pub fn agent_default() -> Vec<Scope> {
        Scope::ALL
            .iter()
            .copied()
            .filter(|s| {
                !matches!(
                    s,
                    Scope::GenerateSpend | Scope::FsOutsideProject | Scope::Net
                )
            })
            .collect()
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Scope {
    type Err = Diagnostic;

    fn from_str(s: &str) -> Result<Self, Diagnostic> {
        Scope::ALL
            .iter()
            .copied()
            .find(|scope| scope.as_str() == s)
            .ok_or_else(|| {
                let mut msg = format!("unknown scope `{s}`");
                if let Some(near) = did_you_mean(s, Scope::ALL.iter().map(|x| x.as_str())) {
                    msg.push_str(&format!("; did you mean `{near}`?"));
                }
                Diagnostic::error(codes::UNKNOWN_SCOPE, msg)
            })
    }
}

impl Serialize for Scope {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(|e: Diagnostic| serde::de::Error::custom(e.message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_round_trip_and_suggest() {
        for s in Scope::ALL {
            assert_eq!(s.as_str().parse::<Scope>().ok(), Some(*s));
            assert!(!s.doc().is_empty());
        }
        let err = "edit:scen".parse::<Scope>().expect_err("typo");
        assert_eq!(err.code, codes::UNKNOWN_SCOPE);
        assert!(err.message.contains("`edit:scene`"), "{}", err.message);
        let agent = Scope::agent_default();
        assert!(agent.contains(&Scope::EditScene) && !agent.contains(&Scope::Net));
    }
}
