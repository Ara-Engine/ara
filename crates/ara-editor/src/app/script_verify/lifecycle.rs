//! Acceptance evidence for exact compiler scopes, separate from retained history.
use super::*;
#[cfg(test)]
mod tests;
mod verify;

const MARKER: &str = "ARA_J10_INVALID_SOURCE";
type Order = (u64, u64, u64);

struct Target {
    entry: Value,
    key: Value,
    order: Order,
    instance: String,
    source: Option<String>,
    first: u64,
    seq: u64,
    captured_seq: u64,
}

#[derive(Default)]
pub(super) struct Lifecycle {
    since: Option<u64>,
    targets: Vec<Target>,
    aliases: Vec<String>,
    before: Value,
    preserved: Vec<Value>,
    preservation: Value,
    assessment: Value,
    verified: bool,
    captured: bool,
    pub console_active: bool,
    console_index: usize,
    problems: Value,
    console: Vec<Value>,
}

impl Lifecycle {
    pub fn arm(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if self.since.is_none() {
            self.since = Some(
                editor.action("logs", json!({"limit":1}))?["cursor"]
                    .as_u64()
                    .ok_or("J10 logs omitted the pre-error cursor")?,
            );
        }
        Ok(())
    }

    /// Freeze every located Spinner error before writing the fixed source.
    pub fn capture(
        &mut self,
        editor: &mut Editor,
        shell: &Shell,
        source: &Path,
    ) -> ara_ui::Result<()> {
        if self.captured {
            return Ok(());
        }
        let since = self
            .since
            .ok_or("J10 compiler inventory was not armed before Broken")?;
        let records = errors(editor)?;
        let root = editor.host.root();
        let storage = source.strip_prefix(root)?.to_string_lossy();
        let logical = storage.replace('\\', "/");
        let logical = logical
            .strip_prefix(&format!("{}/", ara_doc::AUTHORING_ROOT))
            .unwrap_or(&logical);
        let aliases = vec![
            normal(&source.to_string_lossy()),
            normal(&storage),
            normal(logical),
        ];
        let mut targets = Vec::new();
        for entry in &records {
            if entry["seq"].as_u64().is_none_or(|seq| seq <= since) {
                continue;
            }
            let diagnostic = &entry["diagnostic"];
            if !diagnostic["message"]
                .as_str()
                .is_some_and(|m| m.contains(MARKER))
            {
                continue;
            }
            // An unlocated global summary is not a scoped per-file compiler row.
            let Some(file) = diagnostic["location"]["file"].as_str() else {
                continue;
            };
            if !aliases.contains(&normal(file)) {
                continue;
            }
            let target = Target::parse(entry)?;
            if !targets.iter().any(|old: &Target| old.first == target.first) {
                targets.push(target);
            }
        }
        if targets.is_empty() {
            return Err("J10 has no exactly located, lifecycle-tagged Spinner errors".into());
        }
        if targets.len() > 64 {
            return Err("J10 compiler inventory exceeds the 64-row native proof budget; no rows were silently omitted".into());
        }
        self.before = shell.compile_problem_state();
        self.preserved = records
            .into_iter()
            .filter(|entry| {
                !targets
                    .iter()
                    .any(|target| entry["first"].as_u64() == Some(target.first))
            })
            .collect();
        self.targets = targets;
        self.aliases = aliases;
        self.captured = true;
        Ok(())
    }

    pub fn assess(&mut self, projection: &Value) -> bool {
        let checks = self
            .targets
            .iter()
            .map(|target| verify::scope(target, projection))
            .collect::<Vec<_>>();
        let clean = verify::inventory(projection, &self.before);
        self.verified = self.captured && clean.is_ok() && checks.iter().all(Result::is_ok);
        self.assessment = json!({"verified":self.verified,"projectionRevision":projection["projectionRevision"],
            "inventory":clean.as_ref().err(),"scopes":self.targets.iter().zip(checks.iter()).map(|(target,check)|
                json!({"key":target.key,"failedOrder":target.order,"result":check.as_ref().ok(),"pendingReason":check.as_ref().err()})).collect::<Vec<_>>()});
        self.verified
    }

    /// Raw Store identity is immutable even when a repeated record gets a newer seq.
    pub fn refresh_history(
        &mut self,
        editor: &mut Editor,
        projection: &Value,
    ) -> ara_ui::Result<bool> {
        let records = errors(editor)?;
        let since = self
            .since
            .ok_or("J10 error inventory has no starting cursor")?;
        // A second compiler may finish after the runtime proof. Its errors need
        // their own completion and native checks; the first producer proves nothing for it.
        let mut added = false;
        for entry in &records {
            let diagnostic = &entry["diagnostic"];
            if entry["seq"].as_u64().is_none_or(|seq| seq <= since)
                || !diagnostic["message"]
                    .as_str()
                    .is_some_and(|m| m.contains(MARKER))
                || !diagnostic["location"]["file"]
                    .as_str()
                    .is_some_and(|file| self.aliases.contains(&normal(file)))
                || self
                    .targets
                    .iter()
                    .any(|t| Some(t.first) == entry["first"].as_u64())
            {
                continue;
            }
            self.targets.push(Target::parse(entry)?);
            added = true;
        }
        if self.targets.len() > 64 {
            return Err(
                "J10 late compiler inventory exceeds the native proof budget; no rows omitted"
                    .into(),
            );
        }
        if added {
            self.verified = false;
            self.problems = Value::Null;
            self.console.clear();
            self.console_index = 0;
            self.console_active = false;
            return Ok(false);
        }
        for target in &mut self.targets {
            let entry = retained(&records, &target.entry)?;
            target.seq = entry["seq"]
                .as_u64()
                .ok_or("J10 history record has no sequence")?;
        }
        let protected = self.before["protectedRuntimeFaultSeqs"]
            .as_array()
            .ok_or("J10 baseline protected inventory missing")?;
        let current = projection["protectedRuntimeFaultSeqs"]
            .as_array()
            .ok_or("J10 current protected inventory missing")?;
        let mut preservation = Vec::new();
        for old in &self.preserved {
            let latest = retained(&records, old)?;
            if protected.contains(&old["seq"]) && !current.contains(&latest["seq"]) {
                return Err("J10 fixed source retired an unrelated protected runtime fault".into());
            }
            let scoped = if let Ok(other) = Target::parse(old) {
                let active = projection["scopes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|scope| scope["key"] == other.key)
                    .and_then(|scope| scope["activeErrorSeqs"].as_array())
                    .is_some_and(|rows| rows.contains(&latest["seq"]));
                if !active && verify::scope(&other, projection).is_err() {
                    return Err("J10 unrelated compiler error disappeared without its own newer verified completion".into());
                }
                json!({"key":other.key,"active":active,"ownVerifiedCompletion":!active})
            } else {
                Value::Null
            };
            preservation.push(json!({"first":old["first"],"previousSequence":old["seq"],"sequence":latest["seq"],
                "historyRetained":true,"protectedRuntimeFault":protected.contains(&old["seq"]),"compileScope":scoped}));
        }
        self.preservation = json!(preservation);
        Ok(true)
    }

    pub fn absent_ids(&self) -> Vec<u64> {
        self.targets
            .iter()
            .flat_map(|t| [t.captured_seq, t.seq])
            .collect()
    }
    pub fn mark_problems(&mut self, projection: &Value) {
        self.problems = json!({"nativeProblemsFiltered":true,"absentExactSequences":self.absent_ids(),
            "projectionRevision":projection["projectionRevision"],"projection":projection});
    }
    pub fn sequence(&self) -> Option<u64> {
        self.targets.get(self.console_index).map(|t| t.seq)
    }
    pub fn record_console(&mut self, message: String, source: String) -> ara_ui::Result<()> {
        let target = self
            .targets
            .get(self.console_index)
            .ok_or("J10 Console has no captured target")?;
        let line = target.entry["diagnostic"]["location"]["line"]
            .as_u64()
            .ok_or("J10 target omitted its source line")?;
        if target.entry["diagnostic"]["message"].as_str() != Some(message.as_str())
            || !source.ends_with(&format!("Spinner.cs:{line}"))
        {
            return Err("J10 expanded Console row differs from the exact captured diagnostic or source line".into());
        }
        self.console.push(
            json!({"sequence":target.seq,"first":target.first,"key":target.key,
            "message":message,"source":source,"expandedViaNativePointer":true}),
        );
        self.console_index += 1;
        Ok(())
    }
    pub fn complete(&self) -> bool {
        self.verified
            && self.problems["nativeProblemsFiltered"] == true
            && !self.targets.is_empty()
            && self.console.len() == self.targets.len()
    }
    pub fn report(&self) -> Value {
        json!({"captured":self.captured,"since":self.since,"retirementVerified":self.complete(),
            "brokenProjection":self.before,"checks":self.assessment,"nativeProblems":self.problems,
            "nativeConsole":self.console,"preservedOtherErrorHistory":self.preserved,"preservationChecks":self.preservation,
            "targets":self.targets.iter().map(|t|json!({"first":t.first,"capturedSequence":t.captured_seq,
                "currentSequence":t.seq,"key":t.key,"failedOrder":t.order,"instance":t.instance,"source":t.source,"entry":t.entry})).collect::<Vec<_>>()})
    }
}

impl Target {
    fn parse(entry: &Value) -> Result<Self, String> {
        let diagnostic: ara_diag::Diagnostic = serde_json::from_value(entry["diagnostic"].clone())
            .map_err(|_| "J10 malformed compiler diagnostic")?;
        if !diagnostic.is_error() || !matches!(diagnostic.code.0,0..=905|907..=909|923..=925) {
            return Err("J10 located error is not an owned compiler diagnostic family".into());
        }
        let data = &entry["diagnostic"]["data"];
        let compile = &data["compile"];
        let play = data.get("play");
        let producer = token(&compile["producer"])?;
        let profile = token(&compile["profile"])?;
        if compile["schema"] != 1
            || compile["phase"] != "diagnostic"
            || !matches!(producer.as_str(), "project" | "warm" | "editor")
            || !matches!(profile.as_str(), "gameplay" | "editor")
            || !matches!(
                compile["channel"].as_str(),
                Some("compiler" | "analyzer" | "generator" | "dependency")
            )
        {
            return Err("J10 Spinner error has malformed or missing compiler provenance".into());
        }
        let (world, player, source, play_binding) = match play {
            None => (None, None, None, 0),
            Some(play) => (
                Some(token(&play["world"])?),
                Some(u32::try_from(number(&play["player"])?).map_err(|_| "J10 invalid player")?),
                Some(token(&play["source"])?),
                number(&play["binding"])?,
            ),
        };
        let attempt = number(&compile["attempt"])?;
        if attempt == 0 {
            return Err("J10 compiler error has no positive attempt".into());
        }
        let seq = number(&entry["seq"])?;
        Ok(Self {
            entry: entry.clone(),
            key: json!({"producer":producer,"profile":profile,"module":token(&data["module"])?,"world":world,"player":player}),
            order: (play_binding, number(&compile["binding"])?, attempt),
            instance: token(&compile["instance"])?,
            source,
            first: number(&entry["first"])?,
            seq,
            captured_seq: seq,
        })
    }
}
fn errors(editor: &mut Editor) -> ara_ui::Result<Vec<Value>> {
    let page = editor.action(
        "logs",
        json!({"severity":"error","group":false,"limit":1000}),
    )?;
    let records = page["entries"]
        .as_array()
        .ok_or("J10 error inventory omitted entries")?;
    if page["total"].as_u64() != Some(records.len() as u64) {
        return Err(
            "J10 error inventory is truncated or malformed; retirement cannot be proved".into(),
        );
    }
    Ok(records.clone())
}
fn retained<'a>(records: &'a [Value], old: &Value) -> ara_ui::Result<&'a Value> {
    records
        .iter()
        .find(|e| e["first"] == old["first"] && e["diagnostic"] == old["diagnostic"])
        .ok_or_else(|| "J10 lost or changed a historical diagnostic while fixing source".into())
}
fn token(value: &Value) -> Result<String, String> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .map(str::to_owned)
        .ok_or_else(|| "J10 missing provenance token".into())
}
fn number(value: &Value) -> Result<u64, String> {
    value
        .as_u64()
        .ok_or_else(|| "J10 missing numeric lifecycle counter".into())
}
fn order(value: &Value) -> Result<Order, String> {
    Ok((
        number(&value["playBinding"])?,
        number(&value["compileBinding"])?,
        number(&value["attempt"])?,
    ))
}
fn normal(path: &str) -> String {
    let path = path.replace('\\', "/");
    let path = path.strip_prefix("//?/").unwrap_or(&path);
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.into()
    }
}
