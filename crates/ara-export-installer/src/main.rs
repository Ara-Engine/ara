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
    let bytes = std::fs::read(std::env::current_exe()?)?;
    if option == "--list" {
        for path in ara_export_installer::list(&bytes)? {
            println!("{path}");
        }
    } else if option == "--destination" {
        let directory = args.next().context("--destination needs a new directory")?;
        anyhow::ensure!(args.next().is_none(), "unexpected extra arguments");
        ara_export_installer::install(&bytes, std::path::Path::new(&directory))?;
        println!("Installed. Launch ara-player.exe in the destination directory.");
    } else {
        bail!("Use --destination <new directory> or --list");
    }
    Ok(())
}
