# ara-input

Device-independent action declarations, physical bindings and restorable tick input.
No ECS, window, networking or device-driver dependency.

- `ActionMap`: declare typed actions, bind named physical controls, validate injection.
- `Injection`: explicit `PlayerId`, action name/value and positive hold duration.
- `InputState::apply`: atomic validation, expiry, then submission-order replacement.
- `InputRecord`: tick-indexed applied inputs for replay consumers.
- `device` translates active controls; an adapter chooses owner and tick.

Missing/expired actions are neutral (`None`). The last submission for one owner/action
wins. A button held for one tick is a press. Axes must be finite. Physical bindings do
not aggregate axes; a device adapter can aggregate contributions before injection.
InputState contains active holds; history is optional and owned by ara-sim recording.

Run `cargo test -p ara-input`, `cargo clippy -p ara-input --all-targets -- -D warnings`,
and `cargo fmt -p ara-input -- --check`.
