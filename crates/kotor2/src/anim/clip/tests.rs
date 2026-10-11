use super::*;

#[test]
fn identity_quat_round_trip() {
    // x = y = 0 -> 1023, z = 0 -> 511, w = 1.
    let q = decompress_quat(1023 | (1023 << 11) | (511 << 22));
    assert!(q.w > 0.99, "{q:?}");
}

#[test]
fn bracket_interpolates() {
    let keys = [(0.0, 0.0f32), (1.0, 10.0)];
    let (a, b, f) = bracket(&keys, 0.25);
    assert_eq!((a, b), (0, 1));
    assert!((f - 0.25).abs() < 1e-6);
}
