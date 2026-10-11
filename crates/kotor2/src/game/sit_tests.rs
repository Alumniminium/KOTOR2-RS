use super::*;

#[test]
fn a_looping_sit_is_a_held_one_shot() {
    // The cantina sitters' ActionPlayAnimation(37, 1.0, -1.0).
    assert_eq!(scripted_playback(37, true, -1.0, 5.0), (false, Some(f32::INFINITY)));
    assert_eq!(scripted_playback(36, true, 0.0, 5.0), (false, Some(f32::INFINITY)));
    // A timed sit holds until its deadline.
    assert_eq!(scripted_playback(38, true, 3.0, 5.0), (false, Some(8.0)));
}

#[test]
fn other_animations_keep_their_flags() {
    assert_eq!(scripted_playback(0, true, -1.0, 5.0), (true, None));
    assert_eq!(scripted_playback(1, true, 5.0, 5.0), (true, Some(10.0)));
    assert_eq!(scripted_playback(100, false, 0.0, 5.0), (false, None));
    // Duration is ignored for fire-and-forget animations (nwscript).
    assert_eq!(scripted_playback(100, false, 2.0, 5.0), (false, None));
    assert_eq!(scripted_playback(101, false, 100.0, 5.0), (false, None));
}

#[test]
fn sit_clips_prefer_the_chair_sit() {
    // Female bodies reach sitdown through S_Male02, so one name serves both.
    assert_eq!(anim_clips(37), &["sitdown"]);
    assert_eq!(anim_clips(40), &["sitdown"]);
}
