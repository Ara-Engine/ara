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
Play consumes these command settings immediately, including a newly initialized
import or a configure operation before the host's debounced sidecar save.
Sidecars save alongside the source while asset references continue to name the
source file. Undo removes a newly initialized sidecar, preserving the source.

Merge a model's companion opacity image during its cook:

```powershell
ara do import.configure --args '{"source":"models/bush.gltf","settings":{"opacityMaps":{"textures/bush_diff.jpg":"textures/bush_alpha.png"}}}' --why 'Use the supplied foliage cutout'
```

Map keys name glTF image URIs, or `#0` for an embedded image index. Values are
companion paths relative to the model. The mask's red channel multiplies existing
alpha; its bytes participate in cache invalidation. The importer preserves each
texture's transform/UV selector, emissive strength and transmission data.

Audio `decodedBudget` counts resident PCM staging bytes (64 MiB default; 64 KiB
minimum). Music and dialogue decode and resample in bounded blocks before Opus
encoding, independent of track duration. Effects decode into a resident clip and
must fit the budget. Compressed input/output and codec/meter scratch are separate.
Play keeps streamed imports compressed and feeds 4096-frame stereo PCM queues
(about 33 KiB per voice) under an 8 MiB aggregate queue budget. Live playback
decodes on workers; offline captures refill synchronously before each render
quantum. Looping rewinds the compressed source on its producer. Graph sample
nodes retain resident loading for their short effects.

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

Select a source kit piece and switch a lamp's emissive material without generating
a derived glTF:

```powershell
ara do component.add --doc levels/yard.scene --args '{"entity":"e_lamp","component":"MeshRenderer","value":{"model":"@models/kit.gltf#as_kit","node":"WallLamp","emissiveFactor":[2000,1800,1000],"emissiveMaterials":["Bulb"]}}' --why 'Use the kit lamp and light its bulb'
ara do property.set --doc levels/yard.scene --args '{"entity":"e_lamp","path":"MeshRenderer.emissiveEnabled","value":false}' --why 'Switch the lamp off'
```

`node` selects a named source subtree or numeric node index as a string; `name:3`
selects a numeric name. Duplicate names require an index. The selected pivot moves
to the entity origin while source rotation/scale and descendant transforms remain.
Omitting `node` retains the complete default scene and its authored placement.
`primitive` optionally selects a zero-based primitive within that subtree.

Imported emission is enabled by default. `emissiveFactor` replaces the source RGB
factor for the named/indexed `emissiveMaterials` (empty targets all), then
`emissiveStrength` multiplies emission once. `emissiveEnabled` switches it off.
Procedural shapes retain emitted-radiance semantics. Invalid node/primitive or
material selectors report `ARA7310`; invalid geometry selections show a placeholder.

`doc.delete` removes a loaded document and its records with attributed history and
undo. Run from another batch document; `ara.project.json` is the usual anchor.
Delete and recreate the same ID in one batch for a reproducible regeneration.

```powershell
ara do doc.delete --doc ara.project.json --args '{"document":"@levels/generated.scene#sc_generated"}' --why 'Regenerate the generated level'
```

`materialOverrides` replaces individual primitive materials with native surface
graphs or `.materialinstance` assets. Slot numbers are relative to the selected
subtree before `primitive` filtering; procedural shapes have slot zero.

```powershell
ara do property.set --doc levels/yard.scene --args '{"entity":"e_lamp","path":"MeshRenderer.materialOverrides","value":[{"primitive":0,"material":"@materials/paint.graph#gr_paint"}]}' --why 'Use the native paint material'
```

The same MCP call is `ara_apply` with `doc: "levels/yard.scene"` and a
`property.set` command containing those arguments. Graph defaults and typed
instance values compile at content boundaries; failed reloads retain the last
good material. Unsupported reachable expressions report their graph/node identity.
PBR constants, affine UVs, normal strength, RGB samples, shared ORM channels,
base-color alpha masks and transmission lower without baking a new glTF.
Graph color samples filter raw UNORM before sRGB decoding; data/alpha stay raw.
The Basic renderer shares one UV stream; fidelity consumes per-slot coordinates.

Create collision from the same source model selection:

```powershell
ara do physics.collider.fromModel --doc levels/yard.scene --args '{"entity":"e_crate","mode":"convex"}' --why 'Make the selected kit piece solid'
```

MCP uses `ara_apply` with the same command and arguments. Omit `model`, `node`
and `primitive` to inherit the entity's MeshRenderer selection, or supply a model
explicitly. `convex` supports moving bodies; `mesh` requires a static body. A
missing RigidBody is added as static. Cooking uses published model geometry with
the selected pivot and entity scale; no source glTF is rewritten.

`Collider.center` offsets any rigid-body shape in body-local metres, after scale
is baked. Body rotation rotates that offset. `PhysicsId` may be omitted or zero;
Play assigns a unique nonzero value from the stable entity identity, reserving
explicit positive IDs. Automatic values survive snapshots and streamed adoption.

`terrain.raise`, `terrain.paint` and `foliage.exclude` accept an optional `id`.
Use stable IDs such as `s_hill`, `s_grass` and `x_road` for reproducible builds.
MCP's `as` binds a batch symbol in the same way as `entity.create`. Newly touched
terrain tiles also derive stable IDs from their grid coordinates.

Terrain grids may have a nonzero `origin` and signed tile bounds. A scene's
`Terrain` entity may also have a translated `Transform`, including a translated
parent: that offset reaches rendering, foliage, navigation and terrain collision.
Strokes and foliage exclusions stay in the terrain document's authored coordinates.
Bake rotation/scale into the grid; scene terrain transforms currently accept
translation. The landscape pass retains its square extent and 32-cell tile ABI.
