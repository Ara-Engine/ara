# Editor UX v2, phase 2: the alpha journey

The 0.0.1-alpha exit criterion: the essential journey through the editor's
visible UI only, on the Outpost sample. Produced by

```
ara-editor <copy of samples/outpost> --verify-journey --size 1440x900  --dpi 1 --output crates/ara-editor/evidence/journey/1440
ara-editor <copy of samples/outpost> --verify-journey --size 2560x1369 --dpi 1 --output crates/ara-editor/evidence/journey/2560
```

with the dev profile (optimized) on the RTX 4070 SUPER, dark theme. The window is
hidden and the UI is read back offscreen. Each step drives the editor's own paths:
registry commands (the same ones the menus, shortcuts and palette run), the Assets
drag and drop, the Inspector, and the Game view's input routing. `journey.json`
records each step: what it did, whether it worked, and the frames and seconds it
took. The PNGs were losslessly recompressed after capture.

| Step | What it shows |
|---|---|
| `01-open-scene-picker` | File > Open Scene: every scene, recent first |
| `02-scene-open` | `hangar.scene` opens, framed |
| `03-import` | Assets > Import… `rock.glb`: thumbnail and import settings (shared `import.initialize`) |
| `04-place` | The Assets search finds `rock.glb`; dragged into the Scene view, named after its file |
| `05-prefab` | Create > Prefab from Selection: a linked instance and the prefab bar |
| `06-override` | Scale edited in the Inspector (1 override); the field menu offers Apply/Revert Override |
| `07-apply-override` | Apply all writes the override into `prefabs/rock.prefab` |
| `08-script` | Create > C# Behaviour: `Spinner.cs` in the project's hot module |
| `09-play` | Play inside the Game view (D19); click to capture, hold W: the Salvager walks |
| `10-hot-reload` | `Spinner.cs` edited while playing; the Console shows "C# reloaded at tick N" |
| `11-failure` | A semicolon deleted while playing: CS1513 in Problems (`scripts/Outpost/Spinner.cs:15`) with Why |
| `12-stop` | The script fixed, Play stopped |
| `13-undo` | The rock moved; Edit > Undo names the move |
| `14-save-reopen` | Undo, Save, open `outpost.scene`, reopen `hangar.scene` (its camera comes back) |
| `15-export` | Build & Export > Build Current Profile, disabled, its tooltip says what to do next |

| | 1440×900 | 2560×1369 |
|---|---:|---:|
| Steps that worked | 15 / 15 | 15 / 15 |
| Whole route (s) | 215.7 | 225.0 |
| Play: first Game view frame and 2.5 s walk (s) | 19.7 | 21.7 |
| Hot reload visible in the Console (s) | 8.0 | 5.1 |
| Compile error in Problems (s) | 7.3 | 4.9 |
| Game view while capturing | 26–29 fps at 898×778 | 29 fps at 1604×1247 |

Each capture also runs the full layout dump and a GPU readback, so step times are
upper bounds. Most of a step is the 13-frame settle plus the capture.

## Known gaps

- **Export.** Build Current Profile stays disabled because the project has no build
  profile. Exporting a real project needs the integrated runtime player template
  (`ara.player.runtime/1`). ara-export has only the conformance template so far,
  and that one validates packs but does not run Outpost. The tooltip still points
  to the CLI to create a preset; a Build Profiles window arrives with the runtime
  template.
- **The hangar's start pose.** The follow camera starts inside the back wall
  (`samples/outpost`), so the Play step walks forward first, as the sample
  harness does.
- **Seeing the reload.** The behaviour is not attached to an entity: the module's
  types exist only after the first Play compiles it. So the reload shows as the
  Console line, not as motion.
- These are single frames. The walk and the reload are shown by their results.
