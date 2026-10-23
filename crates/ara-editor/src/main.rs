use std::path::PathBuf;
fn main() -> ara_ui::Result<()> {
    let mut options = ara_editor::Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--verify" => options.verify = true,
            "--sample" => options.sample = true,
            "--frames" => {
                options.frames = Some(args.next().ok_or("--frames needs a number")?.parse()?)
            }
            "--output" => {
                options.output = PathBuf::from(args.next().ok_or("--output needs a directory")?)
            }
            "--help" | "-h" => {
                println!(
                    "ara-editor <project> [--sample] [--frames N] [--verify] [--output DIR]\n\nNative Vulkan editor + project Engine Protocol authority.\n--sample creates an undoable sample scene if the scene is empty.\n--verify runs native/protocol interactions, screenshots and timings, then exits."
                );
                return Ok(());
            }
            _ if arg.starts_with('-') => return Err(format!("Unknown option {arg}").into()),
            _ => options.project = PathBuf::from(arg),
        }
    }
    ara_editor::run(options)
}
