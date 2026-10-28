//! Application identity is applied to the copied player before code signing.
#[cfg(any(windows, test))]
use crate::layout;
use crate::{ExportPreset, Platform, tools};
use anyhow::Result;
#[cfg(any(windows, test))]
use anyhow::ensure;
use std::path::Path;

pub(crate) fn apply(
    project: &Path,
    staged: &Path,
    name: &str,
    preset: &ExportPreset,
) -> Result<()> {
    let icon = preset
        .icon
        .as_ref()
        .map(|path| tools::authored_path(project, path))
        .transpose()?;
    match preset.platform {
        Platform::Windows => {
            let icon = icon.map(std::fs::read).transpose()?;
            if let Some(icon) = &icon {
                let png = window_icon(icon)?;
                std::fs::write(staged.join("icon.png"), png)?;
            }
            windows(
                &staged.join("ara-player.exe"),
                name,
                &preset.version,
                icon.as_deref(),
            )?;
        }
        Platform::Macos => {
            if let Some(icon) = icon {
                tools::copy(&icon, &staged.join(crate::macos::ICON))?;
            }
        }
        Platform::Linux => {
            if let Some(icon) = icon {
                tools::copy(&icon, &staged.join("icon.svg"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn window_icon(ico: &[u8]) -> Result<Vec<u8>> {
    use anyhow::Context;
    let image = image::load_from_memory_with_format(ico, image::ImageFormat::Ico)
        .context("ARA-EXP-BRAND-001: cannot decode the game ICO")?;
    let mut png = std::io::Cursor::new(Vec::new());
    image
        .resize(256, 256, image::imageops::FilterType::Lanczos3)
        .write_to(&mut png, image::ImageFormat::Png)
        .context("ARA-EXP-BRAND-001: cannot encode the game window icon")?;
    Ok(png.into_inner())
}

#[cfg(not(windows))]
fn windows(_player: &Path, _name: &str, _version: &str, _icon: Option<&[u8]>) -> Result<()> {
    anyhow::bail!(
        "ARA-EXP-BRAND-001: Windows game resources must be stamped on Windows before signing"
    )
}

#[cfg(windows)]
fn windows(player: &Path, name: &str, version: &str, icon: Option<&[u8]>) -> Result<()> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::System::LibraryLoader::{
        BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW,
    };
    // A signed engine template becomes a different application. Remove its old
    // certificate before stamping; the export's optional signing runs afterward.
    let mut image = std::fs::read(player)?;
    if clear_certificate(&mut image)? {
        std::fs::write(player, image)?;
    }
    struct Update(*mut c_void);
    impl Drop for Update {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: This guard owns the live resource update; discard on failure.
                unsafe {
                    EndUpdateResourceW(self.0, 1);
                }
            }
        }
    }
    let path: Vec<_> = player.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `path` is a nul-terminated UTF-16 filename, retained for the call.
    let handle = unsafe { BeginUpdateResourceW(path.as_ptr(), 0) };
    ensure!(
        !handle.is_null(),
        "ARA-EXP-BRAND-001: cannot update game executable resources: {}",
        std::io::Error::last_os_error()
    );
    let mut update = Update(handle);
    let write = |kind: u16, id: u16, bytes: &[u8]| -> Result<()> {
        let size = u32::try_from(bytes.len())?;
        // SAFETY: Win32 integer resource IDs are encoded as pointers; the owned
        // update handle is live and `bytes` remains readable throughout the call.
        let ok = unsafe {
            UpdateResourceW(
                update.0,
                kind as usize as *const u16,
                id as usize as *const u16,
                0x0409,
                bytes.as_ptr().cast(),
                size,
            )
        };
        ensure!(
            ok != 0,
            "ARA-EXP-BRAND-001: cannot write game resource: {}",
            std::io::Error::last_os_error()
        );
        Ok(())
    };
    write(16, 1, &version_info(name, version)?)?;
    if let Some(icon) = icon {
        let (group, images) = icon_resources(icon)?;
        for (index, image) in images.into_iter().enumerate() {
            write(3, u16::try_from(index + 1)?, image)?;
        }
        write(14, 1, &group)?;
    }
    // Clear the guard before committing: EndUpdateResource closes on failure too.
    let handle = std::mem::replace(&mut update.0, std::ptr::null_mut());
    // SAFETY: The live update transaction is owned and consumed exactly once.
    let committed = unsafe { EndUpdateResourceW(handle, 0) };
    ensure!(
        committed != 0,
        "ARA-EXP-BRAND-001: cannot commit game resources: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}

#[cfg(any(windows, test))]
fn clear_certificate(image: &mut Vec<u8>) -> Result<bool> {
    let offset = image
        .get(0x3c..0x40)
        .ok_or_else(|| anyhow::anyhow!("ARA-EXP-BRAND-001: truncated PE header"))?;
    let pe = u32::from_le_bytes(offset.try_into()?) as usize;
    ensure!(
        pe <= image.len().saturating_sub(24),
        "ARA-EXP-BRAND-001: PE header outside image"
    );
    ensure!(
        image.get(pe..pe + 4) == Some(b"PE\0\0"),
        "ARA-EXP-BRAND-001: invalid PE image"
    );
    let optional = pe + 24;
    let magic = image
        .get(optional..optional + 2)
        .ok_or_else(|| anyhow::anyhow!("ARA-EXP-BRAND-001: missing PE optional header"))?;
    let directory = match u16::from_le_bytes(magic.try_into()?) {
        0x20b => optional + 112,
        0x10b => optional + 96,
        _ => anyhow::bail!("ARA-EXP-BRAND-001: unsupported PE optional header"),
    };
    let certificate = directory + 4 * 8;
    let entry = image
        .get(certificate..certificate + 8)
        .ok_or_else(|| anyhow::anyhow!("ARA-EXP-BRAND-001: missing PE security directory"))?;
    let offset = u32::from_le_bytes(entry[..4].try_into()?) as usize;
    let size = u32::from_le_bytes(entry[4..].try_into()?) as usize;
    if offset == 0 && size == 0 {
        return Ok(false);
    }
    let end = offset
        .checked_add(size)
        .ok_or_else(|| anyhow::anyhow!("ARA-EXP-BRAND-001: invalid PE certificate range"))?;
    ensure!(
        offset >= certificate + 8 && size > 0 && end <= image.len(),
        "ARA-EXP-BRAND-001: invalid PE certificate range"
    );
    image[certificate..certificate + 8].fill(0);
    image[optional + 64..optional + 68].fill(0); // PE checksum is recomputed by signing.
    if end == image.len() {
        image.truncate(offset);
    }
    Ok(true)
}

#[cfg(any(windows, test))]
fn icon_resources(icon: &[u8]) -> Result<(Vec<u8>, Vec<&[u8]>)> {
    ensure!(
        icon.len() >= 6 && icon[..4] == [0, 0, 1, 0],
        "ARA-EXP-BRAND-001: invalid ICO header"
    );
    let count = usize::from(u16::from_le_bytes([icon[4], icon[5]]));
    ensure!(
        (1..=256).contains(&count) && icon.len() >= 6 + count * 16,
        "ARA-EXP-BRAND-001: invalid ICO directory"
    );
    let mut group = icon[..6].to_vec();
    let mut images = Vec::new();
    for index in 0..count {
        let entry = &icon[6 + index * 16..6 + (index + 1) * 16];
        let size = u32::from_le_bytes(entry[8..12].try_into()?) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into()?) as usize;
        let end = offset
            .checked_add(size)
            .ok_or_else(|| anyhow::anyhow!("ARA-EXP-BRAND-001: ICO image overflow"))?;
        let image = icon
            .get(offset..end)
            .filter(|bytes| !bytes.is_empty())
            .ok_or_else(|| anyhow::anyhow!("ARA-EXP-BRAND-001: ICO image outside file"))?;
        group.extend_from_slice(&entry[..12]);
        group.extend_from_slice(&u16::try_from(index + 1)?.to_le_bytes());
        images.push(image);
    }
    Ok((group, images))
}

#[cfg(any(windows, test))]
fn version_info(name: &str, version: &str) -> Result<Vec<u8>> {
    fn utf16(value: &str) -> Vec<u8> {
        value
            .encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect()
    }
    fn block(key: &str, value: &[u8], text: bool, children: &[Vec<u8>]) -> Result<Vec<u8>> {
        let mut bytes = vec![0, 0];
        bytes.extend_from_slice(
            &u16::try_from(if text { value.len() / 2 } else { value.len() })?.to_le_bytes(),
        );
        bytes.extend_from_slice(&u16::from(text).to_le_bytes());
        bytes.extend(utf16(key));
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
        bytes.extend_from_slice(value);
        for child in children {
            while !bytes.len().is_multiple_of(4) {
                bytes.push(0);
            }
            bytes.extend_from_slice(child);
        }
        let length = u16::try_from(bytes.len())?;
        bytes[..2].copy_from_slice(&length.to_le_bytes());
        Ok(bytes)
    }
    let [major, minor, patch, revision] = layout::version_parts(version)?;
    let ms = (u32::from(major) << 16) | u32::from(minor);
    let ls = (u32::from(patch) << 16) | u32::from(revision);
    let fixed: Vec<_> = [
        0xfeef04bd_u32,
        0x00010000,
        ms,
        ls,
        ms,
        ls,
        0x3f,
        0,
        0x00040004,
        1,
        0,
        0,
        0,
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    let filename = format!("{name}.exe");
    let strings = [
        ("FileDescription", name),
        ("ProductName", name),
        ("FileVersion", version),
        ("ProductVersion", version),
        ("OriginalFilename", filename.as_str()),
        ("InternalName", name),
    ]
    .into_iter()
    .map(|(key, value)| block(key, &utf16(value), true, &[]))
    .collect::<Result<Vec<_>>>()?;
    let table = block("040904b0", &[], true, &strings)?;
    let strings = block("StringFileInfo", &[], true, &[table])?;
    let translation = block("Translation", &[0x09, 0x04, 0xb0, 0x04], false, &[])?;
    let vars = block("VarFileInfo", &[], true, &[translation])?;
    block("VS_VERSION_INFO", &fixed, false, &[strings, vars])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resources_include_the_game_identity_and_complete_icon_images() {
        let version = version_info("Outpost", "1.2.3").expect("version resource");
        assert_eq!(
            usize::from(u16::from_le_bytes([version[0], version[1]])),
            version.len()
        );
        let text = String::from_utf16_lossy(
            &version
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes(*b))
                .collect::<Vec<_>>(),
        );
        for value in ["Outpost", "1.2.3", "OriginalFilename", "Outpost.exe"] {
            assert!(text.contains(value));
        }
        let icon = include_bytes!("../../../assets/brand/icons/game/ara-game.ico");
        let (group, images) = icon_resources(icon).expect("game icon");
        assert_eq!(group.len(), 6 + images.len() * 14);
        assert!(images.iter().all(|image| !image.is_empty()));
        assert!(icon_resources(&icon[..8]).is_err());
        let png = window_icon(icon).expect("window icon");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn branding_removes_the_templates_signature_before_the_game_is_signed() {
        let mut image = vec![0; 520];
        image[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        image[64..68].copy_from_slice(b"PE\0\0");
        image[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
        image[232..236].copy_from_slice(&512u32.to_le_bytes());
        image[236..240].copy_from_slice(&8u32.to_le_bytes());
        assert!(clear_certificate(&mut image).expect("remove template signature"));
        assert_eq!(image.len(), 512);
        assert_eq!(&image[232..240], &[0; 8]);
        assert!(!clear_certificate(&mut image).expect("already unsigned"));
    }

    #[test]
    #[cfg(windows)]
    fn copied_pe_contains_the_game_icon_and_version_without_running_it() -> Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{
            Foundation::FreeLibrary,
            System::LibraryLoader::{
                FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LoadLibraryExW, LoadResource,
                LockResource, SizeofResource,
            },
        };
        let scratch = tempfile::tempdir()?;
        let player = scratch.path().join("Outpost.exe");
        std::fs::copy(std::env::current_exe()?, &player)?;
        let ico = include_bytes!("../../../assets/brand/icons/game/ara-game.ico");
        windows(&player, "Outpost", "4.2.1", Some(ico))?;
        let path: Vec<_> = player.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: The test maps its own copied PE as data only. Resource slices
        // are consumed before freeing the module, and the image is never executed.
        unsafe {
            let module = LoadLibraryExW(
                path.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_AS_DATAFILE,
            );
            ensure!(!module.is_null(), "load copied resource image");
            let read = |kind: u16| -> Result<&[u8]> {
                // MAKEINTRESOURCE: integer resource IDs travel in the pointer arguments.
                let resource = FindResourceW(
                    module,
                    std::ptr::without_provenance(1),
                    std::ptr::without_provenance(usize::from(kind)),
                );
                ensure!(!resource.is_null(), "missing resource {kind}");
                let loaded = LoadResource(module, resource);
                ensure!(!loaded.is_null(), "load resource {kind}");
                let bytes = LockResource(loaded).cast::<u8>();
                ensure!(!bytes.is_null(), "lock resource {kind}");
                Ok(std::slice::from_raw_parts(
                    bytes,
                    SizeofResource(module, resource) as usize,
                ))
            };
            let result = (|| -> Result<()> {
                assert_eq!(read(16)?, version_info("Outpost", "4.2.1")?);
                assert_eq!(read(14)?, icon_resources(ico)?.0);
                Ok(())
            })();
            FreeLibrary(module);
            result?;
        }
        Ok(())
    }
}
