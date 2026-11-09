set -eu
cd /home/islam/ara-ux-editor
file=crates/ara-cmd/tests/snapshots/commands__every_example_validates_and_deserializes.snap
actual=$(git hash-object "$file")
expected=$(git rev-parse "origin/t3/ux-editor-p4:$file")
if [ "$actual" != "$expected" ]; then printf '%s\n' 'Generated catalogue differs from the committed artifact; preserving it'; exit 1; fi
# This exact generated snapshot is committed at d14ec0a7; release its old index
# before advancing this checkout. No other source or artifacts are restored.
git restore --source=HEAD -- "$file"
git checkout --detach origin/t3/ux-editor-p4
bash target/ux-p4/kit-kernel-tests.sh
