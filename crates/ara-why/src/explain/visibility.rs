//! "Why is `name:Crate` not visible?" — walks from the entity through everything
//! that has to be right for it to show up, and reports what is not.
//!
//! Checks, in order: the entity resolves; it, its ancestors and its renderer are
//! enabled; it has a renderer; its world transform (own and inherited) is finite
//! and not flattened; its mesh resolves, has a source and imported; its material
//! resolves, compiles, draws meshes and is not fully transparent; its tint alpha;
//! some camera's culling mask includes its layer, it is within the camera's far
//! plane and its own draw distance, and in front of a camera; its world cell is
//! resident; the renderer included it in a pass. Runtime facts (NaN transforms from
//! physics, import failures, streaming, pass exclusion) come from the bus.
//!
//! Field conventions, by name (project types define the components): renderers are
//! `MeshRenderer`, `SkinnedMeshRenderer`, `SpriteRenderer`, `Sprite` or any
//! `*Renderer` with `mesh`/`sprite`, `material`/`materials`, `tint`/`color`,
//! `layer`, `enabled`/`visible`, `maxDistance`/`drawDistance`/`cullDistance`.
//! Cameras are `Camera` with `cullingMask` (layer names, layer indices or an
//! integer bitmask; absent = everything), `far` and `enabled`.

use ara_diag::store::{Filter, Record};
use ara_diag::{Code, Diagnostic, Severity, Subject, codes};
use serde_json::{Value, json};

use super::material;
use super::{line, set};
use crate::facts::ancestors;
use crate::math::{self, Affine, Vec3, show, show3};
use crate::{
    Asset, Cause, Collector, Entity, Explainer, Facts, Findings, Likelihood, Question, Report,
    Status, Target,
};

/// Renderer components, by exact name; any other `*Renderer` also counts.
const RENDERERS: &[&str] = &["MeshRenderer", "SkinnedMeshRenderer", "SpriteRenderer", "Sprite"];
/// Topic words (prefixes) of visibility questions.
const WORDS: &[&str] = &[
    "visib", "invisib", "see", "seen", "show", "render", "draw", "drawn", "appear", "hidden",
    "hide", "disappear", "gone", "display", "vanish",
];

/// The visibility explainer.
#[derive(Default)]
pub struct Visibility;

/// Everything the checks share.
struct Ctx<'a> {
    facts: &'a dyn Facts,
    e: &'a Entity,
    /// Ancestors, nearest first.
    chain: Vec<Entity>,
    renderer: Option<(String, Value)>,
}

impl Ctx<'_> {
    fn renderer_field(&self, names: &[&str]) -> Option<(&'static str, &Value)> {
        let (_, r) = self.renderer.as_ref()?;
        names.iter().find_map(|n| {
            let v = r.get(*n)?;
            (!v.is_null()).then(|| (static_name(n), v))
        })
    }

    fn renderer_path(&self, field: &str) -> String {
        let name = self.renderer.as_ref().map_or("MeshRenderer", |(n, _)| n.as_str());
        format!("{name}.{field}")
    }
}

/// Field names come from fixed lists; this hands back the `'static` copy.
fn static_name(n: &str) -> &'static str {
    const ALL: &[&str] = &[
        "mesh", "sprite", "material", "materials", "tint", "color", "layer", "enabled",
        "visible", "maxDistance", "drawDistance", "cullDistance",
    ];
    ALL.iter().find(|a| **a == n).copied().unwrap_or("field")
}

impl Explainer for Visibility {
    fn name(&self) -> &'static str {
        "visibility"
    }

    fn about(&self) -> &'static str {
        "Why an entity does not show up: enabled flags, renderer, transform, mesh, material, layers and cameras, draw distance, streaming, passes."
    }

    fn examples(&self) -> &'static [&'static str] {
        &["name:Crate not visible", "e_8b21 invisible"]
    }

    fn matches(&self, q: &Question) -> u32 {
        let topic = q.about(WORDS);
        match (!q.entities.is_empty(), topic) {
            (true, true) => 90,
            (true, false) if q.other_words(WORDS).is_empty() && q.codes.is_empty() => 40,
            (false, true) if !q.names.is_empty() => 70,
            _ => 0,
        }
    }

    fn explain(
        &self,
        q: &Question,
        facts: &dyn Facts,
        extra: &[&dyn Collector],
    ) -> Result<Report, Vec<Diagnostic>> {
        let e = resolve(q, facts)?;
        let renderer = e
            .component(RENDERERS, Some("Renderer"))
            .map(|(n, v)| (n.to_owned(), v.clone()));
        let cx = Ctx {
            facts,
            e: &e,
            chain: ancestors(facts, &e),
            renderer,
        };
        let mut f = Findings::default();
        enabled(&cx, &mut f);
        renderer_present(&cx, &mut f);
        let world = transform(&cx, &mut f);
        let mesh = mesh(&cx, &mut f);
        let material = materials(&cx, &mut f);
        tint(&cx, &mut f);
        cameras(&cx, world, &mut f);
        streaming(&cx, &mut f);
        passes(&cx, &mut f);
        f.collect(extra, &Target::Entity(&e), facts);
        related(&cx, &[mesh, material].concat(), &mut f);
        let answer = Report::verdict(
            &f,
            "nothing in the documents or on the bus explains it; checks marked ? need the running renderer",
        );
        let subject = format!("why is {} in {} not visible", e.label(), e.doc);
        Ok(Report::new(&q.text, "visibility", Some(subject), answer, f))
    }
}

/// The entity the question is about: a selector, else a capitalized word that
/// names exactly one entity.
pub(crate) fn resolve(q: &Question, facts: &dyn Facts) -> Result<Entity, Vec<Diagnostic>> {
    if let Some(sel) = q.entities.first() {
        return facts.entity(sel, q.doc.as_deref());
    }
    let mut last = None;
    for n in &q.names {
        match facts.entity(&format!("name:{n}"), q.doc.as_deref()) {
            Ok(e) => return Ok(e),
            Err(d) => last = Some(d),
        }
    }
    Err(last.unwrap_or_else(|| {
        vec![Diagnostic::error(
            codes::WHY_UNANSWERED,
            "name the entity: `name:Crate`, an ID (`e_8b21`) or `@levels/a.scene#e_8b21`",
        )]
    }))
}

/// Records about any of `subjects` at warning or above.
fn bus(facts: &dyn Facts, codes: &[Code], subjects: Vec<Subject>) -> Vec<Record> {
    facts.diagnostics(&Filter {
        codes: codes.to_vec(),
        subjects,
        severity: Some(Severity::Warning),
        ..Filter::default()
    })
}

/// A cause backed by a bus record: its message, location, tick and causal chain.
fn from_record(code: Code, likelihood: Likelihood, title: String, r: &Record, facts: &dyn Facts) -> Cause {
    let mut c = Cause::new(code, likelihood, title).evidence(line(r));
    for parent in facts.causes(r) {
        c = c.evidence(format!("caused by: {}", line(&parent)));
        c.fixes.extend(super::runnable(&parent.diagnostic.fixes));
    }
    c.fixes.extend(super::runnable(&r.diagnostic.fixes));
    c
}

/// A flag that turns the entity (or a subtree) off: `(path, value that fixes it)`.
fn off_flags(e: &Entity, renderer: Option<&str>) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let off = |k: &str, v: &Value| match (k, v) {
        ("enabled" | "active" | "visible", Value::Bool(false)) => Some(json!(true)),
        ("hidden", Value::Bool(true)) => Some(json!(false)),
        _ => None,
    };
    for (k, v) in &e.flags {
        if let Some(fix) = off(k, v) {
            out.push((k.clone(), fix));
        }
    }
    for (name, value) in &e.components {
        let relevant = Some(name.as_str()) == renderer
            || name.contains("Visib")
            || name.contains("Hidden")
            || name == "Active";
        let Some(fields) = value.as_object().filter(|_| relevant) else {
            continue;
        };
        for (k, v) in fields {
            if let Some(fix) = off(k, v) {
                out.push((format!("{name}.{k}"), fix));
            }
        }
    }
    out
}

fn enabled(cx: &Ctx<'_>, f: &mut Findings) {
    let mut found = false;
    let renderer = cx.renderer.as_ref().map(|(n, _)| n.as_str());
    let own = std::iter::once((cx.e, renderer));
    let parents = cx.chain.iter().map(|a| (a, None));
    for (who, r) in own.chain(parents) {
        for (path, fix) in off_flags(who, r) {
            found = true;
            let whose = if who.address == cx.e.address {
                "It".to_owned()
            } else {
                format!("Its ancestor {}", who.label())
            };
            f.cause(
                Cause::new(codes::RENDER_DISABLED, Likelihood::High, format!("{whose} is switched off: `{path}` is {}", flip(&fix)))
                    .evidence(format!("{} {path} = {}", who.address, flip(&fix)))
                    .subject(Subject::entity(who.address.clone()))
                    .fix(set(format!("Set `{path}` to {fix}"), &who.address, &path, fix)),
            );
        }
    }
    f.check("enabled", if found { Status::Fail } else { Status::Ok }, "");
}

fn flip(v: &Value) -> Value {
    match v {
        Value::Bool(b) => Value::Bool(!b),
        other => other.clone(),
    }
}

fn renderer_present(cx: &Ctx<'_>, f: &mut Findings) {
    match &cx.renderer {
        Some((name, _)) => f.check("renderer", Status::Ok, name.clone()),
        None => {
            f.check("renderer", Status::Fail, "none");
            let mut c = Cause::new(
                codes::RENDER_NO_RENDERER,
                Likelihood::High,
                format!("{} has no renderer component, so it has nothing to draw", cx.e.label()),
            )
            .evidence(format!(
                "components: {}",
                cx.e.components.keys().cloned().collect::<Vec<_>>().join(", ")
            ))
            .subject(Subject::entity(cx.e.address.clone()));
            if cx.facts.has_component_type("MeshRenderer") {
                c = c.fix(ara_diag::Fix {
                    title: "Add a `MeshRenderer`".into(),
                    action: ara_diag::FixAction::Command {
                        command: "component.add".into(),
                        params: json!({ "entity": cx.e.address, "component": "MeshRenderer" }),
                    },
                })
                .advice("then set `MeshRenderer.mesh` to a mesh asset");
            }
            f.cause(c);
        }
    }
}

/// Checks the transform chain; returns the entity's world transform.
fn transform(cx: &Ctx<'_>, f: &mut Findings) -> Affine {
    let mut ok = true;
    // Root first.
    let mut lineage: Vec<&Entity> = cx.chain.iter().rev().collect();
    lineage.push(cx.e);
    let mut world = Affine::IDENTITY;
    for who in &lineage {
        let t = who.components.get("Transform");
        let whose = if who.address == cx.e.address {
            format!("{}'s", cx.e.label())
        } else {
            format!("Its ancestor {}'s", who.label())
        };
        for (field, default) in [("position", 0.0), ("rotation", 0.0), ("scale", 1.0)] {
            let Some(v) = math::vec3(t.and_then(|t| t.get(field))) else {
                continue;
            };
            if v.iter().any(|x| !x.is_finite()) {
                ok = false;
                let fixed: Vec3 = v.map(|x| if x.is_finite() { x } else { default });
                f.cause(
                    Cause::new(
                        codes::RENDER_NON_FINITE_TRANSFORM,
                        Likelihood::High,
                        format!("{whose} `Transform.{field}` is not a finite number"),
                    )
                    .evidence(format!("{} Transform.{field} = {}", who.address, show3(v)))
                    .subject(Subject::entity(who.address.clone()))
                    .fix(set(
                        format!("Reset `Transform.{field}`"),
                        &who.address,
                        &format!("Transform.{field}"),
                        json!(fixed),
                    )),
                );
            }
            if field == "scale" && v.iter().any(|x| *x == 0.0) {
                ok = false;
                let axes: Vec<&str> = ["x", "y", "z"]
                    .iter()
                    .zip(v)
                    .filter(|(_, x)| *x == 0.0)
                    .map(|(a, _)| *a)
                    .collect();
                let fixed: Vec3 = v.map(|x| if x == 0.0 { 1.0 } else { x });
                let note = if who.address == cx.e.address {
                    String::new()
                } else {
                    format!(", which flattens everything under it, including {}", cx.e.label())
                };
                f.cause(
                    Cause::new(
                        codes::RENDER_DEGENERATE_TRANSFORM,
                        Likelihood::High,
                        format!("{whose} `Transform.scale` is 0 on {}{note}", axes.join(", ")),
                    )
                    .evidence(format!("{} Transform.scale = {}", who.address, show3(v)))
                    .subject(Subject::entity(who.address.clone()))
                    .fix(set(
                        "Give the scale its size back",
                        &who.address,
                        "Transform.scale",
                        json!(fixed),
                    )),
                );
            }
        }
        world = world.then(&Affine::from_transform(t));
    }
    // NaNs written at runtime (physics, scripts) arrive on the bus.
    let mut subjects: Vec<Subject> = lineage.iter().map(|e| Subject::entity(e.address.clone())).collect();
    subjects.dedup();
    for r in bus(cx.facts, &[codes::RENDER_NON_FINITE_TRANSFORM], subjects) {
        ok = false;
        let at = r.tick.map(|t| format!(" at tick {t}")).unwrap_or_default();
        let mut c = from_record(
            codes::RENDER_NON_FINITE_TRANSFORM,
            Likelihood::High,
            format!("The running game reported a non-finite transform{at}"),
            &r,
            cx.facts,
        );
        c = c
            .fix(set(
                "Reset the position",
                &cx.e.address,
                "Transform.position",
                json!(math::vec3(cx.e.get("Transform", "position")).filter(|v| v.iter().all(|x| x.is_finite())).unwrap_or([0.0; 3])),
            ))
            .advice("fix what wrote the NaN (see `caused by`), or it comes back on the next play");
        f.cause(c);
    }
    let note = if world.is_finite() {
        format!("world position {}", show3(world.t))
    } else {
        "not finite".to_owned()
    };
    f.check("transform", if ok { Status::Ok } else { Status::Fail }, note);
    world
}

/// Checks the mesh; returns the subjects it involves.
fn mesh(cx: &Ctx<'_>, f: &mut Findings) -> Vec<Subject> {
    if cx.renderer.is_none() {
        f.check("mesh", Status::Skip, "");
        return Vec::new();
    }
    let Some((field, value)) = cx.renderer_field(&["mesh", "sprite"]) else {
        f.check("mesh", Status::Fail, "none");
        f.cause(
            Cause::new(
                codes::RENDER_NO_MESH,
                Likelihood::High,
                format!("`{}` names no mesh", cx.renderer_path("mesh")),
            )
            .subject(Subject::entity(cx.e.address.clone()))
            .advice(format!("set `{}` to a mesh asset (`ara_query` the meshes folder)", cx.renderer_path("mesh"))),
        );
        return Vec::new();
    };
    let reference = value.as_str().unwrap_or_default();
    let asset = cx.facts.asset(reference);
    let path = cx.renderer_path(field);
    let subject = Subject::asset(asset.canonical());
    if !asset.resolves() {
        f.check("mesh", Status::Fail, "does not resolve");
        f.cause(unresolved(cx, &asset, codes::RENDER_NO_MESH, &path, "mesh"));
        return vec![subject];
    }
    let mut ok = true;
    let records = bus(
        cx.facts,
        &[codes::ASSET_IMPORT_FAILED, codes::ASSET_SOURCE_MISSING, codes::ASSET_WORKER_FAILED],
        vec![subject.clone()],
    );
    for r in &records {
        ok = false;
        f.cause(from_record(
            codes::RENDER_NO_MESH,
            Likelihood::High,
            format!("Its mesh `{}` failed to import", asset.canonical()),
            r,
            cx.facts,
        ));
    }
    if records.is_empty() && asset.source_exists == Some(false) {
        f.cause(
            Cause::new(
                codes::ASSET_SOURCE_MISSING,
                Likelihood::Low,
                format!(
                    "The mesh source `{}` is missing next to its sidecar; without a cached import there is nothing to draw",
                    asset.path.as_deref().unwrap_or(reference)
                ),
            )
            .subject(subject.clone())
            .advice("restore the source file (`git lfs pull`) or point the renderer at another mesh"),
        );
    }
    f.check(
        "mesh",
        if ok { Status::Ok } else { Status::Fail },
        asset.path.clone().unwrap_or_default(),
    );
    vec![subject]
}

/// A cause for a reference that does not resolve, with a fix per closest match.
fn unresolved(cx: &Ctx<'_>, asset: &Asset, code: Code, path: &str, what: &str) -> Cause {
    let mut c = Cause::new(
        code,
        Likelihood::High,
        format!("Its {what} `{}` does not resolve to any asset", asset.reference),
    )
    .evidence(match &asset.problem {
        Some(d) => format!("{} {}", d.code, d.message),
        None => format!("{} {path} = {}", cx.e.address, asset.reference),
    })
    .subject(Subject::entity(cx.e.address.clone()));
    for candidate in &asset.closest {
        c = c.fix(set(
            format!("Use `{candidate}`"),
            &cx.e.address,
            path,
            json!(candidate),
        ));
    }
    if asset.closest.is_empty() {
        c = c.advice(format!("set `{path}` to an existing {what}"));
    }
    c
}

/// Checks the materials; returns the subjects they involve.
fn materials(cx: &Ctx<'_>, f: &mut Findings) -> Vec<Subject> {
    if cx.renderer.is_none() {
        f.check("material", Status::Skip, "");
        return Vec::new();
    }
    let Some((field, value)) = cx.renderer_field(&["material", "materials"]) else {
        f.check("material", Status::Skip, "default");
        return Vec::new();
    };
    let refs: Vec<(String, &str)> = match value {
        Value::String(s) => vec![(cx.renderer_path(field), s.as_str())],
        Value::Array(a) => a
            .iter()
            .enumerate()
            .filter_map(|(i, v)| Some((format!("{}[#{i}]", cx.renderer_path(field)), v.as_str()?)))
            .collect(),
        _ => Vec::new(),
    };
    let mut ok = true;
    let mut subjects = Vec::new();
    for (path, reference) in refs.iter().filter(|(_, r)| !r.is_empty()) {
        let asset = cx.facts.asset(reference);
        let subject = Subject::asset(asset.canonical());
        subjects.push(subject.clone());
        if !asset.resolves() {
            ok = false;
            f.cause(unresolved(cx, &asset, codes::RENDER_MATERIAL, path, "material"));
            continue;
        }
        let name = asset.path.clone().unwrap_or_else(|| (*reference).to_owned());
        if let Some(status) = material::inspect(cx.facts, &asset) {
            if !status.errors.is_empty() {
                ok = false;
                let mut c = Cause::new(
                    codes::RENDER_MATERIAL,
                    Likelihood::High,
                    format!("Its material `{name}` does not compile, so there is no shader to draw it with"),
                )
                .subject(subject.clone());
                for d in status.errors.iter().take(3) {
                    c = c.evidence(format!("{} {} ({})", d.code, d.message, d.location));
                    c.fixes.extend(d.fixes.iter().filter(|x| matches!(x.action, ara_diag::FixAction::Command { .. } | ara_diag::FixAction::Cli { .. })).cloned());
                }
                if let Some(at) = status.errors.first().map(|d| d.location.to_string()) {
                    c = c.advice(format!("fix the material source at {at}, then ask again"));
                }
                f.cause(c);
            } else if !status.draws_meshes() {
                ok = false;
                f.cause(
                    Cause::new(
                        codes::RENDER_MATERIAL,
                        Likelihood::Medium,
                        format!("Its material `{name}` is a `{}` material, which does not draw scene meshes", status.domain.map_or("non-surface", |d| d.name())),
                    )
                    .subject(subject.clone())
                    .advice("use a `surface` or `unlit` material for meshes"),
                );
            }
            for (output, from_param) in &status.zero {
                ok = false;
                let how = if *from_param { "a parameter that defaults to 0" } else { "a constant 0" };
                f.cause(
                    Cause::new(
                        codes::RENDER_TRANSPARENT,
                        if *from_param { Likelihood::Medium } else { Likelihood::High },
                        format!("Its material `{name}` sets `{output}` to {how}, so it is fully transparent"),
                    )
                    .evidence(format!("{name}: output {output} = 0"))
                    .subject(subject.clone())
                    .advice(format!("give `{output}` a value above 0 in `{name}` (1 for opaque)")),
                );
            }
        }
        for r in bus(cx.facts, &[], vec![subject.clone()]) {
            if r.diagnostic.severity == Severity::Error {
                ok = false;
                f.cause(from_record(
                    codes::RENDER_MATERIAL,
                    Likelihood::High,
                    format!("Its material `{name}` reported an error"),
                    &r,
                    cx.facts,
                ));
            }
        }
    }
    f.check(
        "material",
        if ok { Status::Ok } else { Status::Fail },
        refs.iter().map(|(_, r)| *r).collect::<Vec<_>>().join(" "),
    );
    subjects
}

fn tint(cx: &Ctx<'_>, f: &mut Findings) {
    let Some((field, value)) = cx.renderer_field(&["tint", "color"]) else {
        return;
    };
    let Some(rgba) = value.as_array().filter(|a| a.len() == 4) else {
        return;
    };
    if rgba[3].as_f64() == Some(0.0) {
        let path = cx.renderer_path(field);
        let mut fixed = rgba.clone();
        fixed[3] = json!(1);
        f.cause(
            Cause::new(
                codes::RENDER_TRANSPARENT,
                Likelihood::High,
                format!("`{path}` has alpha 0, so the mesh is drawn fully transparent"),
            )
            .evidence(format!("{} {path} = {}", cx.e.address, value))
            .subject(Subject::entity(cx.e.address.clone()))
            .fix(set("Make the tint opaque", &cx.e.address, &path, Value::Array(fixed))),
        );
        f.check("tint", Status::Fail, "alpha 0");
    }
}

/// The entity's render layer: the renderer's `layer`, a `Layer` component, or a
/// `layer` flag; `default` when none is set.
fn layer(cx: &Ctx<'_>) -> (Value, Option<String>) {
    if let Some((_, v)) = cx.renderer_field(&["layer"]) {
        return (v.clone(), Some(cx.renderer_path("layer")));
    }
    if let Some((name, v)) = cx.e.component(&["Layer", "RenderLayer"], None) {
        let field = ["layer", "name", "value"].into_iter().find(|k| v.get(*k).is_some());
        if let Some(k) = field {
            return (v[k].clone(), Some(format!("{name}.{k}")));
        }
    }
    match cx.e.flags.get("layer") {
        Some(v) => (v.clone(), Some("layer".into())),
        None => (json!("default"), None),
    }
}

/// Whether a camera mask includes a layer; `None` when they cannot be compared.
fn sees(mask: Option<&Value>, layer: &Value) -> Option<bool> {
    let Some(mask) = mask.filter(|m| !m.is_null()) else {
        return Some(true);
    };
    match (mask, layer) {
        (Value::Array(items), _) => {
            let all = items
                .iter()
                .any(|i| i.as_str().is_some_and(|s| matches!(s, "*" | "all" | "everything")));
            Some(
                all || items.iter().any(|i| match (i, layer) {
                    (Value::String(a), Value::String(b)) => a.eq_ignore_ascii_case(b),
                    (a, b) => a.as_f64().is_some() && a.as_f64() == b.as_f64(),
                }),
            )
        }
        (Value::Number(bits), Value::Number(index)) => {
            let (bits, index) = (bits.as_u64()?, index.as_u64()?);
            Some(index < 64 && bits & (1 << index) != 0)
        }
        _ => None,
    }
}

fn camera_off(cam: &Entity) -> bool {
    !off_flags(cam, Some("Camera")).is_empty()
}

fn cameras(cx: &Ctx<'_>, world: Affine, f: &mut Findings) {
    if !cx.facts.has_component_type("Camera") {
        f.check("camera", Status::Skip, "no Camera type in this project");
        return;
    }
    let cams: Vec<Entity> = cx
        .facts
        .with_component(&cx.e.doc, "Camera")
        .into_iter()
        .filter(|c| !camera_off(c))
        .collect();
    if cams.is_empty() {
        f.check("camera", Status::Fail, "none enabled");
        f.cause(
            Cause::new(
                codes::RENDER_NO_CAMERA,
                Likelihood::Medium,
                format!("`{}` has no enabled camera, so nothing in it is drawn in game (the editor viewport has its own)", cx.e.doc),
            )
            .advice("add an entity with a `Camera` component (`entity.create`), or enable the existing one"),
        );
        return;
    }
    let (layer, layer_path) = layer(cx);
    let pos = world.t;
    let limit = cx
        .renderer_field(&["maxDistance", "drawDistance", "cullDistance"])
        .and_then(|(k, v)| Some((k, v.as_f64()?)))
        .filter(|(_, d)| *d > 0.0);
    let mut culled = Vec::new();
    let mut seeing = Vec::new();
    for cam in &cams {
        let mut chain: Vec<Entity> = ancestors(cx.facts, cam);
        chain.reverse();
        let cam_world = chain
            .iter()
            .chain(std::iter::once(cam))
            .fold(Affine::IDENTITY, |w, e| w.then(&Affine::from_transform(e.components.get("Transform"))));
        let mask = cam.get("Camera", "cullingMask");
        match sees(mask, &layer) {
            Some(false) => culled.push((cam, mask.cloned().unwrap_or(Value::Null))),
            _ => seeing.push((cam, cam_world)),
        }
    }
    let layer_text = layer.as_str().map_or_else(|| layer.to_string(), str::to_owned);
    if seeing.is_empty() {
        f.check("layer", Status::Fail, format!("`{layer_text}`"));
        let mut c = Cause::new(
            codes::RENDER_LAYER_CULLED,
            Likelihood::High,
            format!("No camera renders its layer `{layer_text}`: every camera's culling mask leaves it out"),
        )
        .subject(Subject::entity(cx.e.address.clone()));
        for (cam, mask) in &culled {
            c = c.evidence(format!("Camera {} cullingMask = {mask}", cam.label()));
        }
        if let (Some(path), Some((_, Value::Array(items)))) = (&layer_path, culled.first()) {
            let target = items
                .iter()
                .find(|i| i.as_str().is_some_and(|s| s.eq_ignore_ascii_case("default")))
                .or_else(|| items.first());
            if let Some(target) = target {
                c = c.fix(set(format!("Move it to layer {target}"), &cx.e.address, path, target.clone()));
            }
        }
        if let Some((cam, Value::Array(items))) = culled.first() {
            let mut widened = items.clone();
            widened.push(layer.clone());
            c = c.fix(set(
                format!("Add `{layer_text}` to the culling mask of {}", cam.label()),
                &cam.address,
                "Camera.cullingMask",
                Value::Array(widened),
            ));
        }
        f.cause(c);
        return;
    }
    f.check("layer", Status::Ok, format!("`{layer_text}`"));
    // Distance and direction, against the cameras that render the layer.
    let mut nearest: Option<(f64, &Entity, f64)> = None;
    let mut in_front = false;
    for (cam, cam_world) in &seeing {
        let to = math::sub(pos, cam_world.t);
        let d = math::length(to);
        let far = cam.get("Camera", "far").and_then(Value::as_f64).unwrap_or(f64::INFINITY);
        let forward = cam_world.vector([0.0, 0.0, -1.0]);
        if math::dot(forward, to) >= 0.0 || d < 1e-6 {
            in_front = true;
        }
        if nearest.is_none_or(|(nd, _, _)| d < nd) {
            nearest = Some((d, cam, far));
        }
    }
    let Some((d, cam, far)) = nearest else {
        return;
    };
    if !world.is_finite() {
        f.check("distance", Status::Skip, "position not finite");
        return;
    }
    let mut ok = true;
    if d > far {
        ok = false;
        let new_far = (d * 1.25).ceil();
        f.cause(
            Cause::new(
                codes::RENDER_DISTANCE_CULLED,
                Likelihood::High,
                format!("It is {} m from the nearest camera {}, beyond its far plane ({} m)", show(d), cam.label(), show(far)),
            )
            .evidence(format!("{} at {}; Camera far = {}", cx.e.address, show3(pos), show(far)))
            .subject(Subject::entity(cx.e.address.clone()))
            .fix(set(format!("Raise the camera's far plane to {} m", show(new_far)), &cam.address, "Camera.far", json!(new_far))),
        );
    }
    if let Some((field, limit)) = limit
        && d > limit
    {
        ok = false;
        let path = cx.renderer_path(field);
        f.cause(
            Cause::new(
                codes::RENDER_DISTANCE_CULLED,
                Likelihood::High,
                format!("It is {} m from the nearest camera but `{path}` is {} m", show(d), show(limit)),
            )
            .subject(Subject::entity(cx.e.address.clone()))
            .fix(set(format!("Raise `{path}`"), &cx.e.address, &path, json!((d * 1.25).ceil()))),
        );
    }
    f.check(
        "distance",
        if ok { Status::Ok } else { Status::Fail },
        format!("{} m from {}", show(d), cam.label()),
    );
    if !in_front {
        f.cause(
            Cause::new(
                codes::RENDER_OUT_OF_VIEW,
                Likelihood::Low,
                "It is behind every camera that renders its layer".to_owned(),
            )
            .evidence(format!("nearest camera {} at {} m", cam.label(), show(d)))
            .advice("move it in front of the camera, or turn the camera"),
        );
    }
}

fn streaming(cx: &Ctx<'_>, f: &mut Findings) {
    let records = bus(
        cx.facts,
        &[codes::WORLD_NOT_RESIDENT, codes::WORLD_CELL_FAILED],
        vec![Subject::entity(cx.e.address.clone())],
    );
    if records.is_empty() {
        f.check("streaming", Status::Skip, "no reports");
        return;
    }
    f.check("streaming", Status::Fail, "");
    for r in &records {
        let failed = r.diagnostic.code == codes::WORLD_CELL_FAILED;
        f.cause(from_record(
            r.diagnostic.code,
            if failed { Likelihood::High } else { Likelihood::Medium },
            if failed {
                "Its world cell failed to load".to_owned()
            } else {
                "Its world cell is streamed out".to_owned()
            },
            r,
            cx.facts,
        ));
    }
}

fn passes(cx: &Ctx<'_>, f: &mut Findings) {
    let records = bus(
        cx.facts,
        &[codes::RENDER_PASS_EXCLUDED],
        vec![Subject::entity(cx.e.address.clone())],
    );
    if records.is_empty() {
        f.check("passes", Status::Unknown, "needs the running renderer");
        return;
    }
    f.check("passes", Status::Fail, "");
    for r in &records {
        f.cause(from_record(
            codes::RENDER_PASS_EXCLUDED,
            Likelihood::High,
            "The renderer left it out of every pass".to_owned(),
            r,
            cx.facts,
        ));
    }
}

/// Other warnings and errors about the entity, its ancestors or its assets.
fn related(cx: &Ctx<'_>, assets: &[Subject], f: &mut Findings) {
    let mut subjects = vec![Subject::entity(cx.e.address.clone())];
    subjects.extend(cx.chain.iter().map(|a| Subject::entity(a.address.clone())));
    subjects.extend(assets.iter().cloned());
    let shown: Vec<String> = f.causes.iter().flat_map(|c| c.evidence.clone()).collect();
    for r in bus(cx.facts, &[], subjects).iter().rev().take(5) {
        let l = line(r);
        if !shown.iter().any(|s| s.contains(&r.diagnostic.message)) {
            f.related.push(l);
        }
    }
}
