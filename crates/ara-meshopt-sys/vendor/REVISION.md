# Pinned native source

Copied without edits from crates.io `meshopt` **0.7.0** (meshoptimizer **1.3**).
Crate checksum in Cargo.lock:
`a403c16e3ca5752600e1d4c5ba3ba2a6496f407ded66e767f05279514363f461`.

SHA-256:

- simplifier.cpp: `719303B6970398631E30724E8EEBC6CD2E7E08B67FE8AB7CBF7EECAFDAA08616`
- meshoptimizer.h: `E3688EF553F603391747895A5C221EF1528574CC7F8173782417A96CB531F61B`

Original MIT notice remains in meshoptimizer.h and LICENSE. Ara's separate
shim adds owned pristine-state preparation/reset and exception-contained C calls.
It includes upstream under private symbol names; the ordinary Rust meshopt crate
and all its entry points remain unchanged. The shim's prepare/run bodies retain
the upstream arithmetic, traversal and collapse order. Only the first ranking
and mutable state snapshots are reused. No upstream floating-point rewrite.
