set -eu
cd /home/islam/ara-ux-editor
source <(head -n 15 target/ux-p4/kit-kernel-tests.sh)
ARA_UI_UPDATE_STORY_GOLDENS=1 nice -n 10 cargo test -p ara-ui-test --test suite --locked >"$gate_dir/story-update.log" 2>&1 || { tail -n 70 "$gate_dir/story-update.log"; exit 1; }
tail -n 10 "$gate_dir/story-update.log"
git diff --stat -- crates/ara-ui-test/goldens crates/ara-ui-test/examples
bash target/ux-p4/kit-kernel-tests.sh
