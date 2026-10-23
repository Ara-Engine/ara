#![allow(clippy::unwrap_used)]
use ara_geo::{Geometry, ROOT, Settings, cook, sample};

fn verify(rings: u32, segments: u32) {
    let source = sample::rock(rings, segments);
    let a = cook(&source, Settings::default()).unwrap();
    let bytes = a.encode().unwrap();
    assert_eq!(
        bytes,
        cook(&source, Settings::default())
            .unwrap()
            .encode()
            .unwrap()
    );
    let b = Geometry::decode(&bytes).unwrap();
    assert!(b.groups.len() > 1);
    assert!(b.diagnostics.levels > 2);
    assert!(b.clusters.iter().any(|c| c.parent == ROOT && c.level > 0));
    for id in 0..b.clusters.len() {
        let page = b.cluster(id as u32).unwrap();
        assert!(page.vertices.len() <= 64 && page.triangles.len() <= 128 * 3);
    }
    let leaf_triangles: u64 = b
        .clusters
        .iter()
        .filter(|c| c.group == ROOT)
        .map(|c| u64::from(c.triangle_count))
        .sum();
    assert_eq!(leaf_triangles, source.indices.len() as u64 / 3);
    for n in [0, 4, 12, bytes.len() - 1] {
        assert!(Geometry::decode(&bytes[..n]).is_err());
    }
    println!(
        "{} tris, {} clusters, {} groups, {} bytes, {} levels",
        leaf_triangles,
        b.clusters.len(),
        b.groups.len(),
        bytes.len(),
        b.diagnostics.levels
    );
}
#[test]
fn deterministic_page_roundtrip_and_dag() {
    verify(48, 96);
}
#[test]
#[ignore = "explicit million-triangle cook gate"]
fn million_triangle_determinism() {
    verify(710, 710);
}

#[test]
fn invalid_input_is_diagnosed() {
    let mut s = sample::rock(4, 8);
    s.indices[0] = u32::MAX;
    assert!(cook(&s, Settings::default()).is_err());
}
