//! Size over Life holds a curve but saves its keys as effects always have.
use ara_vfx::Module;
use ara_vfx::ara_curve::{Curve, Interpolation};

#[test]
fn a_straight_size_curve_saves_as_it_always_did_and_a_shaped_one_adds_only_its_fields() {
    let saved = r#"{"op":"size_over_life","keys":[{"t":0.0,"value":0.25},{"t":1.0,"value":1.0}]}"#;
    let module: Module = serde_json::from_str(saved).expect("reads the old form");
    let Module::SizeOverLife { curve } = &module else {
        panic!("size over life");
    };
    assert_eq!(curve, &Curve::linear(&[(0.0, 0.25), (1.0, 1.0)]));
    assert_eq!(
        serde_json::to_string(&module).expect("saves"),
        saved,
        "unchanged"
    );
    let mut shaped = curve.clone();
    shaped.keys[0].interpolation = Interpolation::Cubic;
    let module = Module::SizeOverLife { curve: shaped };
    assert_eq!(
        serde_json::to_string(&module).expect("saves"),
        r#"{"op":"size_over_life","keys":[{"t":0.0,"value":0.25,"interpolation":"cubic"},{"t":1.0,"value":1.0}]}"#
    );
}

#[test]
fn the_gpu_lowering_of_a_shaped_curve_evaluates_it_segment_by_segment() {
    let mut curve = Curve::empty();
    for (t, v) in [(0.0, 0.1), (0.4, 0.6), (1.0, 0.2)] {
        curve.insert(t, v);
    }
    let effect = {
        let mut e = ara_vfx::samples::library().remove(0);
        for emitter in &mut e.emitters {
            for m in &mut emitter.stacks.update {
                if let Module::SizeOverLife { curve: c } = &mut m.module {
                    *c = curve.clone();
                }
            }
        }
        e
    };
    let source = ara_vfx::generate(&effect).expect("lowers");
    let [c0, c1, c2, c3] = curve.polynomial(0);
    let fmt = |x: f32| {
        let s = ara_json::format_f64(f64::from(x));
        if s.contains(['.', 'e', 'E']) {
            s
        } else {
            format!("{s}.0")
        }
    };
    assert!(
        source.contains(&format!(
            "p.size={}+u*({}+u*({}+u*{}))",
            fmt(c0),
            fmt(c1),
            fmt(c2),
            fmt(c3)
        )),
        "the first segment's polynomial"
    );
}
