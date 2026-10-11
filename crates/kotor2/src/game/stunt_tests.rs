use super::*;

#[test]
fn a_stunt_moved_off_its_stage_is_displaced() {
    let stage = Vec3::new(-24.4, 11.6, 9.1);
    assert!(!stunt_displaced(stage, stage));
    assert!(!stunt_displaced(stage + Vec3::new(0.0, 0.0, 0.05), stage));
    // a_cut_kreia jumps the get-up Kreia to MN_MEDLAB.
    assert!(stunt_displaced(Vec3::new(-8.6, -4.1, 9.06), stage));
}
