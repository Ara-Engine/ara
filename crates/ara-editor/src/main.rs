use std::path::PathBuf;
fn main() -> ara_ui::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--import-worker") {
        ara_asset::serve_worker(&ara_importers::registry()?)?;
        return Ok(());
    }
    let mut options = ara_editor::Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--verify" => options.verify = true,
            "--verify-panels" => options.verify_panels = true,
            "--sample" => options.sample = true,
            "--screenshot" => options.screenshot = true,
            "--panel" => options.panel = Some(args.next().ok_or("--panel needs a panel name")?),
            "--size" => {
                let size = args.next().ok_or("--size needs WxH")?;
                let (w, h) = size.split_once('x').ok_or("--size needs WxH")?;
                options.size = [w.parse()?, h.parse()?];
                if options.size.iter().any(|v| *v < 320 || *v > 8192) {
                    return Err("invalid window size".into());
                }
            }
            "--dpi" => {
                let dpi: f32 = args.next().ok_or("--dpi needs a scale")?.parse()?;
                if !dpi.is_finite() || !(0.5..=4.).contains(&dpi) {
                    return Err("invalid DPI".into());
                }
                options.dpi = Some(dpi);
            }

            "--frames" => {
                options.frames = Some(args.next().ok_or("--frames needs a number")?.parse()?)
            }
            "--output" => {
                options.output = PathBuf::from(args.next().ok_or("--output needs a directory")?)
            }
            "--help" | "-h" => {
                println!(
                    "ara-editor <project> [--sample] [--frames N] [--verify | --verify-panels] [--size WxH] [--dpi SCALE] [--panel NAME] [--screenshot] [--output DIR]\n\nNative Vulkan editor + project Engine Protocol authority.\n--sample creates an undoable sample scene if the scene is empty.\n--verify runs native/protocol interactions, screenshots and timings, then exits.\n--verify-panels runs real imports, panel interactions, both themes and a 500-node navigation benchmark."
                );
                return Ok(());
            }
            _ if arg.starts_with('-') => return Err(format!("Unknown option {arg}").into()),
            _ => options.project = PathBuf::from(arg),
        }
    }
    ara_editor::run(options)
}
