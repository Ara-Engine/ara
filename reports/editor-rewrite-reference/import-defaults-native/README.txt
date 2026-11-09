Native import/defaults proof draft — NOT RUN / NOT ACCEPTANCE EVIDENCE

Files
- native-import-defaults.patch: six new app leaf files plus the small app/mod.rs hook.
- app-hooks.patch: only the serial hardfile hunk; Root should integrate it manually alongside J7/Recovery/Profile hooks.
- import_defaults_verify.rs and import_defaults_verify/: source copies for inspection.
- app-base.rs/app-next.rs: D-only snapshots used to generate the hook; do not copy a whole app/mod.rs into the repository.

No production editor, Host, renderer, importer, registry or schema files are changed by the leaf. No extra read-only production seam is necessary: it uses Shell::asset_import_selection, Shell::asset_authored_files, Store::workspace/history/revision and ProjectImportSettings::resolved_settings. Root must real-merge the existing import-defaults UI/backend before compiling this leaf. This writer performed no builds, tests, captures or source-repository edits; only standalone rustfmt on these D files.

App/mod.rs hook contract (all additive / serial)
1. mod import_defaults_verify.
2. Immediately after content_drags::fixture(&mut options), before Editor/Host open, import_defaults_verify::prepare(&options).
3. Local Option<ImportDefaultsCheck>.
4. Start after BookmarksCheck finishes; insert the check into the serial chain before LightingCheck starts. Each age calls inject with the actual Window and points_to_logical scale.
5. Route both event and shell-write clipboard paths to check.clipboard while active (in-memory only).
6. Call check.end after main UI render; this writes a requested hidden GPU screenshot once per recorded stage, with compact actual control geometry.
7. In the final behaviour gate call write and validate. The existing later serial route completion still determines exit.

Five required results
1. Two real source cards selected with pointer then primary-modifier pointer. Different TextureUsage and maxDimension are visibly mixed. A mounted shared Select changes Usage to Mask on both documents in exactly one human content transaction. Native Ctrl+Z and Ctrl+Y must revert the exact preceding transaction IDs. Complete sidecar records are compared at all three revisions; independent mipmaps and maxDimension must survive.
2. Through the real Project defaults button and the Texture page toggle, set flipNormalY in one policy transaction. Other importer slots and global Fast preset stay identical.
3. Through the shared Importer type Select choose glTF model, then disable virtualGeometry with its real toggle. Texture defaults and global preset stay identical.
4. Choose Audio through that Select, then choose Adpcm through its real codec Select. The other slots and global preset stay identical.
5. Through the actual global quality Select choose Balanced. All per-type maps must remain identical. In-memory worker resolution must show source Usage/size/mipmap overrides winning, inherited texture flipNormalY true, global Balanced quality effective, glTF virtualGeometry false, and audio Adpcm effective. Return via the real Back to selection button and require both original source selections still present.

Fixture / cleanup
- The existing behaviour route has already replaced the user's project with a fresh private fixture and marker. prepare refuses a missing marker or stale own subfolder.
- Two tiny real PNG sources and their sidecars are added only before Host open. Sidecars are produced using SidecarCommand::InitializeInherited -> kernel Edit -> Op -> canonical writer, after validating the partial texture settings with the real typed schema. Installed importer version is read from the actual registry. No JSON sidecar patch or post-open filesystem restore exists.
- The fixture starts with an existing canonical settings/ara.imports document: global Fast and absent per-type defaults. This avoids testing document creation while testing the UI scopes.
- Source settings intentionally remain partial (usage/mipmaps/maxDimension): absent flipNormalY must inherit the changed project texture default; serializing full source defaults would weaken this test.
- Cleanup closes the defaults page if needed, then uses one ordinary Editor.apply batch of import.configure and import.project.configure/resetType commands. It checks both complete source records and the whole policy return exactly to the initial in-memory values. Navigation, grid, inspected asset, maximization and focus are restored. Scene document, camera and entity selection must remain unchanged throughout. Like the existing Bookmarks proof, this fixture does not assert the docking layout unchanged.

Bounds / evidence
- 120-second correctness bound; one pointer location / intent per age. Scroll reveal uses actual SDL wheel events, never a retained scroll setter. All SDL events go through SDL_PushEvent and Window::poll_tagged.
- Persistent synthetic desktop coordinates accompany the SDL motion on the hidden window and survive keyboard/wait ages. No OS pointer/window is moved or shown.
- Compact snapshots contain only two sidecars, the small policy, at most 16 relevant content transaction summaries and at most 128 mounted import-control rectangles per capture. No per-frame scene serialization, filesystem read, codec, IO or diagnostic re-emission is added to production.
- output/import-defaults.json plus import-defaults-<stage>.png/.json. Until Root runs it and visually inspects the captures, the 5 required results are merely assertions in an unrun draft.

Static API review
- ImporterRegistry::get returns Result, not Option (handled).
- native::wheel takes only window, y, events (handled).
- Workspace::storage_path returns String; semantic index maps authored paths to storage keys for mounted card bindings.
- No new command IDs, diagnostic codes, public reflected types, external process or package dependency.
- New leaf code was parsed/formatted by rustfmt only; Rust type checking and Windows SDL/geometry behaviour remain unverified. Do not describe this artifact as merge-ready.

Root gates to request
- fmt, strict ara-editor all-target clippy, editor --lib and focused existing importer/defaults suites.
- matching-player Windows --verify-behaviours or core route with private output/profile, hidden window, appropriate shared GPU admission; no gate prepared-tests overlap.
- Require import-defaults.json completed=true, restored=true, passed=5 and error=null; inspect mixed/edit/Undo/Redo/defaults screenshots and control metadata. Preserve failures before rerunning.

Optional package native proof was intentionally not added before this import proof is compiled and exercised. It remains a separate follow-up: normal menu -> package format/destination dialog -> real completed manifest/hash -> reveal intent with no desktop window.