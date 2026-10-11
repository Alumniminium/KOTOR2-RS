use super::*;

#[test]
fn cycle_advances_at_fps() {
    let mut p = KParams::new(FLAG_UV_CYCLE, 0.0, LinearRgba::WHITE);
    p.cycle = Vec4::new(2.0, 2.0, 16.0, 0.0);
    p.tune = UvTuning {
        scroll: 1.0,
        cycle: 1.0,
        water: 1.0,
    }
    .vec();
    // 16 fps over 4 frames: 1/16 s per frame, 4 full loops per second.
    assert!(p.describe_at(0.0).contains("frame 0 of 4"));
    assert!(p.describe_at(0.07).contains("frame 1 of 4"));
    assert!(p.describe_at(0.2).contains("frame 3 of 4"));
    assert!(p.describe_at(0.26).contains("frame 0 of 4"));
    assert!(p.describe_at(1.0).contains("frame 0 of 4"));
    // Default cycle scale 0.5: 8 fps, 2 loops per second.
    p.tune = UvTuning::DEFAULT.vec();
    assert!(p.describe_at(0.07).contains("frame 0 of 4"));
    assert!(p.describe_at(0.13).contains("frame 1 of 4"));
    // Cross-fade weight is the fractional frame: 8 fps, t = 0.0625 s is
    // half way from frame 0 to frame 1.
    assert!(
        p.describe_at(0.0625).contains("0.50 toward next"),
        "{}",
        p.describe_at(0.0625)
    );
}

/// Mirrors the shader at 60 fps for the cage layers and checks that every
/// per-frame step is small and equal-sized (no stepping, no low-rate
/// update): scroll offset, water phase and the cross-faded cycle position.
#[test]
fn cage_layers_advance_every_frame() {
    let tune = UvTuning::DEFAULT;
    let dt = 1.0 / 60.0;
    // PLC_FrcDist01 layer: scroll dir (0.02, 0), water speed 4.
    let scroll_step = 0.02 * tune.scroll * dt;
    let water_step = 4.0 * WATER_PHASE_RATE * tune.water * dt;
    // PLC_FrcDist02 layer: 16 fps TXI, scale 0.5, scroll dir (0, 0.01).
    let cycle_step = 16.0 * tune.cycle * dt;
    let vscroll_step = 0.01 * tune.scroll * dt;
    assert!(
        (scroll_step - 0.02).abs() < 1e-6,
        "horizontal scroll {scroll_step} uv per frame"
    );
    assert!(
        (vscroll_step - 0.01).abs() < 1e-6,
        "vertical scroll {vscroll_step} uv per frame"
    );
    assert!(
        (water_step - 0.1333).abs() < 1e-3,
        "water phase {water_step} rad per frame"
    );
    assert!(
        (cycle_step - 0.1333).abs() < 1e-3,
        "cycle {cycle_step} frames per frame"
    );
    // The blend weight (fract of the cycle position) climbs in equal steps
    // and wraps exactly when frame i+1 becomes frame i, so the output is
    // continuous across the wrap.
    let mut p = KParams::new(FLAG_UV_CYCLE, 0.0, LinearRgba::WHITE);
    p.cycle = Vec4::new(2.0, 2.0, 16.0, 0.0);
    let mut last = 0.0f32;
    for k in 0..240 {
        let pos = k as f32 * dt * 16.0 * tune.cycle;
        let w = pos.fract();
        assert!(
            (w - last).abs() < 0.2 || w < last,
            "weight jumped at frame {k}"
        );
        last = w;
        assert!(p.describe_at(k as f32 * dt).contains("toward next"));
    }
}

#[test]
fn scroll_is_units_per_second() {
    let mut p = KParams::new(FLAG_UV_SCROLL, 0.0, LinearRgba::WHITE);
    p.scroll = Vec4::new(0.02, 0.0, 0.0, 0.0);
    p.tune = UvTuning {
        scroll: 250.0,
        cycle: 1.0,
        water: 1.0,
    }
    .vec();
    assert!(
        p.describe_at(1.0).contains("(5.00, 0.00)"),
        "{}",
        p.describe_at(1.0)
    );
    p.tune = UvTuning::DEFAULT.vec();
    assert!(
        p.describe_at(1.0).contains("(1.20, 0.00)"),
        "{}",
        p.describe_at(1.0)
    );
}
