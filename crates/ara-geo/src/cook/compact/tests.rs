use super::*;

fn source(vertices: usize, indices: Vec<u32>) -> SourceMesh {
    SourceMesh {
        vertices: (0..vertices)
            .map(|i| Vertex {
                position: [i as f32, if i % 2 == 0 { -0. } else { 0. }, -(i as f32)],
                normal: [0., 0., 1.],
                uv: [i as f32 / 256., -0.],
            })
            .collect(),
        indices,
    }
}

fn exact(actual: (SourceMesh, Vec<u32>), expected: (SourceMesh, Vec<u32>)) {
    assert_eq!(actual.1, expected.1);
    assert_eq!(actual.0.indices, expected.0.indices);
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&actual.0.vertices),
        bytemuck::cast_slice::<_, u8>(&expected.0.vertices)
    );
}

#[test]
fn generation_reuse_preserves_first_seen_ids_and_all_vertex_bits() {
    let source = source(256, (0..2048).map(|i| (i * 37 % 256) as u32).collect());
    let mut remap = Remap::new(&source);
    assert_eq!(remap.slots.len(), 256);
    exact(
        remap.compact(&source.indices),
        sparse(&source, &source.indices),
    );
    for start in 0..256 {
        let indices: Vec<_> = (start..start + 100)
            .map(|i| (i * 13 % 256) as u32)
            .collect();
        exact(remap.compact(&indices), sparse(&source, &indices));
    }
    exact(remap.compact(&[]), sparse(&source, &[]));
    remap.generation = u32::MAX;
    let reversed: Vec<_> = source.indices.iter().rev().copied().collect();
    remap.slots[reversed[0] as usize] = Slot {
        generation: 1,
        local: u32::MAX,
    };
    exact(remap.compact(&reversed), sparse(&source, &reversed));
}

#[test]
fn sparse_public_inputs_do_not_allocate_source_sized_lookup_storage() {
    let source = source(4096, vec![4095, 3, 4095, 15, 3, 4095]);
    let mut remap = Remap::new(&source);
    assert!(remap.slots.is_empty());
    exact(
        remap.compact(&source.indices),
        sparse(&source, &source.indices),
    );
    let indices = [15, 4095, 15];
    exact(remap.compact(&indices), sparse(&source, &indices));
}
