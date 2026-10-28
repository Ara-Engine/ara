//! Selected local cages are cached gizmos, independent of Bounds/Collider toggles.
use super::*;

#[test]
fn selected_local_volume_cages_share_the_cache_and_follow_gizmo_visibility() {
    let view=view(vec![("e_volumebox1",json!({"Transform":{"scale":[2,4,6]},"PostProcessVolume":{"shape":"box","blendDistance":2.}}),None),
        ("e_volumesphere1",json!({"Transform":{"scale":[4,4,4]},"PostProcessVolume":{"shape":"sphere"}}),None),
        ("e_volumeglobal1",json!({"Transform":{},"PostProcessVolume":{"shape":"global"}}),None),
        ("e_volumefog1",json!({"Transform":{},"HeightFog":{}}),None)]);
    let mut state=state(false,false);let mut diagnostic=Diagnostics::default();let mut overlays=Vec::new();let theme=sheet();
    let key=SourceKey::default();
    diagnostic.append(&mut overlays,&view,&state,RECT,Source{scene:None,frame:None,key,current:false},&theme).unwrap();
    assert!(overlays.is_empty());
    state.selection=vec!["e_volumebox1".into()];
    diagnostic.append(&mut overlays,&view,&state,RECT,Source{scene:None,frame:None,key,current:false},&theme).unwrap();
    assert_eq!(diagnostic.wires.len(),12);assert_eq!(diagnostic.counts.volumes,1);
    let [min,max]=extent(&diagnostic.wires);assert!(min.abs_diff_eq(Vec3::new(-1.,-2.,-3.),1e-5));assert!(max.abs_diff_eq(Vec3::new(1.,2.,3.),1e-5));
    assert!(diagnostic.wires.iter().all(|wire|matches!(wire.tint,Tint::Volume)));
    assert_eq!(diagnostic.report()["selectedVolumes"][0]["blendDistance"],2.);
    let builds=(diagnostic.geometry_builds,diagnostic.projection_builds);overlays.clear();
    diagnostic.append(&mut overlays,&view,&state,RECT,Source{scene:None,frame:None,key,current:false},&theme).unwrap();
    assert_eq!((diagnostic.geometry_builds,diagnostic.projection_builds),builds);
    state.selection=vec!["e_volumesphere1".into()];overlays.clear();
    diagnostic.append(&mut overlays,&view,&state,RECT,Source{scene:None,frame:None,key,current:false},&theme).unwrap();
    assert_eq!(diagnostic.wires.len(),96);assert_eq!(diagnostic.counts.volumes,1);
    state.hide_gizmos=true;overlays.clear();
    diagnostic.append(&mut overlays,&view,&state,RECT,Source{scene:None,frame:None,key,current:false},&theme).unwrap();
    assert!(diagnostic.wires.is_empty());assert!(overlays.is_empty());
    state.hide_gizmos=false;
    for selected in ["e_volumeglobal1","e_volumefog1"] {
        state.selection=vec![selected.into()];overlays.clear();
        diagnostic.append(&mut overlays,&view,&state,RECT,Source{scene:None,frame:None,key,current:false},&theme).unwrap();
        assert!(diagnostic.wires.is_empty());assert_eq!(diagnostic.counts.volumes,0);
    }
}
