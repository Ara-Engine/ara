# ara-editor-hotpatch

Development bin-only host including the real editor entry point. Keeping the
editor as a normal path dependency works around dx's lib/bin replay and optional
dependency watcher gaps. It has no runtime target without `--features hotpatch`.

Panel, process and authoring behavior belong to ara-editor. Do not fork its main
function here. Test via `ara-dev hotpatch -- editor <scratch project>` with an
offscreen bounded run; keep profile/captures under target.

Focused check: `cargo clippy -p ara-editor-hotpatch --all-targets --features hotpatch -- -D warnings`.
