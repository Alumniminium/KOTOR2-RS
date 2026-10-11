use super::*;

fn cmd(text: &str) -> Command {
    serde_json::from_str(text).expect("a command")
}

fn kreia(left: f32) -> Speech {
    Speech { speaker: "Kreia".to_owned(), left, total: left }
}

#[test]
fn a_call_waits_for_speech_except_stop_quit_and_info() {
    let speaking = kreia(2.0);
    assert!(must_wait(&cmd(r#"{"cmd":"choose_reply","index":0}"#), Some(&speaking)), "a reply waits for the line");
    assert!(must_wait(&cmd(r#"{"cmd":"move_to","point":[1.0,2.0,0.0]}"#), Some(&speaking)), "an action waits for the line");
    assert!(must_wait(&cmd(r#"{"cmd":"pick","x":1.0,"y":2.0}"#), Some(&speaking)), "pick moves the pointer, so it waits");
    for reads in [r#"{"cmd":"observe"}"#, r#"{"cmd":"inspect","object":5}"#, r#"{"cmd":"journal"}"#, r#"{"cmd":"list_saves"}"#, r#"{"cmd":"project","object":5}"#, r#"{"cmd":"screenshot"}"#] {
        assert!(must_wait(&cmd(reads), Some(&speaking)), "reads wait too while someone speaks: {reads}");
    }
    assert!(must_wait(&cmd(r#"{"cmd":"key_press","key":"KeyW"}"#), Some(&speaking)), "raw input waits");
    assert!(must_wait(&cmd(r#"{"cmd":"save","slot":"mcp-a"}"#), Some(&speaking)), "save waits");
    assert!(!must_wait(&cmd(r#"{"cmd":"stop"}"#), Some(&speaking)), "stop interrupts");
    assert!(!must_wait(&cmd(r#"{"cmd":"quit"}"#), Some(&speaking)));
    assert!(!must_wait(&cmd(r#"{"cmd":"info"}"#), Some(&speaking)), "game_status is never held");
    assert!(!must_wait(&cmd(r#"{"cmd":"choose_reply","index":0}"#), None), "nothing is spoken, nothing waits");
}

#[test]
fn a_held_call_is_capped_at_the_line_plus_the_pad() {
    assert_eq!(cap(10.0, &kreia(3.0)), 10.0 + 3.0 + HOLD_PAD);
    assert_eq!(cap(10.0, &kreia(0.0)), 10.0 + HOLD_PAD, "a line with no time left still allows the pad");
}

#[test]
fn a_known_line_counts_down_and_the_cap_follows_its_end() {
    // A 4 s line, measured when it starts: its count-down, then the end.
    let total = 4.0;
    assert_eq!(left_of(total, 0.0), Some(4.0));
    assert_eq!(left_of(total, 1.5), Some(2.5));
    assert_eq!(left_of(total, 4.0), None, "the line is over");
    // A call held 1.5 s into the line, at 10.0 s, may wait for the line's end plus the pad.
    let later = Speech { speaker: "Kreia".to_owned(), left: 2.5, total };
    assert_eq!(cap(10.0, &later), 10.0 + 2.5 + HOLD_PAD);
    // Held at the start, the cap is the whole line plus the pad.
    let first = Speech { speaker: "Kreia".to_owned(), left: 4.0, total };
    assert_eq!(cap(10.0, &first), 16.0);
}

#[test]
fn the_interim_frame_says_how_long_the_call_may_wait() {
    assert_eq!(notice("Kreia", 3.2), "held while Kreia speaks; the call runs when it ends (up to 3.2 s)");
}

#[test]
fn the_wait_note_and_the_overrun_refusal_name_the_speaker() {
    assert_eq!(wait_note("Kreia", 3.2), "waited 3.2 s for Kreia to finish speaking");
    assert_eq!(wait_note("the bark", 0.04), "waited 0.0 s for the bark to finish speaking");
    assert_eq!(overran("Kreia", 5.0), "refused: Kreia is still speaking after 5.0 s; the call was not run");
}

#[test]
fn a_wait_caps_the_line_on_screen_once_and_refreshes_for_the_next() {
    let mut wait = Wait::new(10.0, "Kreia", 10.0);
    wait.track(10.0, Some(&kreia(3.0)));
    assert_eq!(wait.cap, 15.0, "the line that starts sets the cap");
    wait.track(11.0, Some(&kreia(2.0)));
    assert_eq!(wait.cap, 15.0, "the same line does not move the cap, so a frozen line runs past it");
    assert!(!wait.overdue(15.0));
    assert!(wait.overdue(15.5), "past the cap a call is refused");
    wait.track(12.0, None);
    wait.track(14.0, Some(&kreia(4.0)));
    assert_eq!(wait.cap, 20.0, "the next line after a gap sets its own cap");
    assert_eq!(wait.started, 10.0, "the wait started when the call was held");
}

#[test]
fn a_dialog_wait_with_no_line_yet_ends_at_its_own_cap() {
    let wait = Wait::new(10.0, "Kreia", 10.0 + HOLD_PAD);
    assert!(!wait.overdue(11.5));
    assert!(wait.overdue(12.5), "no line came in time: the answer is given as it stands");
}

#[test]
fn the_dialog_notice_and_the_unsettled_note_say_what_is_waited_for() {
    assert_eq!(dialog_notice("Kreia", 3.2), "held until Kreia finishes and the replies are up; the answer follows (up to 3.2 s)");
    assert_eq!(unsettled(2.0), "stopped waiting after 2.0 s; the conversation may still be moving");
}
