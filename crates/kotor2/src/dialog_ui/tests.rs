use super::*;

/// A line one second into a 5 s run, boxed or not.
fn line(boxed: bool) -> LineState {
    LineState { key: Some(("prsndoor".to_owned(), 0)), shown: 1.0, wait: 5.0, boxed, ..Default::default() }
}

#[test]
fn a_message_box_is_not_speech() {
    assert_eq!(line(false).speaking_left(), Some(4.0), "a spoken line counts down");
    assert_eq!(line(false).speaking_total(), Some(5.0));
    assert_eq!(line(true).speaking_left(), None, "a box waits for OK, not for its text time");
    assert_eq!(line(true).speaking_total(), None);
}
