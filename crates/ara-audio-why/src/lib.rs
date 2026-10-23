//! "Why can't I hear X?" for `ara why`.
//!
//! The explainer combines what the documents say (the entity's `AudioSource`, its clip,
//! gain, cones, distance to the scene's `AudioListener`) with what the running audio
//! engine reported on the diagnostics bus (`ara_audio::diagnose::Audibility::explain`:
//! occluded, bus muted, voice-limited, not loaded, not playing). Register it once:
//! `ara_audio_why::register(host.why())`.
//!
//! Codes are resolved from the diagnostics catalog by name (`audio::OCCLUDED`), so the
//! host links this explainer without linking the audio engine.

use ara_diag::{Code, Fix, FixAction, Subject, catalog, codes, store::Filter};
use ara_why::{
    Cause, Collector, Entity, Explainer, Facts, Findings, Likelihood, Question, Report, Status,
    Target, ancestors,
    math::{Affine, length, show, sub},
};
use serde_json::{Value, json};

const WORDS: &[&str] = &[
    "hear", "audio", "sound", "silent", "quiet", "inaudib", "mute", "noise", "loud", "volume",
];

/// Runtime report codes the audio engine emits per source, with how likely each one
/// alone explains silence.
const RUNTIME: &[(&str, Likelihood)] = &[
    ("audio::NOT_LOADED", Likelihood::High),
    ("audio::BUS_MUTED", Likelihood::High),
    ("audio::VOICE_LIMITED", Likelihood::High),
    ("audio::NOT_PLAYING", Likelihood::High),
    ("audio::OUT_OF_RANGE", Likelihood::High),
    ("audio::SILENT_SOURCE", Likelihood::High),
    ("audio::OCCLUDED", Likelihood::Medium),
];

/// An audio code by catalog name; unknown names fall back to the unanswered code.
fn code(name: &str) -> Code {
    catalog::lookup(name).map_or(codes::WHY_UNANSWERED, |d| d.code)
}

fn set(title: String, entity: &str, path: &str, value: Value) -> Fix {
    Fix {
        title,
        action: FixAction::Command {
            command: "property.set".into(),
            params: json!({ "entity": entity, "path": path, "value": value }),
        },
    }
}

/// Explains inaudible sound sources.
pub struct AudioExplainer;

/// Registers the audio explainer with an `ara why` registry (e.g. `Host::why()`).
pub fn register(why: &mut ara_why::Why) {
    why.register(Box::new(AudioExplainer));
}

fn world(facts: &dyn Facts, e: &Entity) -> Affine {
    let mut chain: Vec<Entity> = ancestors(facts, e);
    chain.reverse();
    chain
        .iter()
        .chain(std::iter::once(e))
        .fold(Affine::IDENTITY, |w, x| {
            w.then(&Affine::from_transform(x.components.get("Transform")))
        })
}

fn resolve(q: &Question, facts: &dyn Facts) -> Result<Entity, Vec<ara_diag::Diagnostic>> {
    if let Some(selector) = q.entities.first() {
        return facts.entity(selector, q.doc.as_deref());
    }
    let mut last = Vec::new();
    for name in q.name_candidates(WORDS) {
        match facts.entity(&format!("name:{name}"), q.doc.as_deref()) {
            Ok(e) => return Ok(e),
            Err(d) => last = d,
        }
    }
    Err(last)
}

impl Explainer for AudioExplainer {
    fn name(&self) -> &'static str {
        "audio"
    }

    fn about(&self) -> &'static str {
        "Why a sound can't be heard: source component, clip, gain and cones, distance to the listener, and runtime reports (occlusion, muted buses, voice limits, not playing)."
    }

    fn examples(&self) -> &'static [&'static str] {
        &["why can't I hear name:Radio", "e_8b21 is silent"]
    }

    fn matches(&self, q: &Question) -> u32 {
        match (!q.entities.is_empty(), q.about(WORDS)) {
            (true, true) => 95,
            (false, true) if !q.name_candidates(WORDS).is_empty() => 75,
            _ => 0,
        }
    }

    fn explain(
        &self,
        q: &Question,
        facts: &dyn Facts,
        extra: &[&dyn Collector],
    ) -> Result<Report, Vec<ara_diag::Diagnostic>> {
        let e = resolve(q, facts)?;
        let mut f = Findings::default();
        let subject = Subject::entity(e.address.clone());
        let Some((_, source)) = e.component(&["AudioSource"], None) else {
            f.check("AudioSource", Status::Fail, "missing");
            f.cause(
                Cause::new(
                    code("audio::NOT_LOADED"),
                    Likelihood::High,
                    format!("{} has no `AudioSource`", e.label()),
                )
                .subject(subject.clone())
                .fix(Fix {
                    title: "Add an AudioSource".into(),
                    action: FixAction::Command {
                        command: "component.add".into(),
                        params: json!({ "entity": e.address, "component": "AudioSource" }),
                    },
                }),
            );
            let answer = Report::verdict(&f, "");
            return Ok(Report::new(
                &q.text,
                "audio",
                Some(format!("why can't I hear {}", e.label())),
                answer,
                f,
            ));
        };
        f.check("AudioSource", Status::Ok, "");
        let number = |field: &str, default: f64| {
            source.get(field).and_then(Value::as_f64).unwrap_or(default)
        };
        match source.get("clip").and_then(Value::as_str) {
            None => {
                f.check("clip", Status::Fail, "unset");
                f.cause(
                    Cause::new(
                        code("audio::NOT_LOADED"),
                        Likelihood::High,
                        "`AudioSource.clip` is not set",
                    )
                    .subject(subject.clone())
                    .advice("Set `AudioSource.clip` to an audio asset (`@sounds/x.wav#au_...`)."),
                );
            }
            Some(clip) => {
                let asset = facts.asset(clip);
                if asset.resolves() {
                    f.check("clip", Status::Ok, asset.canonical());
                } else {
                    f.check("clip", Status::Fail, clip);
                    let mut cause = Cause::new(
                        code("audio::NOT_LOADED"),
                        Likelihood::High,
                        format!("`AudioSource.clip` `{clip}` does not resolve"),
                    )
                    .subject(subject.clone())
                    .subject(Subject::asset(clip));
                    if let Some(closest) = asset.closest.first() {
                        cause = cause.fix(set(
                            format!("Use `{closest}`"),
                            &e.address,
                            "AudioSource.clip",
                            json!(closest),
                        ));
                    }
                    f.cause(cause);
                }
            }
        }
        let gain = number("gain", 1.0);
        if gain <= 0.0 {
            f.check("gain", Status::Fail, show(gain));
            f.cause(
                Cause::new(
                    code("audio::SILENT_SOURCE"),
                    Likelihood::High,
                    "`AudioSource.gain` is 0",
                )
                .subject(subject.clone())
                .fix(set(
                    "Set `AudioSource.gain` to 1".into(),
                    &e.address,
                    "AudioSource.gain",
                    json!(1),
                )),
            );
        } else {
            f.check("gain", Status::Ok, show(gain));
        }
        let spatial = source
            .get("spatial")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let listeners = facts.with_component(&e.doc, "AudioListener");
        match (spatial, listeners.first()) {
            (false, _) => f.check("distance", Status::Skip, "non-spatial"),
            (true, None) => {
                f.check("listener", Status::Fail, "none in the scene");
                f.cause(
                    Cause::new(
                        code("audio::OUT_OF_RANGE"),
                        Likelihood::Medium,
                        format!("`{}` has no `AudioListener`", e.doc),
                    )
                    .advice("Add an `AudioListener` to the player camera entity."),
                );
            }
            (true, Some(listener)) => {
                let here = world(facts, &e).t;
                let ear = world(facts, listener).t;
                let distance = length(sub(here, ear));
                let max = number("maxDistance", 100.0);
                if distance >= max {
                    f.check(
                        "distance",
                        Status::Fail,
                        format!("{} m ≥ {} m", show(distance), show(max)),
                    );
                    let reach = (distance * 1.25).ceil();
                    f.cause(
                        Cause::new(
                            code("audio::OUT_OF_RANGE"),
                            Likelihood::High,
                            format!(
                                "the listener {} is {} m away, beyond `maxDistance` {} m",
                                listener.label(),
                                show(distance),
                                show(max)
                            ),
                        )
                        .evidence(format!(
                            "{} AudioSource.maxDistance = {}",
                            e.address,
                            show(max)
                        ))
                        .subject(subject.clone())
                        .fix(set(
                            format!("Set `AudioSource.maxDistance` to {reach}"),
                            &e.address,
                            "AudioSource.maxDistance",
                            json!(reach),
                        )),
                    );
                } else {
                    f.check(
                        "distance",
                        Status::Ok,
                        format!("{} m of {} m", show(distance), show(max)),
                    );
                }
            }
        }
        let filter = Filter {
            codes: RUNTIME.iter().map(|(name, _)| code(name)).collect(),
            subjects: vec![subject.clone()],
            ..Filter::default()
        };
        let reports = facts.diagnostics(&filter);
        if reports.is_empty() {
            f.check(
                "runtime",
                Status::Unknown,
                "no audio reports on the bus; play the scene and ask again",
            );
        } else {
            f.check(
                "runtime",
                Status::Ok,
                format!("{} report(s)", reports.len()),
            );
        }
        // Newest report per code wins; documents already covered range and gain.
        let mut seen = Vec::new();
        for record in reports.iter().rev() {
            let d = &record.diagnostic;
            if seen.contains(&d.code) || f.causes.iter().any(|c| c.code == d.code) {
                continue;
            }
            seen.push(d.code);
            let likelihood = RUNTIME
                .iter()
                .find(|(name, _)| code(name) == d.code)
                .map_or(Likelihood::Low, |(_, l)| *l);
            let mut cause = Cause::new(d.code, likelihood, d.message.clone())
                .evidence(format!("{} {} (runtime)", d.severity, d.code))
                .subject(subject.clone());
            for fix in &d.fixes {
                cause = cause.fix(fix.clone());
            }
            f.cause(cause);
        }
        f.collect(extra, &Target::Entity(&e), facts);
        let answer = Report::verdict(
            &f,
            "nothing in the documents or runtime reports explains it; check the voice in `capture_audio`",
        );
        Ok(Report::new(
            &q.text,
            "audio",
            Some(format!("why can't I hear {} in {}", e.label(), e.doc)),
            answer,
            f,
        ))
    }
}
