//! Exact additive diagnostic metadata contract v1; opaque tokens are never ordered.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Key {
    pub producer: String,
    pub module: String,
    pub profile: String,
    pub world: Option<String>,
    pub player: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Order {
    pub play_binding: u64,
    pub compile_binding: u64,
    pub attempt: u64,
}
impl Order {
    pub fn binding(self) -> (u64, u64) {
        (self.play_binding, self.compile_binding)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Bound,
    Started,
    Diagnostic,
    Completed,
}

pub(super) struct Metadata {
    pub key: Key,
    pub order: Order,
    pub instance: String,
    pub source: Option<String>,
    pub phase: Phase,
    pub completion: Option<Completion>,
}

#[derive(Clone)]
pub(super) struct Completion {
    pub result: String,
    pub verified: bool,
    pub errors: u64,
    pub total: u64,
    pub emitted: u64,
    pub truncated: bool,
}

fn token(value: &Value) -> Option<String> {
    let token = value.as_str()?;
    (!token.is_empty() && token.len() <= 4096).then(|| token.to_owned())
}

impl Metadata {
    pub fn parse(diagnostic: &ara_diag::Diagnostic) -> Option<Self> {
        let data = diagnostic.data.as_deref()?;
        let compile = data.get("compile")?;
        if compile["schema"] != 1 {
            return None;
        }
        let producer = compile["producer"].as_str()?;
        let profile = compile["profile"].as_str()?;
        if !matches!(producer, "warm" | "project" | "editor")
            || !matches!(profile, "gameplay" | "editor")
        {
            return None;
        }
        let phase = match compile["phase"].as_str()? {
            "bound" => Phase::Bound,
            "started" => Phase::Started,
            "diagnostic" => Phase::Diagnostic,
            "completed" => Phase::Completed,
            _ => return None,
        };
        let channel = compile["channel"].as_str()?;
        if !matches!(
            channel,
            "compiler" | "analyzer" | "generator" | "dependency"
        ) {
            return None;
        }
        let attempt = compile["attempt"].as_u64()?;
        if (phase == Phase::Bound) != (attempt == 0) {
            return None;
        }
        if phase != Phase::Diagnostic
            && (channel != "compiler"
                || diagnostic.code != ara_diag::Code(906)
                || diagnostic.severity != ara_diag::Severity::Info)
        {
            return None;
        }
        if phase == Phase::Diagnostic && diagnostic.code == ara_diag::Code(906) {
            return None;
        }
        let (world, player, source, play_binding) = match data.get("play") {
            None => (None, None, None, 0),
            Some(play) => (
                Some(token(&play["world"])?),
                Some(u32::try_from(play["player"].as_u64()?).ok()?),
                Some(token(&play["source"])?),
                play["binding"].as_u64()?,
            ),
        };
        Some(Self {
            key: Key {
                producer: producer.into(),
                module: token(&data["module"])?,
                profile: profile.into(),
                world,
                player,
            },
            order: Order {
                play_binding,
                compile_binding: compile["binding"].as_u64()?,
                attempt,
            },
            instance: token(&compile["instance"])?,
            source,
            phase,
            completion: (phase == Phase::Completed)
                .then(|| Completion::parse(compile))
                .flatten(),
        })
    }
}

impl Completion {
    fn parse(value: &Value) -> Option<Self> {
        let result = value["result"].as_str()?;
        let errors = value["errorCount"].as_u64()?;
        let total = value["total"].as_u64()?;
        let emitted = value["emitted"].as_u64()?;
        let truncated = value["truncated"].as_bool()?;
        let verified = match value.get("validation") {
            None => false,
            Some(Value::String(value)) if value == "current-module-inputs" => true,
            _ => return None,
        };
        if !matches!(result, "success" | "errors" | "cancelled")
            || errors > total
            || emitted > total
            || truncated != (emitted < total)
            || (result == "success" && errors != 0)
            || (result == "errors" && errors == 0)
            || (result == "cancelled" && verified)
            || value["coverage"]["errors"] != "module"
            || value["coverage"]["warnings"] != "partial"
        {
            return None;
        }
        Some(Self {
            result: result.into(),
            verified,
            errors,
            total,
            emitted,
            truncated,
        })
    }
}

/// C# source/analyzer/compiler and dependency diagnostics, excluding runtime faults.
pub(super) fn compile_kind(diagnostic: &ara_diag::Diagnostic) -> bool {
    !matches!(diagnostic.code.0,906|910..=912|920..=922|926..=927|930..=934|950..=953|960..=969)
}

/// Unknown tagged families remain visible with incomplete coverage until their owner is confirmed.
pub(super) fn known_compile_kind(diagnostic: &ara_diag::Diagnostic) -> bool {
    matches!(diagnostic.code.0,0..=905|907..=909|923..=925)
}
