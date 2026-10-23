# Authoring through commands

The editor host, `ara do` and MCP's `ara_apply` use the shared command registry.
Every mutation produces attributed ops with a reason and turn-level undo. Agent
edits apply immediately under D24; `ara accept --all` remains an idempotent
compatibility action, including when called by the same agent.

Create an empty registered document and select a completed scene as the startup
scene:

```powershell
ara do doc.create --args '{"path":"levels/yard.scene","id":"sc_yard"}' --why 'Create the yard level'
ara do project.setMain --args '{"scene":"@levels/yard.scene#sc_yard"}' --why 'Start in the yard'
```

`doc.create` infers the kind from its registered extension, or accepts `kind`.
The document lifecycle is undoable, including after restarting the host. Existing
files are rejected. New scene, prefab, terrain, foliage, animation and music
documents can then be filled by their domain commands. Material templates create
an authoritative `.graph`; native UI templates create lossless XML:

```powershell
ara do doc.create --args '{"path":"materials/plain.graph","kind":"material","id":"gr_plain"}' --why 'Create a surface material'
ara do doc.create --args '{"path":"ui/hud.ui","id":"ui_hud"}' --why 'Create the HUD'
ara do ui.source.apply --doc ui/hud.ui --args '{"source":"<ui id=\"hud\"><text id=\"title\" text=\"Outpost\" /></ui>"}' --why 'Add the title'
```

UI comments, whitespace and XML declarations survive save, restart and `ara fmt`.
An `ara:asset` XML comment retains asset identity independently of element IDs.
`project.setMain` also changes Play's default scene immediately within a live
host, before the debounced file save.

Import source media without knowing importer IDs or version strings:

```powershell
ara do import.initialize --args '{"source":"models/crate.glb","id":"as_crate"}' --why 'Import the crate'
ara do import.configure --args '{"source":"models/crate.glb","settings":{"lodCount":3}}' --why 'Use three crate LODs'
ara do import.initialize --args '{"source":"audio/theme.ogg","id":"as_theme","settings":{"usage":"music","decodedBudget":65536}}' --why 'Import the streamed score'
```

Selection follows the source extension, and the engine records the installed
importer version. Configure replaces supplied settings, preset or dependencies;
omitted fields are retained. Settings are reflected and validated before commit.
Sidecars save alongside the source while asset references continue to name the
source file. Undo removes a newly initialized sidecar, preserving the source.

Audio `decodedBudget` counts resident PCM staging bytes (64 MiB default; 64 KiB
minimum). Music and dialogue decode and resample in bounded blocks before Opus
encoding, independent of track duration. Effects decode into a resident clip and
must fit the budget. Compressed input/output and codec/meter scratch are separate.

Animation commands expose the Rust `AnimationCommand` API:
`animation.create`, `animation.skeleton.set`, `animation.clip.set`,
`animation.controller.set`, `animation.rig.set`, `animation.ik.set`,
`animation.motionDatabase.set`, `animation.ragdoll.set` and
`animation.property.set`. Music uses `music.segment.set`, `music.transition.set`,
`music.stinger.set` and `music.settings.set`. Ask `ara schema <command>`
for the complete typed parameters.

MCP uses the same names and arguments:

```json
{
  "why": "Compose the ambient score",
  "doc": "audio/score.music",
  "commands": [
    {"cmd":"music.segment.set","args":{"id":"sg_ambient","segment":{"name":"Ambient","bars":8}}},
    {"cmd":"music.settings.set","args":{"settings":{"initial":"Ambient","seed":7}}}
  ]
}
```

Aim a camera or light's local minus Z at a world point or another entity:

```powershell
ara do transform.lookAt --args '{"entity":"e_camera","point":[0,2,8],"up":[0,1,0]}' --why 'Frame the doorway'
ara do transform.lookAt --args '{"entity":"e_light","target":"e_crate"}' --why 'Light the crate'
```

Supply exactly one of `point` and `target`. Parent transforms are respected;
parallel up/direction vectors use a deterministic upright fallback. Coincident
targets, zero up vectors and singular parent transforms diagnose without editing.

`terrain.raise`, `terrain.paint` and `foliage.exclude` accept an optional `id`.
Use stable IDs such as `s_hill`, `s_grass` and `x_road` for reproducible builds.
MCP's `as` binds a batch symbol in the same way as `entity.create`. Newly touched
terrain tiles also derive stable IDs from their grid coordinates.
