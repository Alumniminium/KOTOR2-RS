use super::*;

#[test]
fn defaults_roundtrip() {
    let d = GraphicsSettings::default();
    assert_eq!(GraphicsSettings::from_json(&d.to_json()), d);
    assert_eq!(d.shadows, ShadowQuality::High);
}

#[test]
fn missing_fields_keep_defaults() {
    let s = GraphicsSettings::from_json(r#"{"msaa": 8, "vsync": false}"#);
    assert_eq!(s.msaa, 8);
    assert!(!s.vsync);
    assert_eq!(s.anisotropy, 16);
    assert_eq!((s.width, s.height), (1600, 900));
}

#[test]
fn bad_values_fall_back_per_field() {
    let s = GraphicsSettings::from_json(
        r#"{"msaa": "lots", "shadows": "Low", "anisotropy": 5, "gamma": 900, "width": 10, "future_option": true}"#,
    );
    assert_eq!(s.msaa, 4);
    assert_eq!(s.shadows, ShadowQuality::Low);
    assert_eq!(s.anisotropy, 4);
    assert_eq!(s.gamma, 100);
    assert_eq!(s.width, MIN_WIDTH);
}

#[test]
fn garbage_gives_defaults() {
    assert_eq!(
        GraphicsSettings::from_json("not json"),
        GraphicsSettings::default()
    );
    assert_eq!(
        GraphicsSettings::from_json("[1,2]"),
        GraphicsSettings::default()
    );
}

#[test]
fn save_and_load_through_env_dir() {
    let dir = std::env::temp_dir().join("k2-settings-test");
    // SAFETY: only this test touches K2_SETTINGS_DIR.
    unsafe { std::env::set_var("K2_SETTINGS_DIR", &dir) };
    let mut s = GraphicsSettings::default();
    s.msaa = 2;
    s.fullscreen = true;
    s.shadows = ShadowQuality::Off;
    s.save();
    assert_eq!(GraphicsSettings::load(), s);
    unsafe { std::env::remove_var("K2_SETTINGS_DIR") };
}

#[test]
fn gamma_mapping() {
    let mut s = GraphicsSettings::default();
    assert!((s.gamma_value() - 1.0).abs() < 1e-6);
    s.gamma = 0;
    assert!(s.gamma_value() < 0.7);
    s.gamma = 100;
    assert!(s.gamma_value() > 1.5);
}

#[test]
fn mip_drop_is_bounded() {
    assert_eq!(drop_for(2, 1024, 1024, 11), 2);
    assert_eq!(drop_for(2, 128, 128, 8), 1);
    assert_eq!(drop_for(2, 64, 64, 7), 0);
    assert_eq!(drop_for(2, 1024, 1024, 2), 1);
}
