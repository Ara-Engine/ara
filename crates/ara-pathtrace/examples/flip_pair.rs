//! Prints the mean LDR ꟻLIP of `test` against `reference` (PNG paths), for
//! cross-checking `ara_pathtrace::flip` against other implementations.
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(reference), Some(test)) = (args.next(), args.next()) else {
        anyhow::bail!("usage: flip_pair <reference.png> <test.png>");
    };
    let reference = image::open(reference)?.to_rgba8();
    let test = image::open(test)?.to_rgba8();
    let result =
        ara_pathtrace::flip::flip(&reference, &test, ara_pathtrace::flip::PIXELS_PER_DEGREE)?;
    println!("{:.6}", result.mean);
    Ok(())
}
