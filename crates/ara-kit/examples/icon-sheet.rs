//! Draws every icon in a labelled grid to `target/ara-kit/icons.png` (a visual check of
//! `gen-icons`). `cargo run -p ara-kit --example icon-sheet`.
use stillkit::el::{El, Runtime, col, grid, icon, text};
use stillkit::{Rgba, Scheme, TextRole, Theme};

fn view(_: &()) -> El<()> {
    grid(12)
        .gap(6.)
        .pad(12.)
        .children(ara_kit::icons::ALL.iter().map(|(name, i)| {
            col().gap(4.).align(stillkit::el::Align::Center).children([
                icon(*i).size(24.).color(Rgba::rgb8([236, 236, 240], 1.)),
                text(*name)
                    .font(TextRole::Caption)
                    .color(Rgba::rgb8([140, 140, 150], 1.)),
            ])
        }))
}

fn main() -> anyhow::Result<()> {
    let rows = ara_kit::icons::ALL.len().div_ceil(12) as f32;
    let mut rt = Runtime::new(
        (),
        view,
        Theme::new(
            stillkit::Style::Solid,
            Scheme::Dark,
            Rgba::rgb8([52, 107, 241], 1.),
        ),
        (1300, (rows * 62. + 24.) as u32),
        1.,
    )?;
    rt.set_background(Some(Rgba::rgb8([10, 10, 10], 1.)));
    rt.frame(std::time::Duration::ZERO);
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-kit/icons.png");
    std::fs::create_dir_all(out.parent().expect("parent"))?;
    std::fs::write(&out, ara_kit::capture::png(&rt.snap()?)?)?;
    println!("{}", out.display());
    Ok(())
}
