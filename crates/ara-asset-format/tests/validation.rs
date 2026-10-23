#![allow(clippy::unwrap_used)]
use ara_asset_format::*;

fn skeleton() -> Vec<u8> {
    let joint = Joint {
        parent: -1,
        node: 0,
        inverse_bind: [0.0; 16],
        rest: [0.0; 10],
    };
    write_skeleton(&[joint], &["root"]).unwrap()
}
#[test]
fn borrowed_readers_reject_every_truncation_and_bad_offsets() {
    let valid = skeleton();
    let parsed = Skeleton::read(&valid).unwrap();
    assert!(parsed.joints.as_ptr() >= valid.as_ptr());
    assert_eq!(parsed.joints.len(), 112);
    assert_eq!(parsed.joint_names().collect::<Vec<_>>(), ["root"]);
    for len in 0..valid.len() {
        assert!(Skeleton::read(&valid[..len]).is_err());
    }
    for offset in [4, 8, 12, 16, 24, 36, 40, 48] {
        let mut bad = valid.clone();
        bad[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Skeleton::read(&bad).is_err(), "offset {offset}");
    }
    let mut cyclic = valid.clone();
    let start = Container::read(&cyclic)
        .unwrap()
        .section(b"JNTS")
        .unwrap()
        .bytes
        .as_ptr() as usize
        - cyclic.as_ptr() as usize;
    cyclic[start..start + 4].copy_from_slice(&0u32.to_le_bytes());
    assert!(Skeleton::read(&cyclic).is_err());
}
#[test]
fn animation_span_overflow_and_nonmonotonic_times_are_rejected() {
    let ch = Channel {
        node: 0,
        path: 3,
        interpolation: 2,
        time_offset: 0,
        key_count: u32::MAX,
        value_offset: 0,
        components: u32::MAX,
        reserved: 0,
    };
    let mut b = Builder::new(Kind::Clip);
    b.section(*b"CHAN", 32, bytemuck::bytes_of(&ch).to_vec())
        .section(*b"TIME", 4, vec![])
        .section(*b"VALU", 4, vec![]);
    assert!(Clip::read(&b.finish().unwrap()).is_err());
    let ch = Channel {
        node: 0,
        path: 0,
        interpolation: 1,
        time_offset: 0,
        key_count: 2,
        value_offset: 0,
        components: 3,
        reserved: 0,
    };
    let mut b = Builder::new(Kind::Clip);
    b.section(*b"CHAN", 32, bytemuck::bytes_of(&ch).to_vec())
        .section(*b"TIME", 4, bytemuck::cast_slice(&[0.0f32, 0.0]).to_vec())
        .section(*b"VALU", 4, vec![0; 24]);
    assert!(Clip::read(&b.finish().unwrap()).is_err());
}
#[test]
fn ktx2_validates_npot_mip_sizes_and_dfd() {
    let levels = vec![vec![0; 32], vec![0; 16], vec![0; 16]];
    let bytes = write_ktx2(5, 3, TextureFormat::Bc7Srgb, &levels).unwrap();
    let texture = Texture::read(&bytes).unwrap();
    assert_eq!(texture.dimensions(), [5, 3]);
    assert_eq!(
        texture.levels().map(<[u8]>::len).collect::<Vec<_>>(),
        [32, 16, 16]
    );
    assert_eq!(texture.format(), TextureFormat::Bc7Srgb);
    assert!(write_ktx2(5, 3, TextureFormat::Bc7Srgb, &[vec![0; 16]]).is_err());
    for len in 0..bytes.len() {
        assert!(Texture::read(&bytes[..len]).is_err());
    }
}

#[test]
fn astc_plugin_outputs_use_the_same_validated_container_boundary() {
    for format in [
        TextureFormat::Astc4x4Srgb,
        TextureFormat::Astc6x6,
        TextureFormat::Astc8x8,
    ] {
        let side = format.block_extent();
        let bytes = write_ktx2(side, side, format, &[vec![0; 16]]).unwrap();
        assert_eq!(Texture::read(&bytes).unwrap().format(), format);
    }
}
