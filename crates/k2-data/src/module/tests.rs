use super::*;

#[test]
fn bearing_round_trip() {
    for (x, y) in [
        (0.0f32, 1.0f32),
        (1.0, 0.0),
        (-0.5858, -0.8105),
        (0.0, -1.0),
    ] {
        let b = bearing_from_dir(x, y);
        let d = dir_from_bearing(b);
        let n = (x * x + y * y).sqrt();
        assert!(
            (d[0] - x / n).abs() < 1e-4 && (d[1] - y / n).abs() < 1e-4,
            "{x},{y}"
        );
    }
}

#[test]
fn polygon_contains() {
    let sq = [
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [2.0, 2.0, 0.0],
        [0.0, 2.0, 0.0],
    ];
    assert!(point_in_polygon(&sq, 1.0, 1.0));
    assert!(!point_in_polygon(&sq, 3.0, 1.0));
}
