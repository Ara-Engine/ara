set -eu
cd /home/islam/ara-ux-editor
source <(head -n 15 target/ux-p4/kit-kernel-tests.sh)
nice -n 10 cargo test -p ara-diag --test coverage --locked >"$gate_dir/diag-coverage.log" 2>&1 || { tail -n 85 "$gate_dir/diag-coverage.log"; exit 1; }
tail -n 12 "$gate_dir/diag-coverage.log"
INSTA_UPDATE=no nice -n 10 cargo test -p ara-doc -p ara-cmd --locked >"$gate_dir/kernel-full.log" 2>&1 || { tail -n 100 "$gate_dir/kernel-full.log"; exit 1; }
tail -n 20 "$gate_dir/kernel-full.log"
