# Editor UX v2, phase 2: the alpha journey

The 0.0.1-alpha exit criterion: the essential journey through the editor's
visible UI only, on the Outpost sample. Produced at `88b59c84` by

```
ara-editor <copy of samples/outpost> --verify-journey --size 1440x900  --dpi 1 --output target/journey-1440/out
ara-editor <copy of samples/outpost> --verify-journey --size 2560x1369 --dpi 1 --output target/journey-2560/out
```

with the dev profile (optimized) on the RTX 4070 SUPER, dark theme. Only a compact
set is committed: `1440/` holds the 1440×900 frames as WebP (quality 90) and
`journey.json`, about 3 MB. The full PNG sets at both sizes, with each step's
`.layout.json` dump, stay out of history (they were under
`target/journey-evidence/88b59c84/{1440,2560}` on the machine that produced them).
Regenerating replaces the compact set; it should stay well under 10 MB. The project is
the complete Outpost (sources fetched, `.ara/cache` warm); each step's capture waits
until the Scene's content is published. The window is hidden and the UI is read back
offscreen. Each step drives the editor's own paths: registry commands (the same ones
the menus, shortcuts and palette run), the Assets drag and drop, the Inspector, and
the Game view's input routing. `journey.json` records each step: what it did,
whether it worked, and the frames and seconds it took.

| Step | What it shows |
|---|---|
| `01-open-scene-picker` | File > Open Scene: every scene, recent first |
| `02-scene-open` | `hangar.scene` opens through its follow camera (see Known gaps) |
| `03-import` | Assets > Import… `rock.glb`: thumbnail and import settings (shared `import.initialize`) |
| `04-place` | The Assets search finds `rock.glb`; dragged into the Scene view, named after its file |
| `05-prefab` | Create > Prefab from Selection: a linked instance and the prefab bar |
| `06-override` | Scale edited in the Inspector (1 override); the field menu offers Apply/Revert Override |
| `07-apply-override` | Apply all writes the override into `prefabs/rock.prefab` |
| `08-export` | Main Scene = `hangar.scene`; Build > Build Current Profile creates `presets/windows.export.json` and exports the game into `builds/` |
| `09-run-build` | Build > Run Build runs the exported game headless for one frame (`09-run-build-game`) |
| `10-script` | Create > C# Behaviour: `Spinner.cs` in the project's hot module |
| `11-play` | Play inside the Game view (D19); click to capture, hold W: the Salvager walks |
| `12-hot-reload` | `Spinner.cs` edited while playing; the Console shows "C# reloaded at tick N" |
| `13-failure` | A semicolon deleted while playing: CS1513 in Problems (`scripts/Outpost/Spinner.cs:15`) with Why |
| `14-stop` | The script fixed, Play stopped |
| `15-undo` | The rock moved; Edit > Undo names the move |
| `16-save-reopen` | Undo, Save, open `outpost.scene`, reopen `hangar.scene` (its camera comes back) |
| `17-export-csharp` | Build Current Profile again: the C# module ships NativeAOT (`game/scripts.json`) |
| `18-run-build-csharp` | Run Build runs that game, its C# included (`18-run-build-csharp-game`) |

| | 1440×900 | 2560×1369 |
|---|---:|---:|
| Steps that worked | 18 / 18 | 18 / 18 |
| Whole route (s) | 196.9 | 203.7 |
| Build Current Profile, first export (s) | 62.7 | 59.7 |
| Build with the C# module (s) | 71.2 | 68.8 |
| Run Build, one headless frame (s) | 8.0 | 8.2 |
| Play: first Game view frame and 2.5 s walk (s) | 13.3 | 11.7 |
| Hot reload visible in the Console (s) | 0.6 | 0.8 |
| Compile error in Problems (s) | 0.6 | 0.7 |

Each capture also runs the full layout dump and a GPU readback, so step times are
upper bounds. Most of a step is the 13-frame settle plus the capture.

## Known gaps

- **The opening Scene frame.** `02-scene-open` frames the hangar from its follow
  camera, with the boom stopped in front of the back wall. The fidelity Scene
  capture of that first open shows a brown occluder; a raster capture from the same
  camera shows the Salvager in the hangar, as intended. This is with the renderer.
- **The hangar's start pose in the game.** In these frames the follow camera starts
  behind the back wall, so the exported game's first frames (`09-…-game`,
  `18-…-game`) show that wall. Since then, `place_cameras` stops the boom in front of
  static box colliders, as the editor's opening view does (`ara_runtime::follow`).
- **Seeing the reload.** The behaviour is not attached to an entity: the module's
  types exist only after the first Play compiles it. So the reload shows as the
  Console line, not as motion.
- These are single frames. The walk and the reload are shown by their results.
