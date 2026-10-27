//! Check the actual linked resource, which exported player copies retain.
use windows_sys::Win32::System::LibraryLoader::{
    FindResourceW, GetModuleHandleW, LoadResource, LockResource, SizeofResource,
};

#[test]
fn linked_player_resource_contains_the_committed_game_icon() -> anyhow::Result<()> {
    // Read at runtime so the expected PNGs cannot satisfy this test by being linked
    // into the test's ordinary data section.
    let ico = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/brand/icons/game/ara-game.ico"
    ))?;
    // SAFETY: The current executable module lives for this process; resource APIs
    // return read-only bytes retained by it. Integer pointers are Win32 resource IDs.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        assert!(!module.is_null());
        let resource = |id: u16, kind: u16| {
            let found = FindResourceW(
                module,
                id as usize as *const u16,
                kind as usize as *const u16,
            );
            assert!(!found.is_null(), "missing resource {kind}/{id}");
            let size = SizeofResource(module, found) as usize;
            let loaded = LoadResource(module, found);
            assert!(!loaded.is_null());
            let bytes = LockResource(loaded).cast::<u8>();
            assert!(!bytes.is_null());
            std::slice::from_raw_parts(bytes, size)
        };
        let group = resource(1, 14); // RT_GROUP_ICON
        assert_eq!(group[..6], ico[..6]);
        let count = u16::from_le_bytes(ico[4..6].try_into()?) as usize;
        for index in 0..count {
            let entry = &ico[6 + index * 16..6 + (index + 1) * 16];
            let linked = &group[6 + index * 14..6 + (index + 1) * 14];
            assert_eq!(linked[..12], entry[..12]);
            let id = u16::from_le_bytes(linked[12..14].try_into()?);
            let size = u32::from_le_bytes(entry[8..12].try_into()?) as usize;
            let offset = u32::from_le_bytes(entry[12..16].try_into()?) as usize;
            assert_eq!(resource(id, 3), &ico[offset..offset + size]); // RT_ICON
        }
    }
    Ok(())
}
