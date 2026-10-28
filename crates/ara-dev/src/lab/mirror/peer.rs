//! Bootstrap only our authenticated peer; never retire another lab or unrelated PID.
use super::{Config, transport::quote};
pub(super) fn bootstrap(c: &Config) -> String {
    let exe = quote(&format!("{}/debug/ara-dev", c.build_dir));
    let base = quote(&format!("{}/target/ara-dev/lab", c.repo));
    format!(
        r#"{prelude}
/usr/bin/pmset -g batt | grep -q "Now drawing from 'AC Power'" || {{ echo 'ARA_LAB_MAC_BATTERY: connect the charger' >&2; exit 1; }}
base={base}
mkdir -p "$base/mirror" {target}
signature=$(git rev-parse HEAD:crates/ara-dev HEAD:Cargo.lock HEAD:Cargo.toml | tr '\n' '-')
if ! test -x {exe} || test "$(cat "$base/mirror/tool-source" 2>/dev/null || true)" != "$signature"; then
 if test -f "$base/endpoint.json"; then
  cmp -s "$base/endpoint.json" "$base/mirror/owned-endpoint.json" || {{ echo 'Mac lab belongs to another owner; refusing to stop it' >&2; exit 1; }}
  printf '%s\n' '{{"tool":"lab_stop","arguments":{{}}}}' | {exe} lab rpc >&2
  n=0
  while test -f "$base/endpoint.json"; do n=$((n+1)); test "$n" -le 100 || exit 1; sleep 0.1; done
 fi
 if /usr/bin/pgrep -x cargo >/dev/null; then echo 'another Mac Cargo build is running' >&2; exit 1; fi
 available=$(/bin/df -Pk {target} | /usr/bin/awk 'END {{print $4}}')
 test "$available" -ge 2097152 || {{ echo 'Mac bootstrap needs 2 GiB free' >&2; exit 1; }}
 /usr/bin/pmset -g batt | grep -q "Now drawing from 'AC Power'" || {{ echo 'ARA_LAB_MAC_BATTERY: connect the charger' >&2; exit 1; }}
 /usr/bin/caffeinate -dimsu /usr/bin/nice -n 10 env CARGO_BUILD_JOBS=1 RUSTC_WRAPPER= CARGO_TARGET_DIR={target} CARGO_BUILD_BUILD_DIR={target} cargo build --locked -p ara-dev >&2
 printf '%s' "$signature" > "$base/mirror/tool-source"
fi
if test -f "$base/endpoint.json"; then
 cmp -s "$base/endpoint.json" "$base/mirror/owned-endpoint.json" || {{ echo 'Mac lab belongs to another owner' >&2; exit 1; }}
fi
ARA_LAB_MIRROR=1 ARA_LAB_REFERENCES={references} {exe} lab start --build-dir {target} --max-sessions 1
cp "$base/endpoint.json" "$base/mirror/owned-endpoint.json"
"#,
        prelude = super::transport::prelude(c),
        target = quote(&c.build_dir),
        references = quote(&format!("{}/target/ara-dev/lab/references-ldr", c.repo))
    )
}
