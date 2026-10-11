use super::*;

fn approx_eq(a: [f32; 4], b: [f32; 4], eps: f32) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < eps)
}

#[test]
fn identity_quaternion() {
    let aa = quat_to_axis_angle([1.0, 0.0, 0.0, 0.0]);
    assert_eq!(aa, [0.0, 0.0, 0.0, 0.0]);
}

#[test]
fn identity_axis_angle() {
    let q = axis_angle_to_quat([0.0, 0.0, 0.0, 0.0]);
    assert_eq!(q, [1.0, 0.0, 0.0, 0.0]);
}

#[test]
fn rotation_90_deg_around_z() {
    let angle = std::f32::consts::FRAC_PI_2; // 90 degrees
    let q = axis_angle_to_quat([0.0, 0.0, 1.0, angle]);

    let half = angle * 0.5;
    let expected = [half.cos(), 0.0, 0.0, half.sin()];
    assert!(approx_eq(q, expected, 1.0e-6));

    // Roundtrip
    let aa = quat_to_axis_angle(q);
    assert!(
        (aa[3] - angle).abs() < 1.0e-5,
        "angle mismatch: {} vs {}",
        aa[3],
        angle
    );
    assert!(
        (aa[2] - 1.0).abs() < 1.0e-5,
        "axis Z should be 1.0: {}",
        aa[2]
    );
}

#[test]
fn rotation_180_deg_around_x() {
    let angle = std::f32::consts::PI;
    let q = axis_angle_to_quat([1.0, 0.0, 0.0, angle]);

    let aa = quat_to_axis_angle(q);
    assert!(
        (aa[3] - angle).abs() < 1.0e-4,
        "angle: {} vs {}",
        aa[3],
        angle
    );
    assert!((aa[0] - 1.0).abs() < 1.0e-4, "axis X: {}", aa[0]);
}

#[test]
fn roundtrip_arbitrary_rotation() {
    let original = [0.5, 0.5, 0.5, 0.5]; // 120 deg around [1,1,1]
    let aa = quat_to_axis_angle(original);
    let recovered = axis_angle_to_quat(aa);

    // Quaternions q and -q represent the same rotation.
    let same = approx_eq(original, recovered, 1.0e-5);
    let negated = approx_eq(
        original,
        [-recovered[0], -recovered[1], -recovered[2], -recovered[3]],
        1.0e-5,
    );
    assert!(
        same || negated,
        "roundtrip failed: {original:?} -> {aa:?} -> {recovered:?}"
    );
}

#[test]
fn near_zero_rotation() {
    // Very small rotation -- should not produce NaN.
    let q = [0.9999999, 0.0000001, 0.0, 0.0];
    let aa = quat_to_axis_angle(q);
    assert!(aa.iter().all(|v| v.is_finite()), "NaN in result: {aa:?}");
}

#[test]
fn unnormalized_quaternion() {
    // Scale of [0.5, 0.5, 0.5, 0.5] * 2
    let q = [1.0, 1.0, 1.0, 1.0];
    let aa = quat_to_axis_angle(q);
    assert!(aa.iter().all(|v| v.is_finite()), "NaN in result: {aa:?}");

    // Should produce same result as normalized version.
    let aa_norm = quat_to_axis_angle([0.5, 0.5, 0.5, 0.5]);
    assert!(approx_eq(aa, aa_norm, 1.0e-5));
}
