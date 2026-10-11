//! Unit tests for the dialog runtime's private helpers.

use super::alien_row;

#[test]
fn braces_are_stripped() {
    assert_eq!(super::strip_braces("{Sarcastic}Hello {x}there"), "Hello there");
    assert_eq!(super::strip_braces("{Fade out}"), "");
}

/// Owner row 12 belongs to dialog owner 7; the speaker decides whether it
/// applies. An unresolved named speaker (None) is not the owner.
#[test]
fn alien_row_only_for_the_owner_speaking() {
    assert_eq!(alien_row(0, 12, Some(7), 7), 12, "empty tag or owner tag");
    assert_eq!(alien_row(0, 12, Some(8), 7), 0, "named non-owner");
    assert_eq!(alien_row(0, 12, None, 7), 0, "named speaker that does not resolve");
    assert_eq!(alien_row(0, 0, Some(7), 7), 0, "human owner");
    assert_eq!(alien_row(5, 12, Some(8), 7), 5, "the node's own row always wins");
    assert_eq!(alien_row(5, 12, None, 7), 5, "the node's own row, unresolved speaker");
}
