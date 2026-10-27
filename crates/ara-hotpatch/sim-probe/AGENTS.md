# ara-hotpatch-sim-probe

Development-only bin host for a real ara-sim Session. It keeps the ECS entity and
system/query state alive while a named system body changes. Output is confined to
the required scratch directory. The probe verifies replay, restore and adapter
refusal after the first actual patch. It is not a shipping player.

Run through `ara-dev hotpatch -- sim-probe --output <scratch>` with the optional
prepared dx tool. Focused checks: clippy/test with `--features hotpatch`.
