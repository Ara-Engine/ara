//! Canonical documents omit default fields, so every reflected default must also
//! deserialize. Regression: the migrated Outpost scene writes a slot-0 material
//! override without `primitive`, which Play and the editor failed to read.
use ara_runtime::components::{MeshRenderer, PrimitiveMaterialOverride};
use serde_json::json;

#[test]
fn omitted_primitive_slot_reads_as_zero() {
    let slot: PrimitiveMaterialOverride =
        serde_json::from_value(json!({"material": "@materials/suit.graph#gr_suit"}))
            .expect("slot 0 may be omitted");
    assert_eq!(slot.primitive, 0);
}

#[test]
fn mesh_renderer_with_a_canonical_slot_zero_override_reads() {
    let renderer: MeshRenderer = serde_json::from_value(json!({
        "model": "@assets/human.glb#as_human",
        "materialOverrides": [{"material": "@materials/suit.graph#gr_suit"}]
    }))
    .expect("canonical MeshRenderer with an omitted slot reads");
    assert_eq!(renderer.material_overrides.len(), 1);
    assert_eq!(renderer.material_overrides[0].primitive, 0);
    let written = serde_json::to_value(&renderer.material_overrides[0]).expect("serializes");
    let reread: PrimitiveMaterialOverride = serde_json::from_value(written).expect("round trip");
    assert_eq!(reread, renderer.material_overrides[0]);
}
