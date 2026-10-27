use super::*;
use clap::Parser;

#[test]
fn exported_games_default_to_borderless_with_an_explicit_windowed_override() -> Result<()> {
    let args = Args::try_parse_from(["ara-player"])?;
    assert!(!args.standalone.windowed);
    let args = Args::try_parse_from(["ara-player", "--windowed"])?;
    assert!(args.standalone.windowed);
    assert!(!args.standalone.no_present);
    Ok(())
}

#[test]
fn no_present_is_an_explicit_hidden_perf_diagnostic() -> Result<()> {
    assert!(Args::try_parse_from(["ara-player", "--no-present"]).is_err());
    assert!(
        Args::try_parse_from([
            "ara-player",
            "--no-present",
            "--perf-output",
            "x",
            "--headless"
        ])
        .is_err()
    );
    let args = Args::try_parse_from(["ara-player", "--no-present", "--perf-output", "x"])?;
    assert!(args.standalone.no_present);
    let mut perf = perf::Window::new(args.standalone.perf, false, Instant::now());
    perf.configure_presentation(args.standalone.no_present, false);
    assert!(perf.offscreen());
    Ok(())
}
