use std::path::PathBuf;
fn main() -> ara_ui::Result<()> {
    if ara_crash::monitor_from_args()? {
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--import-worker") {
        ara_asset::serve_worker(&ara_importers::registry()?)?;
        return Ok(());
    }
    let mut options = ara_editor::Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--headless" => options.headless = true,
            "--version" | "-V" => {
                println!("ara-editor {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--verify" => options.verify = true,
            "--verify-panels" => options.verify_panels = true,
            "--verify-ux" => options.verify_ux = true,
            "--verify-journey" => options.verify_journey = true,
            "--sample" => options.sample = true,
            "--screenshot" => options.screenshot = true,
            "--panel" => options.panel = Some(args.next().ok_or("--panel needs a panel name")?),
            "--play" => options.play = true,
            "--scene" => options.scene = Some(args.next().ok_or("--scene needs a scene path")?),
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
                    "ara-editor <project> [--scene levels/x.scene] [--sample] [--headless] [--frames N] [--verify | --verify-panels | --verify-ux] [--size WxH] [--dpi SCALE] [--panel NAME] [--screenshot] [--output DIR]\n\nNative Vulkan editor + project Engine Protocol authority.\n--scene opens that scene instead of the project's main scene.\n--play starts Play in the Game view once the window is up.\n--headless captures editor-scene.png without creating a native window, then exits.\n--sample creates an undoable sample scene if the scene is empty.\n--verify runs native/protocol interactions, screenshots and timings, then exits.\n--verify-panels runs real imports, panel interactions, both themes and a 500-node navigation benchmark.\n--verify-ux captures menus, context menus, the palette, Settings, a floating Inspector and every tool tab in both themes, then records idle and camera timings."
                );
                return Ok(());
            }
            _ if arg.starts_with('-') => return Err(format!("Unknown option {arg}").into()),
            _ => options.project = PathBuf::from(arg),
        }
    }
    let _capture = match ara_crash::desktop::start(
        "ara-editor",
        !options.headless
            && options.frames.is_none()
            && !options.verify
            && !options.verify_panels
            && !options.verify_ux,
    ) {
        Ok(capture) => Some(capture),
        Err(error) => {
            eprintln!("ARA-EXP-CRASH-004: {error:#}");
            None
        }
    };
    ara_editor::run(options)
}
