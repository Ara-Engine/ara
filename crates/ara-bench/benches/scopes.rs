//! Standard Ara kernel harness: Divan, no build-time dependencies beyond Rust/C++.
fn main() { divan::main(); }

#[divan::bench]
fn unattached_scope() {
    let _span = ara_profile::scope(std::hint::black_box("bench.scope"));
}

#[divan::bench]
fn frame_distribution() {
    let profile = ara_profile::Profile::default();
    std::hint::black_box(profile.summary(ara_profile::SummaryBudget::default())).ok();
}
