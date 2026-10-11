//! Dialog link logic and the bare-line (bark) rule, through the public API.

use k2_data::dlg::{Condition, Link, Node};
use k2_data::Dialog;

fn cond(name: &str, not: bool) -> Option<Condition> {
    Some(Condition { resref: name.into(), not, ..Default::default() })
}

#[test]
fn link_logic() {
    let mut eval = |c: &Condition| c.resref == "yes";
    let l = Link { active: cond("yes", false), active2: cond("no", false), ..Default::default() };
    assert!(!l.passes(&mut eval));
    let l = Link { logic: 1, ..l };
    assert!(l.passes(&mut eval));
    let l = Link { active: cond("no", true), ..Default::default() };
    assert!(l.passes(&mut eval));
    assert!(Link::default().passes(&mut eval));
}

#[test]
fn bare_entries_are_one_liners_unless_they_are_cutscenes() {
    let bare = Node::default();
    let dlg = Dialog { entries: vec![bare.clone()], ..Default::default() };
    assert!(dlg.is_one_liner(0), "no reply links, no cutscene flags");
    assert!(!dlg.is_one_liner(1), "an entry that does not exist");

    let linked = Node { links: vec![Link::default()], ..Default::default() };
    let dlg = Dialog { entries: vec![linked], replies: vec![Node::default()], ..Default::default() };
    assert!(!dlg.is_one_liner(0), "a [Continue] reply link makes a conversation");

    let dlg = Dialog { entries: vec![Node { camera_angle: 4, ..Default::default() }], ..Default::default() };
    assert!(!dlg.is_one_liner(0), "CameraAngle 4 runs an animated camera, not a bark");

    let dlg = Dialog { entries: vec![Node { fade_type: 3, ..Default::default() }], ..Default::default() };
    assert!(!dlg.is_one_liner(0), "a node fade is a cutscene line");

    let dlg = Dialog { entries: vec![bare.clone()], camera_model: Some("cam".into()), ..Default::default() };
    assert!(!dlg.is_one_liner(0), "a dialog with a camera model is a cutscene");

    let dlg = Dialog { entries: vec![bare], animated_cut: true, ..Default::default() };
    assert!(!dlg.is_one_liner(0), "AnimatedCut dialogs are cutscenes");
}

/// The conditions `passes` evaluates, in order, with `value` as every result.
fn evaluated(link: &Link, value: bool) -> (bool, Vec<String>) {
    let mut seen = Vec::new();
    let passes = link.passes(&mut |c: &Condition| {
        seen.push(c.resref.clone());
        value
    });
    (passes, seen)
}

#[test]
fn and_with_a_false_active_never_evaluates_active2() {
    let link = Link { active: cond("a", false), active2: cond("b", false), logic: 0, ..Default::default() };
    let (passes, seen) = evaluated(&link, false);
    assert!(!passes);
    assert_eq!(seen, vec!["a"], "active2 is not evaluated when active decides the AND");
}

#[test]
fn or_with_a_true_active_never_evaluates_active2() {
    let link = Link { active: cond("a", false), active2: cond("b", false), logic: 1, ..Default::default() };
    let (passes, seen) = evaluated(&link, true);
    assert!(passes);
    assert_eq!(seen, vec!["a"], "active2 is not evaluated when active decides the OR");
}

#[test]
fn and_with_a_true_active_evaluates_active2() {
    let link = Link { active: cond("a", false), active2: cond("b", false), logic: 0, ..Default::default() };
    let (passes, seen) = evaluated(&link, true);
    assert!(passes);
    assert_eq!(seen, vec!["a", "b"]);
}

#[test]
fn or_with_a_false_active_evaluates_active2() {
    let link = Link { active: cond("a", false), active2: cond("b", false), logic: 1, ..Default::default() };
    let (passes, seen) = evaluated(&link, false);
    assert!(!passes);
    assert_eq!(seen, vec!["a", "b"]);
}
