//! Baseline for binary size / build time: same output shape, no physics.
fn main() {
    let mut y = 5.0f32;
    let mut v = 0.0f32;
    for _ in 0..60 {
        v -= 9.81 / 60.0;
        y = (y + v / 60.0).max(0.5);
    }
    println!("{y}");
}
