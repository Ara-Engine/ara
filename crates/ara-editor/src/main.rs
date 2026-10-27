use std::path::PathBuf;
fn main() -> ara_ui::Result<()> {
    #[cfg(feature = "hotpatch")]
    // SAFETY: This is process entry, before worker threads and native runtimes.
    let arguments = unsafe { ara_hotpatch::args_os()? };
    #[cfg(not(feature = "hotpatch"))]
    let arguments: Vec<_> = std::env::args_os().collect();
    let arguments = arguments
        .into_iter()
        .map(|a| {
            a.into_string()
                .map_err(|_| "editor arguments must be UTF-8")
        })
        .collect::<Result<Vec<_>, _>>()?;
    if arguments.get(1).map(String::as_str) == Some("--compile-scripts") {
        let root = PathBuf::from(arguments.get(2).ok_or("missing script project")?);
        return ara_editor::compile_scripts(&root);
    }
    ara_editor::LAUNCHED.get_or_init(std::time::Instant::now);
    if ara_crash::monitor_from_args()? {
        return Ok(());
    }
    if arguments.get(1).map(String::as_str) == Some("--import-worker") {
        ara_texture_gpu::serve_worker(&ara_importers::registry()?)?;
        return Ok(());
    }
    let mut options = ara_editor::Options::default();
    let mut args = arguments.into_iter().skip(1);
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
            "--verify-docking" => options.verify_docking = true,
            "--verify-assets" => options.verify_assets = true,
            "--verify-journey" => options.verify_journey = true,
            "--verify-first-hour" => options.verify_first_hour = true,
            "--verify-scene-quality" => options.verify_scene_quality = true,
            "--verify-loading" => options.verify_loading = true,
            "--verify-responsiveness" => options.verify_responsiveness = true,
            "--verify-unpresentable" => options.verify_unpresentable = true,
            "--settle-imports" => {
                options.settle = Some(match args.next().as_deref() {
                    Some("before") => ara_editor::Settle::Before,
                    Some("after") => ara_editor::Settle::After,
                    _ => return Err("--settle-imports needs before or after".into()),
                })
            }
            "--sample" => options.sample = true,
            "--screenshot" => options.screenshot = true,
            "--panel" => options.panel = Some(args.next().ok_or("--panel needs a panel name")?),
            "--play" => options.play = true,
            "--scene" => options.scene = Some(args.next().ok_or("--scene needs a scene path")?),
            "--select" => {
                options.select = Some(args.next().ok_or("--select needs entity addresses")?)
            }
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
                    "ara-editor <project> [--scene levels/x.scene] [--sample] [--headless] [--frames N] [--verify | --verify-panels | --verify-ux] [--size WxH] [--dpi SCALE] [--panel NAME] [--screenshot] [--output DIR]\n\nNative Vulkan editor + project Engine Protocol authority.\n--scene opens that scene instead of the project's main scene.\n--select a,b selects those entities once the scene is open.\n--play starts Play in the Game view once the window is up.\n--headless captures editor-scene.png without creating a native window, then exits.\n--sample creates an undoable sample scene if the scene is empty.\n--verify runs native/protocol interactions, screenshots and timings, then exits.\n--verify-panels runs real imports, panel interactions, both themes and a 500-node navigation benchmark.\n--verify-ux captures menus, context menus, the palette, Settings, a floating Inspector and every tool tab in both themes, then records idle and camera timings.
--verify-first-hour replays a new user's first hour on a fresh project (imports, Assets, Inspector, the tool pages, Play, Build), capturing each step.
--verify-assets proves right-click Create, C# inline rename, real compilation and entity attachment, with offscreen captures.
--verify-loading runs the loading window offscreen on the project and captures its milestones and every state.
--verify-docking runs native tab insertion, reorder, split, detach, reattach and Escape routes at UI scales 1 and 1.25, capturing every gesture.
--verify-responsiveness measures clicks, typing, drags, a Scene orbit, Play and startup through the window loop; --settle-imports before|after waits for imports before or after the interactions.
--verify-unpresentable holds the window unable to show (minimized or occluded) and checks the loop presents nothing, stays responsive and resumes presenting."
                );
                return Ok(());
            }
            _ if arg.starts_with('-') => return Err(format!("Unknown option {arg}").into()),
            _ => options.project = PathBuf::from(arg),
        }
    }
    // SAFETY: still single-threaded: nothing above starts a thread.
    unsafe { options.isolate_profile() };
    #[cfg(feature = "tracy")]
    let _tracy = ara_gpu::__tracy::Client::start();
    // Bounded runs never offer the crash folder or consume its "seen" marker.
    let _capture = match ara_crash::desktop::start("ara-editor", !options.offscreen()) {
        Ok(capture) => Some(capture),
        Err(error) => {
            eprintln!("ARA-EXP-CRASH-004: {error:#}");
            None
        }
    };
    ara_editor::run(options)
}
