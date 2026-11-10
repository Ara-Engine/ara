//! Hash S8's raw state bytes with the same BLAKE3 crate as Ara's ECS benchmark.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: dump-hash <state.bin>")?;
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(std::fs::File::open(path)?)?;
    println!("{}", hasher.finalize().to_hex());
    Ok(())
}
