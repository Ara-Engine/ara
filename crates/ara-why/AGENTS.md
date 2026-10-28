# ara-why

`ara why`: answers "why is X wrong?" with a short ranked list of likely causes,
each with its evidence and runnable fixes, plus the checks that ruled things out.
Pure logic over a `Facts` trait; the project host implements it (`ara-host::why`).
Depends on ara-diag and ara-material (material compile checks).

## Public API

- `Why::builtin()`, `Why::register(Box<dyn Explainer>)`, `Why::add(Box<dyn
  Collector>)`, `Why::explain(question, &dyn Facts) -> Result<Report, Vec<Diagnostic>>`,
  `Why::list()`. The highest `matches` score answers; ties go to the earlier one.
- `Question::parse(text)`: `entities` (`name:Crate`, `e_8b21`, `@levels/a.scene#e_…`),
  `subjects` (`@meshes/x.glb`, `module:Gameplay`, `system:…`, `pass:…`), `codes`
  (`ARA1302`, named codes, constant names), `words`, `names` (capitalized words),
  `doc` (`doc:outpost`). `name_candidates(stems)` turns free text into entity names
  (`gate crate` → `GateCrate`, `workbench` → `Workbench`).
- `Explainer { name, about, examples, matches(&Question) -> u32, explain(q, facts,
  extra) }`; `Collector { explainer, name, collect(&Target, facts, &mut Findings) }`
  adds evidence to an existing explainer (`Findings::collect` runs them).
- `Facts`: `entity(selector, doc)`, `parent`, `with_component(doc, comp)`,
  `has_component_type`, `asset(reference)` (`Asset { id, path, kind,
  source_exists, problem, closest }`), `read(path)`, `users(id, limit)`,
  `diagnostics(&Filter)`, `causes(&Record)`, `profile()` (summary JSON).
- `Report { question, explainer, subject, answer, causes, checks, related, doc }`,
  `Report::text()`; `Cause { code, likelihood (high/medium/low), title, evidence,
  subjects, fixes (runnable `ara_diag::Fix`), advice }`; `Check { name, status:
  ok/FAIL/n/a/? , note }`.
- `math`: `Affine` (TRS, XYZ Euler degrees as Rx·Ry·Rz, cameras look down −Z),
  `vec3`, `json3`, `show3`.

## Built-in explainers (`src/explain/`, one file each)

| Name | Matches | Reads |
|---|---|---|
| `visibility` | an entity + visibility words, or a bare entity | flags, renderer, transform chain, mesh, material (compile, domain, opacity), tint alpha, cameras (mask, far, behind), draw distance, streaming and pass reports |
| `code` | a code | catalog doc, instances on the bus with fixes and causes |
| `reload` | reload / L3 / module | `ARA2020/2021`, script compile and cascade codes; reason advice |
| `frame` | frame / slow / hitch | profiler summary (p95 vs budget, top scopes, hitches), `ARA2030` |
| `asset` | an asset or document | resolution, source file, material compile, bus reports, users |
| `subject` | any subject (fallback) | everything on the bus about it, with causes |

Cause codes: visibility uses ARA3301–3312 (`RENDER_*`), ARA2102/2202/2203; fixes
are `property.set` / `component.add` commands agents pass to `ara_apply` as is.

## Visibility sources

Geometry uses the registered schema's asset fields and procedural-selection hints
(the MeshRenderer shape/model contract), including Sprite/Tilemap atlases, terrain
and generated VFX. Legacy project renderer types without schema facts retain the
`mesh`/`sprite` convention. Other checks use `material`/`materials`, `tint`/`color`, `layer`,
`enabled`/`visible`, `maxDistance`/`drawDistance`/`cullDistance`. Flags: record
keys `enabled`/`active`/`visible`/`hidden`, components named `*Visib*`/`*Hidden*`.
Cameras: `Camera { cullingMask (names, indices or bitmask; absent = all), far,
enabled }`; native scenes without a camera use the renderer's overview fallback.

## Gotchas

- Runtime facts (NaN from physics, import failures, streaming, pass exclusion)
  only exist if a subsystem emitted them; the host persists engine reports in
  `.ara/diagnostics.jsonl`, so offline `ara why` still sees them.
- Checks marked `?` need the running renderer; never claim them as causes.
- Material checks parse `.material` sources with ara-material (asset IDs `gr_…`).

## Testing

`cargo test -p ara-why`: question parsing, transform math, and `tests/explain.rs`
(every explainer on an in-memory `Facts` fixture: zero scale inherited, layer
culled, material errors and zero opacity, missing mesh with closest fix, runtime
NaN with its physics cause, disabled ancestor, far plane, reload reasons, frame
summaries, code instances, assets, unclear questions).
