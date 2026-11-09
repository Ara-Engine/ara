set -eu
cd /home/islam/ara-ux-editor
source <(head -n 15 target/ux-p4/kit-kernel-tests.sh)
INSTA_UPDATE=always nice -n 10 cargo test -p ara-cmd --test commands every_example_validates_and_deserializes --locked >"$gate_dir/catalog-update.log" 2>&1 || { tail -n 70 "$gate_dir/catalog-update.log"; exit 1; }
tail -n 12 "$gate_dir/catalog-update.log"
git diff --stat -- crates/ara-cmd/tests/snapshots/commands__every_example_validates_and_deserializes.snap
INSTA_UPDATE=no nice -n 10 cargo test -p ara-doc -p ara-cmd --locked >"$gate_dir/kernel-full.log" 2>&1 || { tail -n 90 "$gate_dir/kernel-full.log"; exit 1; }
tail -n 18 "$gate_dir/kernel-full.log"
