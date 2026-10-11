//! The door message box: a door's own line with no real reply is shown as a box
//! (text and OK), not a conversation. Covers which lines are boxes, that the
//! scripts of a chain of nodes run in order as it advances, and how a box ends
//! (OK ends it as the conversation does; Escape aborts it). Runs against the real
//! install and skips (passes, printing why) when none is found at K2_INSTALL or
//! ../.. . With K2_INSTALL set, an install that does not load is a failure.

use k2_res::Install;
use k2_script::ObjectId;
use k2_world::{World, WorldEvent};

fn install() -> Option<Install> {
    let env = std::env::var("K2_INSTALL").ok();
    let dir = env.clone().unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    match Install::discover(&dir) {
        Ok(install) => Some(install),
        Err(e) if env.is_some() => panic!("K2_INSTALL={dir} is not an install: {e}"),
        Err(e) => {
            eprintln!("SKIP door_box: no install at {dir} ({e})");
            None
        }
    }
}

/// `module` entered with nothing run: the state at load.
fn fresh(module: &str) -> Option<World> {
    let mut w = World::new(install()?).expect("the install loads into a world");
    w.enter_module(module, None).unwrap();
    w.drain_events();
    Some(w)
}

fn door(w: &World, tag: &str) -> ObjectId {
    w.by_tag(tag, 0).unwrap_or_else(|| panic!("the module has {tag}"))
}

fn start(w: &mut World, owner: ObjectId, dialog: &str) {
    let pc = w.party_leader();
    assert!(w.start_conversation(owner, pc, dialog), "{dialog} starts");
}

/// The conversation's current line: its entry index and whether it is a box.
fn line(w: &World) -> (usize, bool) {
    let v = w.conversation_view().expect("a conversation is running");
    (v.entry, v.boxed)
}

#[test]
fn a_named_speakers_line_stays_a_conversation() {
    let Some(mut w) = fresh("101per") else { return };
    let owner = door(&w, "PrisonRoomDr");
    start(&mut w, owner, "prsndoor");
    let v = w.conversation_view().expect("a conversation is running");
    assert_eq!(v.entry, 1, "prsndoor's first line is Kreia's");
    assert!(!v.boxed, "a line by a named speaker is a conversation");
}

#[test]
fn a_door_line_with_real_replies_stays_a_conversation() {
    let Some(mut w) = fresh("101per") else { return };
    let owner = door(&w, "102PERDoor");
    start(&mut w, owner, "emrhatch");
    assert_eq!(line(&w), (1, false), "Kreia's line opens emrhatch");
    // Reply 0 is "Kreia?", which leads to the door's own line (entry 3), whose
    // replies are real choices ("Try to find some way to open the hatch.").
    w.choose_reply(0);
    let v = w.conversation_view().expect("a conversation is running");
    assert_eq!(v.entry, 3, "the reply leads to the door's line");
    assert!(v.replies.iter().any(|r| !r.text.trim().is_empty()), "the door's line has a real reply");
    assert!(!v.boxed, "a real choice needs the conversation");
}

#[test]
fn a_door_line_with_no_real_reply_is_a_box() {
    let Some(mut w) = fresh("232tel") else { return };
    let owner = door(&w, "TelosDoor3A2");
    start(&mut w, owner, "doorm");
    let v = w.conversation_view().expect("a conversation is running");
    assert!(v.boxed, "doorm is the door's own line with only [Continue]");
    assert!(v.text.contains("cannot be opened"), "the box shows the door's text: {}", v.text);

    let Some(mut w) = fresh("101per") else { return };
    let owner = door(&w, "MedClosetDoor");
    start(&mut w, owner, "meddoor");
    assert!(w.conversation_view().is_some_and(|v| v.boxed), "meddoor has no reply at all: a box");
}

#[test]
fn prsndoors_magnetic_seal_line_is_a_box() {
    let Some(mut w) = fresh("101per") else { return };
    let owner = door(&w, "PrisonRoomDr");
    // The first starting link needs the talked flag unset (c_talkedto); with it set,
    // prsndoor starts at its own line, entry 0.
    w.get_mut(owner).expect("the door is in the world").local_bools.insert(10, true);
    start(&mut w, owner, "prsndoor");
    assert_eq!(line(&w), (0, true), "the door's magnetic seal line is a box");
}

#[test]
fn a_silent_door_node_is_not_a_box() {
    let Some(mut w) = fresh("101per") else { return };
    let owner = door(&w, "PrisonRoomDr");
    start(&mut w, owner, "prsndoor");
    w.advance_conversation();
    // Entry 2 is the door's line with no text (its script only): nothing to show, so
    // it is not a box; the client moves on by itself.
    assert_eq!(line(&w), (2, false), "a door line with no text is not a box");
}

#[test]
fn the_scripts_of_a_chain_run_in_order_as_it_advances() {
    let Some(mut w) = fresh("101per") else { return };
    let owner = door(&w, "PrisonRoomDr");
    start(&mut w, owner, "prsndoor");
    // Entry 1 (Kreia) runs its script, a_setclaireffect; entry 2 (the door) runs
    // a_setkretel, which sets 101PER_Kreia_Telepath, before its line is shown.
    assert_eq!(w.globals.number("101PER_Kreia_Telepath"), 0, "entry 1 has not set it");
    w.advance_conversation();
    assert_eq!(line(&w).0, 2);
    assert_eq!(w.globals.number("101PER_Kreia_Telepath"), 1, "entry 2's script ran before its line");
    w.advance_conversation();
    assert_eq!(line(&w).0, 3, "the chain goes on to Kreia's last line");
    assert_eq!(w.globals.number("101PER_Kreia_Telepath"), 1);
}

#[test]
fn a_box_ends_on_ok_and_aborts_on_escape_as_a_conversation_does() {
    let Some(mut w) = fresh("232tel") else { return };
    let owner = door(&w, "TelosDoor3A2");

    // OK: the only line is done, so the conversation ends normally.
    start(&mut w, owner, "doorm");
    assert!(w.conversation_view().is_some_and(|v| v.boxed));
    w.advance_conversation();
    assert!(w.conversation.is_none(), "OK ends the box's conversation");
    let ended = w.drain_events().into_iter().find_map(|e| match e {
        WorldEvent::ConversationEnded { owner, aborted } => Some((owner, aborted)),
        _ => None,
    });
    assert_eq!(ended, Some((owner, false)), "OK is a normal end");

    // Escape: the client ends the conversation as aborted.
    start(&mut w, owner, "doorm");
    w.end_conversation(true);
    let ended = w.drain_events().into_iter().find_map(|e| match e {
        WorldEvent::ConversationEnded { owner, aborted } => Some((owner, aborted)),
        _ => None,
    });
    assert_eq!(ended, Some((owner, true)), "Escape is an abort");
}
