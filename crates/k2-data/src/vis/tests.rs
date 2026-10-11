use super::*;

#[test]
fn tolerates_short_child_lists() {
    let v = Visibility::parse(b"rooma 3\r\n  roomb\r\n  roomc\r\nroomb 1\r\n  rooma\r\n");
    assert_eq!(v.rooms["rooma"].len(), 2);
    assert!(v.visible_from("ROOMB").contains("rooma"));
    assert!(v.visible_from("roomb").contains("roomb"));
}
