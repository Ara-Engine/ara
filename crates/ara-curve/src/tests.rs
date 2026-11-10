use super::*;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn linear_and_constant_segments_follow_their_keys() {
    let line = Curve::linear(&[(0.0, 0.0), (2.0, 4.0), (3.0, 1.0)]);
    assert!(close(line.evaluate(1.0), 2.0));
    assert!(close(line.evaluate(2.5), 2.5));
    let mut hold = line.clone();
    hold.keys[0].interpolation = Interpolation::Constant;
    assert!(close(hold.evaluate(1.99), 0.0));
    assert!(
        close(hold.evaluate(2.0), 4.0),
        "the next key's value at its time"
    );
}

#[test]
fn presets_have_their_shapes() {
    for u in [0.0, 0.25, 0.5, 0.8, 1.0_f32] {
        let smooth = 3.0 * u * u - 2.0 * u * u * u;
        assert!(
            close(Preset::EaseInOut.curve().evaluate(u), smooth),
            "smoothstep at {u}"
        );
        assert!(
            close(Preset::EaseIn.curve().evaluate(u), u * u),
            "ease in at {u}"
        );
        assert!(
            close(Preset::EaseOut.curve().evaluate(u), 2.0 * u - u * u),
            "ease out at {u}"
        );
        assert!(close(Preset::Linear.curve().evaluate(u), u));
        assert!(close(Preset::Constant.curve().evaluate(u), 1.0));
    }
}

#[test]
fn auto_tangents_never_overshoot_and_level_out_at_peaks() {
    // Rising data with a sharp step: a plain Catmull-Rom would overshoot past 1.
    let mut c = Curve::empty();
    for (t, v) in [(0.0, 0.0), (1.0, 0.05), (1.2, 1.0), (3.0, 1.02)] {
        c.insert(t, v);
    }
    let (low, high) = c.value_range(64).expect("keys");
    assert!(low >= -1e-6 && high <= 1.02 + 1e-6, "{low}..{high}");
    // A peak is level: its slope is zero both ways.
    let peak = Curve {
        keys: vec![Key::new(0.0, 0.0), Key::new(1.0, 1.0), Key::new(2.0, 0.0)],
        ..Curve::empty()
    };
    assert_eq!(peak.slopes(1), (0.0, 0.0));
    assert!(peak.evaluate(1.0) >= peak.evaluate(0.9));
}

#[test]
fn hand_set_slopes_shape_the_curve_and_broken_ones_differ_per_side() {
    let mut c = Curve {
        keys: vec![Key::new(0.0, 0.0), Key::new(1.0, 0.0)],
        ..Curve::empty()
    };
    c.keys[0].tangent = Tangent::Free;
    c.keys[0].out_slope = 1.0;
    assert!(c.evaluate(0.1) > 0.0, "rises off a positive slope");
    c.keys[1].tangent = Tangent::Broken;
    c.keys[1].in_slope = 3.0;
    c.keys[1].out_slope = -7.0;
    assert_eq!(c.slopes(1), (3.0, -7.0));
    assert!(
        c.evaluate(0.9) < 0.0,
        "comes up into the last key from below"
    );
}

#[test]
fn outside_its_keys_a_curve_clamps_loops_or_mirrors() {
    let mut c = Curve::linear(&[(0.0, 0.0), (1.0, 1.0)]);
    assert!(close(c.evaluate(-3.0), 0.0) && close(c.evaluate(7.0), 1.0));
    c.after = Extrapolation::Loop;
    assert!(close(c.evaluate(2.25), 0.25));
    c.after = Extrapolation::PingPong;
    assert!(close(c.evaluate(1.25), 0.75));
    assert!(close(c.evaluate(2.25), 0.25));
    c.before = Extrapolation::Loop;
    assert!(close(c.evaluate(-0.25), 0.75));
    assert!(close(Curve::empty().evaluate(1.0), 0.0));
    assert!(close(Curve::constant(4.0).evaluate(-9.0), 4.0));
    assert!(
        close(c.evaluate(f32::NAN), 0.0),
        "a bad time reads the first key"
    );
}

#[test]
fn each_segment_s_polynomial_matches_evaluation() {
    let mut c = Curve::empty();
    for (t, v) in [(0.0, 0.2), (0.3, 1.0), (0.5, 0.4), (1.0, 0.9)] {
        c.insert(t, v);
    }
    c.keys[1].tangent = Tangent::Broken;
    c.keys[1].in_slope = 2.0;
    c.keys[1].out_slope = -1.0;
    c.keys[2].interpolation = Interpolation::Linear;
    for i in 0..c.keys.len() - 1 {
        let (a, b) = (c.keys[i].time, c.keys[i + 1].time);
        let [c0, c1, c2, c3] = c.polynomial(i);
        for u in [0.0, 0.3, 0.7, 0.99_f32] {
            let shader = c0 + u * (c1 + u * (c2 + u * c3));
            assert!(
                close(shader, c.evaluate(a + (b - a) * u)),
                "segment {i} at {u}"
            );
        }
    }
}

#[test]
fn keys_insert_and_move_in_order() {
    let mut c = Curve::linear(&[(0.0, 0.0), (1.0, 1.0)]);
    assert_eq!(c.insert(0.5, 9.0), 1);
    assert_eq!(
        c.keys[1].interpolation,
        Interpolation::Linear,
        "as the split segment"
    );
    assert_eq!(c.insert(0.5, 2.0), 1, "the key already there");
    assert_eq!(c.keys.len(), 3);
    assert!(close(c.keys[1].value, 2.0));
    let got = c.move_key(1, 5.0, 3.0);
    assert!(got < 1.0 && got > 0.99, "kept short of the next key: {got}");
    assert!(c.validate().is_ok());
    c.keys.push(Key::new(0.2, 0.0));
    assert!(c.validate().is_err(), "out of order");
    assert!(Curve::empty().validate().is_err());
}

#[test]
fn a_curve_saves_its_keys_and_reads_back_sparse_json() {
    let c = Preset::EaseIn.curve();
    let json = serde_json::to_string(&c).expect("serialises");
    assert_eq!(serde_json::from_str::<Curve>(&json).expect("reads"), c);
    assert_eq!(
        serde_json::to_string(&Curve::linear(&[(0.0, 1.0)])).expect("serialises"),
        r#"{"keys":[{"time":0.0,"value":1.0,"interpolation":"linear"}]}"#,
        "defaults are left out"
    );
    let sparse: Curve =
        serde_json::from_str(r#"{"keys":[{"time":0,"value":1},{"time":2,"value":3,"interpolation":"linear"}],"after":"pingPong"}"#)
            .expect("defaults fill the rest");
    assert_eq!(sparse.keys[0].tangent, Tangent::Auto);
    assert_eq!(sparse.after, Extrapolation::PingPong);
    assert!(close(sparse.evaluate(1.0), 2.0));
}

#[test]
fn the_schema_describes_a_curve() {
    let mut db = ara_schema::SchemaDb::new();
    db.register::<Curve>();
    assert!(db.registers::<Curve>());
    assert!(db.registers::<Key>());
}
