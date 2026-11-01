//! Finite-range many-light shading, V2 (council-v3 architecture D6, section 3.2, gate
//! G1): a light grid over authored ranges, per-pixel visibility through the shared
//! visibility service, and omission-only overload within a 1% energy bound.
//! `look::local` stays the default and the A/B control. Owned by the B1 build track.
