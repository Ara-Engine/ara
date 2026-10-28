//! Binding/attempt ordering and complete-coverage retirement, independent of display filters.
use super::*;

struct Binding {
    order: (u64, u64),
    instance: String,
    source: Option<String>,
}
struct Attempt {
    order: Order,
    started: bool,
    tainted: bool,
    phase: &'static str,
    rows: BTreeMap<u64, (u64, bool)>,
    completion: Option<metadata::Completion>,
}

#[derive(Default)]
pub(super) struct Scope {
    binding: Option<Binding>,
    attempt: Option<Attempt>,
    pub covered: Option<Order>,
    pub incomplete: bool,
    unaccepted: Option<Order>,
    unknown_code: bool,
    covered_result: Option<&'static str>,
    omitted_errors: Option<Order>,
    omitted_warnings: bool,
}

impl Scope {
    pub fn gap(&mut self) {
        self.incomplete = true;
        if let Some(attempt) = &mut self.attempt {
            attempt.tainted = true;
        }
    }

    pub fn observe(
        &mut self,
        view: &mut CompilationView,
        metadata: &Metadata,
        entry: Entry,
        gap: bool,
    ) {
        let pair = metadata.order.binding();
        if metadata.phase == Phase::Bound {
            match &self.binding {
                Some(binding) if pair < binding.order => return,
                Some(binding) if pair == binding.order => {
                    if binding.instance != metadata.instance || binding.source != metadata.source {
                        self.gap();
                        self.unaccepted = Some(metadata.order);
                    }
                    return;
                }
                _ => {}
            }
            self.binding = Some(Binding {
                order: pair,
                instance: metadata.instance.clone(),
                source: metadata.source.clone(),
            });
            self.attempt = None;
            self.incomplete = true;
            if self.unaccepted.is_some_and(|order| order.binding() <= pair) {
                self.unaccepted = None;
            }
            return;
        }
        let accepted = self.binding.as_ref().is_some_and(|binding| {
            binding.order == pair
                && binding.instance == metadata.instance
                && binding.source == metadata.source
        });
        if !accepted {
            if self
                .binding
                .as_ref()
                .is_some_and(|binding| pair < binding.order)
            {
                return;
            }
            self.unaccepted = Some(
                self.unaccepted
                    .map_or(metadata.order, |old| old.max(metadata.order)),
            );
            self.gap();
            if metadata.phase == Phase::Diagnostic && compile_kind(&entry.diagnostic) {
                let error = entry.diagnostic.is_error();
                if !view.cache(metadata, entry) {
                    if error {
                        self.omitted_errors = Some(metadata.order);
                    } else {
                        self.omitted_warnings = true;
                    }
                }
            }
            return;
        }
        if self
            .attempt
            .as_ref()
            .is_some_and(|attempt| metadata.order < attempt.order)
        {
            return;
        }
        if self
            .attempt
            .as_ref()
            .is_none_or(|attempt| metadata.order > attempt.order)
        {
            self.attempt = Some(Attempt {
                order: metadata.order,
                started: false,
                tainted: gap,
                phase: "pending",
                rows: BTreeMap::new(),
                completion: None,
            });
            self.incomplete = true;
        }
        let Some(attempt) = &mut self.attempt else {
            return;
        };
        attempt.tainted |= gap;
        match metadata.phase {
            Phase::Started => {
                if attempt.completion.is_none() {
                    attempt.started = true;
                    attempt.phase = "started";
                }
            }
            Phase::Diagnostic => {
                if !compile_kind(&entry.diagnostic) {
                    return;
                }
                if !known_compile_kind(&entry.diagnostic) {
                    self.unknown_code = true;
                    attempt.tainted = true;
                    self.incomplete = true;
                }
                if !attempt.started || attempt.completion.is_some() {
                    attempt.tainted = true;
                    self.incomplete = true;
                }
                if attempt.rows.len() < MAX_ROWS || attempt.rows.contains_key(&entry.first) {
                    attempt
                        .rows
                        .insert(entry.first, (entry.count, entry.diagnostic.is_error()));
                } else {
                    attempt.tainted = true;
                }
                let error = entry.diagnostic.is_error();
                if !view.cache(metadata, entry) {
                    if error {
                        self.omitted_errors = Some(metadata.order);
                        attempt.tainted = true;
                    } else {
                        self.omitted_warnings = true;
                    }
                    self.incomplete = true;
                }
            }
            Phase::Completed => {
                // A replay cannot rewrite the terminal facts already associated
                // with this attempt or turn a cancelled attempt into a success.
                if attempt.completion.is_some() {
                    return;
                }
                let Some(completion) = metadata.completion.as_ref() else {
                    attempt.tainted = true;
                    attempt.phase = "invalid-completion";
                    self.incomplete = true;
                    return;
                };
                let observed: u64 = attempt.rows.values().map(|(count, _)| count).sum();
                let errors: u64 = attempt
                    .rows
                    .values()
                    .filter(|(_, error)| *error)
                    .map(|(count, _)| count)
                    .sum();
                let complete = attempt.started
                    && !attempt.tainted
                    && !self.unknown_code
                    && self.unaccepted.is_none()
                    && completion.verified
                    && !completion.truncated
                    && completion.result != "cancelled"
                    && completion.total == completion.emitted
                    && completion.emitted == observed
                    && completion.errors == errors;
                if complete {
                    view.retire_errors(&metadata.key, metadata.order);
                    self.covered = Some(metadata.order);
                    self.covered_result = Some(if completion.result == "success" {
                        "success"
                    } else {
                        "errors"
                    });
                    if self
                        .omitted_errors
                        .is_some_and(|order| order < metadata.order)
                    {
                        self.omitted_errors = None;
                    }
                }
                self.incomplete = !complete;
                attempt.phase = if completion.result == "cancelled" {
                    "cancelled"
                } else if completion.result == "errors" {
                    "errors"
                } else if complete {
                    "success"
                } else {
                    "unverified-success"
                };
                attempt.completion = metadata.completion.clone();
            }
            Phase::Bound => {}
        }
    }

    pub fn pending(&self) -> bool {
        self.binding.is_some()
            && self
                .attempt
                .as_ref()
                .is_none_or(|attempt| attempt.completion.is_none())
    }
    pub fn omitted(&self) -> bool {
        self.omitted_errors.is_some() || self.omitted_warnings
    }
    pub fn order(&self) -> Option<Order> {
        self.attempt.as_ref().map(|attempt| attempt.order)
    }
    pub fn phase(&self) -> &'static str {
        self.attempt
            .as_ref()
            .map_or("bound", |attempt| attempt.phase)
    }
    pub fn bound_report(&self) -> Value {
        self.binding.as_ref().map_or(Value::Null,|binding|json!({"playBinding":binding.order.0,"compileBinding":binding.order.1,"instance":binding.instance,"source":binding.source}))
    }
    pub fn verified_report(&self) -> Value {
        self.covered.map_or(Value::Null,|order|json!({"order":order,"result":self.covered_result,"validation":"current-module-inputs"}))
    }
    pub fn attempt_report(&self) -> Value {
        self.attempt.as_ref().map_or(Value::Null,|attempt|json!({"started":attempt.started,"tainted":attempt.tainted,"unknownCodeFamily":self.unknown_code,
            "observed":attempt.rows.values().map(|(count,_)|count).sum::<u64>(),
            "observedErrors":attempt.rows.values().filter(|(_,error)|*error).map(|(count,_)|count).sum::<u64>(),
            "result":attempt.completion.as_ref().map(|completion|&completion.result),
            "total":attempt.completion.as_ref().map(|completion|completion.total),"emitted":attempt.completion.as_ref().map(|completion|completion.emitted),
            "truncated":attempt.completion.as_ref().map(|completion|completion.truncated),"verified":attempt.completion.as_ref().map(|completion|completion.verified)}))
    }
}
