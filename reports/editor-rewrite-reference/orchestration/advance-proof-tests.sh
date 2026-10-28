set -eu
cd /home/islam/ara-ux-editor
git fetch origin t3/ux-editor-p4
for file in $(git diff --name-only); do
 case "$file" in
  crates/ara-ui-test/goldens/widget-popover-[0-5].png) ;;
  crates/ara-ui-test/goldens/dock-[45].png|crates/ara-ui-test/goldens/widget-splitter-[45].png|crates/ara-ui-test/goldens/widget-tooltip-[45].png)
   actual=$(git hash-object "$file")
   expected=$(git rev-parse "origin/t3/ux-editor-p4:$file")
   if [ "$actual" != "$expected" ]; then printf '%s\n' "Golden artifact differs: $file"; exit 1; fi ;;
  *) printf '%s\n' "Unexpected dirty source preserved: $file"; exit 1 ;;
 esac
done
# Only this owner's explicit opt-in golden outputs are released from the old
# checkout. The six accepted images are already committed; six within-tolerance
# popover images intentionally keep the previous baseline.
for file in $(git diff --name-only); do git restore --source=HEAD -- "$file"; done
git checkout --detach origin/t3/ux-editor-p4
source <(head -n 15 target/ux-p4/kit-kernel-tests.sh)
mkdir -p "$gate_dir"
nice -n 10 cargo clippy -p ara-ui -p ara-ui-document -p ara-ui-stories -p ara-ui-test -p ara-doc -p ara-cmd -p ara-platform -p ara-play -p ara-play-host -p ara-runtime-net -p ara-runtime-render -p ara-render -p ara-vfx-gpu -p ara-player -p ara-editor --all-targets --locked -- -D warnings >"$gate_dir/clippy.log" 2>&1 || { tail -n 95 "$gate_dir/clippy.log"; exit 1; }
tail -n 7 "$gate_dir/clippy.log"
nice -n 10 cargo test -p ara-editor --lib --locked -- --skip editor_overlays_and_control_choices_fit_all_mounted_contexts --skip scene_captures_native_quality_environment_and_effects_after_resize >"$gate_dir/editor-libs.log" 2>&1 || { tail -n 100 "$gate_dir/editor-libs.log"; exit 1; }
tail -n 14 "$gate_dir/editor-libs.log"
nice -n 10 cargo test -p ara-ui-test --test suite --locked >"$gate_dir/story-verified.log" 2>&1 || { tail -n 75 "$gate_dir/story-verified.log"; exit 1; }
tail -n 7 "$gate_dir/story-verified.log"
nice -n 10 cargo test -p ara-editor --lib editor_overlays_and_control_choices_fit_all_mounted_contexts --locked >"$gate_dir/full-overlays.log" 2>&1 || { tail -n 85 "$gate_dir/full-overlays.log"; exit 1; }
tail -n 7 "$gate_dir/full-overlays.log"
