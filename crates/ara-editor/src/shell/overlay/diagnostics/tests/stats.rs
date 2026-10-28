use super::*;
use std::cell::Cell;

#[test]
fn sampler_is_lazy_at_four_hz_and_unknown_metrics_never_turn_into_zero() {
    let mut stats=super::super::stats::Stats::default();let calls=Cell::new(0);
    let sample=||{calls.set(calls.get()+1);StatsSample{cpu_loop_ms:Some(0.),render_instances:Some(0),..Default::default()}};
    assert!(stats.observe(Duration::ZERO,true,sample));
    for _ in 0..10 {assert!(!stats.observe(Duration::from_millis(16),true,sample));}
    assert_eq!(calls.get(),1);stats.observe(Duration::from_millis(100),true,sample);assert_eq!(calls.get(),2);
    stats.observe(Duration::from_secs(1),false,sample);assert_eq!(calls.get(),2);
    let mut markup=String::new();stats.markup(Flags{stats:true,..Default::default()},&Counts::default(),true,Some([180.,120.]),&mut markup);
    let mut text=ara_ui_text::TextSystem::bundled().unwrap();
    let mut ui=ara_ui::Ui::editor(&format!("<ui id='root'>{markup}</ui>"),"",[180.,120.]).unwrap();ui.frame(&mut text).unwrap();
    let panel=&ui.nodes[ui.find("scene-diagnostic-stats").unwrap()];
    assert!(panel.rect.x>=0. && panel.rect.y>=0. && panel.rect.x+panel.rect.w<=180.1 && panel.rect.y+panel.rect.h<=120.1);
    let values=ui.nodes.iter().filter_map(|node|node.text.as_ref().map(|text|text.value.as_str())).collect::<Vec<_>>();
    assert!(values.iter().any(|value|value.contains("CPU loop wall: 0.00 ms")));
    assert!(values.iter().any(|value|value.contains("GPU longest pass: unavailable")));
    assert!(values.iter().any(|value|value.contains("Native instances: 0")));
}

#[test]
fn nonfinite_timing_samples_are_explicitly_unknown_and_cooked_shapes_have_visible_notice() {
    let mut stats=super::super::stats::Stats::default();
    stats.observe(Duration::ZERO,true,||StatsSample{cpu_loop_ms:Some(f64::NAN),gpu_longest_pass_ms:Some(f64::INFINITY),..Default::default()});
    assert_eq!(stats.report()["sample"]["cpuLoopMs"],Value::Null);assert_eq!(stats.report()["sample"]["gpuLongestPassMs"],Value::Null);
    let mut markup=String::new();stats.markup(Flags{colliders:true,..Default::default()},&Counts{unsupported_colliders:3,..Default::default()},true,Some([400.,300.]),&mut markup);
    let elements=ara_ui::source::compile(&format!("<ui>{markup}</ui>")).unwrap();
    assert!(elements.iter().any(|element|element.attrs.get("text").is_some_and(|text|text.contains("3 cooked/model collider shapes unavailable"))));
}
