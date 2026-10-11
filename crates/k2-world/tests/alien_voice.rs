//! Which alien voice set a conversation line gets (`ConversationView.alien_race`):
//! the node's own AlienRaceNode, else the owner's AlienRaceOwner only when the
//! owner speaks. Runs on the real install; prints SKIP (and passes) when none
//! is found at K2_INSTALL or ../../..

use std::io::Write;
use k2_data::Dialog;
use k2_res::{Install, Resources};
use k2_world::World;

fn world(module: &str) -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP alien_voice: no install (set K2_INSTALL)");
        return None;
    };
    let mut w = World::new(install).expect("world");
    w.enter_module(module, None).expect(module);
    Some(w)
}

/// The dialog's entry node at `entry`, for the expected values.
fn node_of(install: &Install, module: &str, dlg: &str, entry: usize) -> (u32, u32, String) {
    let mut res = Resources::open(install.clone()).expect("resources");
    res.load_module(module).expect("module");
    let d = Dialog::load(&res, dlg).expect(dlg);
    let n = &d.entries[entry];
    (n.alien_race, d.alien_race_owner, n.speaker.clone())
}

fn install_dir() -> Install {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    Install::discover(dir).expect("install")
}

/// Margar (501ond, alienvo row 1) starts his conversation. His lines have an
/// empty speaker tag, so the owner speaks and the owner's row applies.
#[test]
fn empty_speaker_is_the_owner() {
    let Some(mut w) = world("501ond") else { return };
    let owner = w.by_tag("margar", 0).expect("margar in 501ond");
    let pc = w.party_leader();
    assert!(w.start_conversation(owner, pc, "margar"));
    let v = w.conversation_view().expect("a line");
    assert_eq!(v.speaker, owner);
    let (node_race, owner_race, speaker) = node_of(&install_dir(), "501ond", "margar", v.entry);
    assert!(speaker.is_empty());
    assert_eq!(node_race, 0);
    assert_eq!(owner_race, 1, "Margar is an Aqualish");
    assert_eq!(v.alien_race, 1);
}

/// 502han names its speaker Hanharr, who is not in 501ond. The unresolved
/// name is not the owner, so the owner's Hanharr row does not apply.
#[test]
fn unresolved_named_speaker_gets_no_owner_set() {
    let Some(mut w) = world("501ond") else { return };
    assert!(w.by_tag("Hanharr", 0).is_none(), "precondition: no Hanharr object in 501ond");
    let owner = w.by_tag("margar", 0).expect("margar in 501ond");
    let pc = w.party_leader();
    assert!(w.start_conversation(owner, pc, "502han"), "502han starts with margar as owner");
    let v = w.conversation_view().expect("a line");
    let (node_race, owner_race, speaker) = node_of(&install_dir(), "501ond", "502han", v.entry);
    assert_eq!(speaker, "Hanharr");
    assert_eq!(owner_race, 12, "the dialog's owner row is Hanharr's");
    assert_eq!(v.alien_race, node_race, "unresolved named speaker: the node's own row only");
}

/// vogga.dlg (303nar, Hutt row 13): the Vogga lines name the owner, the Hutt
/// tagged Vogga, so the owner's row applies to them.
#[test]
fn named_speaker_that_is_the_owner() {
    let Some(mut w) = world("303nar") else { return };
    let owner = w.by_tag("Vogga", 0).expect("the Hutt Vogga in 303nar");
    let pc = w.party_leader();
    assert!(w.start_conversation(owner, pc, "vogga"));
    let v = w.conversation_view().expect("a line");
    assert_eq!(v.speaker, owner, "the named speaker resolves to the owner");
    let (node_race, owner_race, speaker) = node_of(&install_dir(), "303nar", "vogga", v.entry);
    assert_eq!(speaker, "Vogga");
    assert_eq!(owner_race, 13, "Hutt row");
    let expected = if node_race != 0 { node_race } else { owner_race };
    assert_eq!(v.alien_race, expected);
}

/// thugboss.dlg (301nar, owner row 27) has one start, E0, with no conditions:
/// a Squik line. Squik is placed in the module and is not the owner. The line
/// has its own row (24), which applies; the owner's row (27) must not.
#[test]
fn named_speaker_that_is_not_the_owner() {
    let Some(mut w) = world("301nar") else { return };
    let owner = w.by_tag("ThugBoss", 0).expect("the owner ThugBoss in 301nar");
    let squik = w.by_tag("Squik", 0).expect("Squik in 301nar");
    assert_ne!(owner, squik);
    let pc = w.party_leader();
    assert!(w.start_conversation(owner, pc, "thugboss"));
    let v = w.conversation_view().expect("a line");
    let (node_race, owner_race, speaker) = node_of(&install_dir(), "301nar", "thugboss", v.entry);
    assert_eq!(speaker, "Squik", "the start line is Squik's");
    assert_eq!(owner_race, 27, "ThugBoss row");
    assert_eq!(node_race, 24, "Squik's own row");
    assert_eq!(v.speaker, squik);
    assert_eq!(v.alien_race, node_race, "a named non-owner gets its own row, not the owner's");
}

/// by_tag finds the first object with a tag. A second Hutt tagged Vogga is the
/// owner here, so the lines that name the tag are its own and get its row.
#[test]
fn owner_that_shares_its_tag_with_an_earlier_object() {
    let Some(mut w) = world("303nar") else { return };
    let first = w.by_tag("Vogga", 0).expect("the Hutt Vogga in 303nar");
    let owner = w.spawn_creature("c_hutt001", glam::Vec3::ZERO, 0.0).expect("a second Vogga");
    assert_ne!(owner, first);
    assert_eq!(w.by_tag("Vogga", 0), Some(first), "by_tag still returns the earlier object");
    let pc = w.party_leader();
    assert!(w.start_conversation(owner, pc, "vogga"));
    let v = w.conversation_view().expect("a line");
    assert_eq!(v.speaker, owner, "the owner's own tag names the owner");
    let (node_race, owner_race, speaker) = node_of(&install_dir(), "303nar", "vogga", v.entry);
    assert_eq!(speaker, "Vogga");
    assert_eq!(v.alien_race, if node_race != 0 { node_race } else { owner_race });
}
