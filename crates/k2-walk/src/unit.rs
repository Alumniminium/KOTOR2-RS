use super::*;

#[test]
fn z_on_triangle() {
    let v = [
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(2.0, 0.0, 1.0),
        Vec3::new(0.0, 2.0, 3.0),
    ];
    assert_eq!(tri_z_at(&v, Vec2::new(0.0, 1.0)), Some(2.0));
    assert_eq!(tri_z_at(&v, Vec2::new(2.0, 2.0)), None);
}

#[test]
fn crossing() {
    assert!(segs_cross(
        Vec2::ZERO,
        Vec2::new(2.0, 0.0),
        Vec2::new(1.0, -1.0),
        Vec2::new(1.0, 1.0)
    ));
    assert!(!segs_cross(
        Vec2::ZERO,
        Vec2::new(0.5, 0.0),
        Vec2::new(1.0, -1.0),
        Vec2::new(1.0, 1.0)
    ));
}
