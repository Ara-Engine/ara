//! Trusted reload markers retire runtime faults, never compiler errors or Console history.
use super::*;
const MAX_MODULES: usize = 4096;
const MAX_INCARNATIONS: usize = 128;
type Modules = BTreeMap<String, u64>;
type Bindings = BTreeMap<u64, Modules>;
type Sources = BTreeMap<String, Bindings>;
type Players = BTreeMap<u32, Sources>;

#[derive(Clone, Copy)]
struct Incarnation<'a> {
    world: &'a str,
    player: u32,
    source: &'a str,
    binding: u64,
}
impl<'a> Incarnation<'a> {
    fn parse(data: &'a Value) -> Option<Self> {
        let play = data.get("play")?;
        let binding = play["binding"].as_u64()?;
        (binding > 0).then_some(Self {
            world: token(&play["world"])?,
            player: u32::try_from(play["player"].as_u64()?).ok()?,
            source: token(&play["source"])?,
            binding,
        })
    }
    fn value(self) -> Value {
        json!({"world":self.world,"player":self.player,"source":self.source,"binding":self.binding})
    }
}
fn token(value: &Value) -> Option<&str> {
    let text = value.as_str()?;
    (!text.is_empty() && text.len() <= 4096).then_some(text)
}

struct Marker<'a> {
    scope: Incarnation<'a>,
    ticket: u64,
    outcome: &'a str,
    modules: Vec<&'a str>,
}
impl<'a> Marker<'a> {
    fn parse(diagnostic: &'a ara_diag::Diagnostic) -> Option<Self> {
        if diagnostic.code != ara_script::codes::RECOVERY_RESULT {
            return None;
        }
        let data = diagnostic.data.as_deref()?;
        let recovery = data.get("recovery")?;
        if recovery["schema"].as_u64() != Some(1) {
            return None;
        }
        let outcome = recovery["outcome"].as_str()?;
        if !matches!(outcome, "reloaded" | "unchanged" | "failed" | "refused") {
            return None;
        }
        let raw = recovery["modules"].as_array()?;
        if raw.len() > MAX_MODULES {
            return None;
        }
        // Validate the entire declaration before accepting any module's retirement.
        let modules = raw.iter().map(token).collect::<Option<Vec<_>>>()?;
        Some(Self {
            scope: Incarnation::parse(data)?,
            ticket: recovery["ticket"].as_u64()?,
            outcome,
            modules,
        })
    }
}

/// A bounded retirement ledger also filters the ordinary 500-row tail fallback.
/// Nested borrowed-key lookups avoid allocating scope/module strings per row.
#[derive(Default)]
pub(super) struct Recovery {
    ledger: BTreeMap<String, Players>,
    incarnations: usize,
    modules: usize,
    invalid: u64,
    omitted: u64,
    gap_markers: u64,
    observed: u64,
    last: Option<Value>,
}
impl Recovery {
    fn modules(&self, scope: Incarnation<'_>) -> Option<&Modules> {
        self.ledger
            .get(scope.world)?
            .get(&scope.player)?
            .get(scope.source)?
            .get(&scope.binding)
    }

    /// Only broker-stamped data.play is trusted; subject string encodings are not an API.
    pub(super) fn observe(&mut self, entry: &Entry, gap: bool) -> bool {
        if entry.diagnostic.code != ara_script::codes::RECOVERY_RESULT {
            return false;
        }
        let Some(marker) = Marker::parse(&entry.diagnostic) else {
            self.invalid = self.invalid.saturating_add(1);
            return true;
        };
        self.observed = self.observed.saturating_add(1);
        let mut updated = 0usize;
        // A cursor gap may conceal a later occurrence collapsed into an older
        // row. Retain it until a later clean marker proves this exact scope.
        if marker.outcome == "reloaded" && gap {
            self.gap_markers = self.gap_markers.saturating_add(1);
        }
        if marker.outcome == "reloaded" && !gap {
            for module in &marker.modules {
                let known = self.modules(marker.scope);
                let new_incarnation = known.is_none();
                let new_module = known.is_none_or(|modules| !modules.contains_key(*module));
                if (new_incarnation && self.incarnations >= MAX_INCARNATIONS)
                    || (new_module && self.modules >= MAX_MODULES)
                {
                    self.omitted = self.omitted.saturating_add(1);
                    continue;
                }
                let modules = self
                    .ledger
                    .entry(marker.scope.world.to_owned())
                    .or_default()
                    .entry(marker.scope.player)
                    .or_default()
                    .entry(marker.scope.source.to_owned())
                    .or_default()
                    .entry(marker.scope.binding)
                    .or_default();
                let seq = modules.entry((*module).to_owned()).or_default();
                if entry.seq > *seq {
                    *seq = entry.seq;
                    updated += 1;
                }
                self.incarnations += usize::from(new_incarnation);
                self.modules += usize::from(new_module);
            }
        }
        self.last = Some(
            json!({"seq":entry.seq,"ticket":marker.ticket,"outcome":marker.outcome,"scope":marker.scope.value(),
            "modules":marker.modules.iter().take(32).collect::<Vec<_>>(),"moduleCount":marker.modules.len(),"modulesInReportOmitted":marker.modules.len().saturating_sub(32),
            "ledgerModulesUpdated":updated,"pageHadPossibleGap":gap}),
        );
        true
    }

    /// New occurrences move to a new Store seq, even when first/count deduplicate.
    /// Such a repeated fault is newer than the marker and becomes active again.
    pub(super) fn retired(&self, entry: &Entry) -> bool {
        if !entry.diagnostic.is_error() || !matches!(entry.diagnostic.code.0, 910..=912) {
            return false;
        }
        let Some(data) = entry.diagnostic.data.as_deref() else {
            return false;
        };
        let Some(scope) = Incarnation::parse(data) else {
            return false;
        };
        let Some(module) = token(&data["module"]) else {
            return false;
        };
        self.modules(scope)
            .and_then(|modules| modules.get(module))
            .is_some_and(|seq| entry.seq < *seq)
    }

    pub(super) fn notice(&self) -> Option<String> {
        (self.invalid>0 || self.omitted>0 || self.gap_markers>0).then(||format!(
            "C# reload history includes {} invalid markers, {} records outside the bounded cache and {} reload results on incomplete diagnostic pages. Those results cannot clear runtime faults; a later clean reload clears only its matching player incarnation and modules.",self.invalid,self.omitted,self.gap_markers))
    }
    pub(super) fn report(&self) -> Value {
        json!({"observedMarkers":self.observed,"incarnations":self.incarnations,"moduleRetirementRecords":self.modules,
            "maxIncarnations":MAX_INCARNATIONS,"maxModuleRetirementRecords":MAX_MODULES,
            "invalidMarkers":self.invalid,"omittedRecords":self.omitted,"gapMarkers":self.gap_markers,"lastMarker":self.last})
    }
}

#[cfg(test)]
mod tests;
