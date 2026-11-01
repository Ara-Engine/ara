//! `trace()`: one ray query for every non-primary visibility and lighting caller
//! (council-v3 architecture D1). Software two-level CWBVHs: a TLAS over instances and
//! one BLAS per asset, built at import. Hardware RT is an optional backend of the same
//! interface, off by default. Owned by the A1 build track.
