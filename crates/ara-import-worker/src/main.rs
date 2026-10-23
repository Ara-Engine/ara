//! Disposable below-normal-priority process hosting the builtin asset importers:
//! glTF/FBX/textures (`ara-importers`), 2D atlases/tilemaps (`ara-import2d`) and
//! audio (`ara-audio`'s cooker, adapted in `audio.rs`).
mod audio;

fn registry() -> ara_asset::Result<ara_asset::ImporterRegistry> {
    let mut registry = ara_importers::registry()?;
    registry.register(ara_import2d::AtlasImporter)?;
    registry.register(ara_import2d::TilemapImporter)?;
    registry.register(audio::AudioAsset)?;
    Ok(registry)
}

fn main() -> std::process::ExitCode {
    match registry().and_then(|registry| ara_asset::serve_worker(&registry)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
