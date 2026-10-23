use ara_geo::{SourceMesh, Vertex, source_warnings};
#[test]
fn diagnoses_degenerate_sliver_broad_and_fragmented_sources() {
    let source = SourceMesh {
        vertices: [[0., 0., 0.], [1., 0., 0.], [1., 0.0001, 0.], [0., 1., 0.]]
            .map(|position| Vertex {
                position,
                normal: [0., 0., 1.],
                uv: [0.; 2],
            })
            .to_vec(),
        indices: vec![0, 0, 0, 0, 1, 2, 0, 1, 3],
    };
    let codes: Vec<_> = source_warnings(&source, 33)
        .iter()
        .map(|d| d.code)
        .collect();
    assert_eq!(
        codes,
        [
            "ARA_GEO_DEGENERATE",
            "ARA_GEO_THIN",
            "ARA_GEO_HUGE_TRIANGLES",
            "ARA_GEO_MANY_MATERIALS"
        ]
    );
}
