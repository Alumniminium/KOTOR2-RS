use super::*;

#[test]
fn axis_param_tracks_the_ray() {
    // A ray looking straight down at x = 3 meets the X axis line at 3.
    let t = axis_param(Vec3::ZERO, Vec3::X, Vec3::new(3.0, 0.5, 10.0), -Vec3::Z).unwrap();
    assert!((t - 3.0).abs() < 1e-4);
    // A ray parallel to the axis has no closest point.
    assert!(axis_param(Vec3::ZERO, Vec3::X, Vec3::new(0.0, 1.0, 0.0), Vec3::X).is_none());
}

#[test]
fn segment_distance_clamps_to_the_ends() {
    let (a, b) = (Vec2::ZERO, Vec2::new(10.0, 0.0));
    assert!((seg_dist(Vec2::new(5.0, 3.0), a, b) - 3.0).abs() < 1e-5);
    assert!((seg_dist(Vec2::new(13.0, 4.0), a, b) - 5.0).abs() < 1e-5);
    assert!((seg_dist(Vec2::new(1.0, 1.0), a, a) - 2f32.sqrt()).abs() < 1e-5);
}

#[test]
fn wrap_pi_takes_the_short_way() {
    assert!(
        (wrap_pi(3.0 * std::f32::consts::FRAC_PI_2) + std::f32::consts::FRAC_PI_2).abs() < 1e-5
    );
    assert!((wrap_pi(0.25) - 0.25).abs() < 1e-6);
}

#[test]
fn knob_sits_on_the_facing_side() {
    let p = Vec3::new(1.0, 2.0, 3.0);
    let k = knob_pos(p, 0.0, 4.0);
    // Bearing 0 faces +Y.
    assert!((k.x - 1.0).abs() < 1e-5 && (k.y - 5.0).abs() < 1e-5);
}

#[test]
fn palette_filter_combines_kind_and_text() {
    let mut ed = Editor::default();
    ed.catalog = vec![
        (Kind::Creature, "c_bantha".into(), "Bantha".into()),
        (Kind::Placeable, "plc_chair".into(), "Chair".into()),
        (Kind::Creature, "c_chair_droid".into(), "Droid".into()),
    ];
    ed.palette_search = "chair".into();
    assert_eq!(ed.filtered(), vec![1, 2]);
    ed.palette_kind = Some(Kind::Creature);
    assert_eq!(ed.filtered(), vec![2]);
}

#[test]
fn a_menu_request_is_ignored_while_the_editor_is_open() {
    let mut ed = Editor::default();
    ed.active = true;
    ed.request_from_menu();
    assert!(!ed.from_menu);
    ed.active = false;
    ed.request_from_menu();
    assert!(ed.from_menu);
}
