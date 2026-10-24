use anyhow::{Context, Result, bail};
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let option = args
        .next()
        .context("Use --destination <new directory> to install, or --list to inspect.")?;
    // Streamed from disk: only the trailer is read before hashing starts.
    let mut verified =
        ara_export_installer::Payload::open(&std::env::current_exe()?)?.verify(|_| {})?;
    if option == "--list" {
        for path in verified.names() {
            println!("{path}");
        }
    } else if option == "--destination" {
        let directory = args.next().context("--destination needs a new directory")?;
        anyhow::ensure!(args.next().is_none(), "unexpected extra arguments");
        ara_export_installer::install_verified(&mut verified, std::path::Path::new(&directory))?;
        println!("Installed. Launch ara-player.exe in the destination directory.");
    } else {
        bail!("Use --destination <new directory> or --list");
    }
    Ok(())
}
