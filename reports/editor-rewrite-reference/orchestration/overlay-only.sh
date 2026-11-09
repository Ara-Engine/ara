set -eu
cd /home/islam/ara-ux-editor
source <(head -n 18 target/ux-p4/current-control-tests.sh)
nice -n 10 cargo test -p ara-editor --lib editor_overlays_and_control_choices_fit_all_mounted_contexts --locked > target/ux-p4/640edd7e/full-overlays.log 2>&1 || { tail -n 80 target/ux-p4/640edd7e/full-overlays.log; exit 1; }
tail -n 12 target/ux-p4/640edd7e/full-overlays.log
