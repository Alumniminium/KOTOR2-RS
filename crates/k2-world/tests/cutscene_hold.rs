//! A cutscene holds the party: a control walk stops where it stands once one starts, and a
//! walking verb issued during one is refused. Skips (passes) when no install is found at
//! K2_INSTALL or ../.. .

use glam::{Vec2, Vec3};
use k2_world::control::{Command, Control, Env, MoveToArgs, ObjectArgs};
use k2_world::{ObjectKind, World};
mod support;

const DT: f32 = 1.0 / 30.0;

/// 101per with the intro's talks run to their end, as door_talk's settled world is.
fn settled() -> Option<World> {
    let mut w = World::new(support::install()?).expect("the install loads into a world");
    w.enter_module("101per", None).expect("101per enters");
    w.auto_dialog = true;
    for _ in 0..300 {
        w.tick(0.1);
    }
    w.auto_dialog = false;
    w.end_conversation(true);
    let _ = w.drain_events();
    Some(w)
}

/// Starts a conversation with a creature in 101per, so the party is held by it.
fn start_a_talk(w: &mut World) {
    let pc = w.party_leader();
    let talks: Vec<(u32, String)> = w.iter().filter(|o| o.kind == ObjectKind::Creature && o.id != pc).filter_map(|o| o.conversation.clone().map(|d| (o.id, d))).collect();
    for (npc, dlg) in talks {
        if w.start_conversation(npc, pc, &dlg) && w.conversation.is_some() {
            return;
        }
    }
    panic!("101per has a conversation to start");
}

/// A walkable point `radius` metres from `from` that a walk from `from` ends at (within 0.6 m).
fn walk_point(w: &World, from: Vec3, radius: f32) -> Option<Vec3> {
    (0..16).find_map(|k| {
        let a = k as f32 * std::f32::consts::TAU / 16.0;
        let q = Vec2::new(from.x, from.y) + Vec2::new(a.cos(), a.sin()) * radius;
        let to = Vec3::new(q.x, q.y, w.walk.ground(q.x, q.y, from.z + 1.0)?.z);
        let end = *w.walk.find_path(from, to, w.leader_clearance())?.last()?;
        (end.truncate().distance(to.truncate()) <= 0.6).then_some(to)
    })
}

fn leader_at(w: &World) -> Vec3 {
    w.get(w.party_leader()).expect("the leader is in the world").position
}

#[test]
fn a_control_walk_stops_where_it_stands_when_a_conversation_starts() {
    let Some(mut w) = settled() else { return };
    let start = leader_at(&w);
    let to = walk_point(&w, start, 6.0).expect("there is open floor 6 m from the leader");
    let root = std::env::temp_dir();
    let env = Env { save_root: &root, paused: false, movie: false };
    let mut control = Control::default();

    let started = control.submit(&mut w, &env, Command::MoveTo(MoveToArgs { point: Some(to.to_array()), ..Default::default() }));
    assert!(started.is_none(), "the walk starts");
    for _ in 0..30 {
        w.tick(DT);
        assert!(control.poll(&mut w, &env).is_none(), "the walk runs on open floor");
    }
    assert!(leader_at(&w).truncate().distance(start.truncate()) > 0.5, "the leader walked before the conversation");

    start_a_talk(&mut w);
    let stop = leader_at(&w);
    let mut answer = None;
    for _ in 0..60 {
        w.tick(DT);
        if let Some(reply) = control.poll(&mut w, &env) {
            answer = Some(reply);
            break;
        }
    }
    let reply = answer.expect("the walk answers once the conversation holds the party");
    assert!(!reply.ok && reply.message.starts_with("interrupted: a cutscene"), "the walk answers as interrupted: {}", reply.message);
    assert_eq!(leader_at(&w), stop, "the walk stopped where it stood");

    for _ in 0..60 {
        w.tick(DT);
        let _ = control.poll(&mut w, &env);
    }
    let after = leader_at(&w);
    assert_eq!(after, stop, "the leader moved during the conversation: from {stop:?} to {after:?}");
}

#[test]
fn a_walking_verb_during_a_cutscene_is_refused_and_nothing_moves() {
    let Some(mut w) = settled() else { return };
    let start = leader_at(&w);
    let to = walk_point(&w, start, 6.0).expect("there is open floor 6 m from the leader");
    let computer = w.iter().find(|o| o.kind == ObjectKind::Placeable && w.name_of(o.id) == "Medical Computer").map(|o| o.id).expect("101per has the Medical Computer");
    let root = std::env::temp_dir();
    let env = Env { save_root: &root, paused: false, movie: false };
    let mut control = Control::default();
    let move_to = || Command::MoveTo(MoveToArgs { point: Some(to.to_array()), ..Default::default() });

    // The conversation holds the party: a walk and a use are refused at once.
    start_a_talk(&mut w);
    for cmd in [move_to(), Command::Interact(ObjectArgs { object: computer })] {
        let reply = control.submit(&mut w, &env, cmd).expect("a verb during a cutscene answers at once");
        assert!(!reply.ok && reply.message == "refused: a cutscene is running", "refused during the conversation: {}", reply.message);
    }
    assert_eq!(leader_at(&w), start, "a refused verb moves nothing");

    // A script's input block holds the party after the conversation has ended.
    w.end_conversation(true);
    w.input_blocked_for = 5.0;
    let reply = control.submit(&mut w, &env, move_to()).expect("a verb during an input block answers at once");
    assert!(!reply.ok && reply.message == "refused: a cutscene is running", "refused during the input block: {}", reply.message);

    // Once the cutscene is over the same verb starts its walk.
    w.input_blocked_for = 0.0;
    assert!(control.submit(&mut w, &env, move_to()).is_none(), "the walk starts once the party has control");
}

/// A door's message box raised during a walk (here, by starting the closet door's own line, as a walk
/// into the locked door does) is feedback: the walk goes on and is not answered as interrupted.
#[test]
fn a_door_message_box_raised_mid_walk_does_not_stop_it() {
    let Some(mut w) = settled() else { return };
    let pc = w.party_leader();
    let start = leader_at(&w);
    let to = walk_point(&w, start, 6.0).expect("there is open floor 6 m from the leader");
    let root = std::env::temp_dir();
    let env = Env { save_root: &root, paused: false, movie: false };
    let mut control = Control::default();
    let started = control.submit(&mut w, &env, Command::MoveTo(MoveToArgs { point: Some(to.to_array()), ..Default::default() }));
    assert!(started.is_none(), "the walk starts");
    for _ in 0..30 {
        w.tick(DT);
        assert!(control.poll(&mut w, &env).is_none(), "the walk runs on open floor");
    }
    let before = leader_at(&w);

    let door = w.iter().find(|o| o.kind == ObjectKind::Door && o.tag == "MedClosetDoor").map(|o| o.id).expect("101per has MedClosetDoor");
    let line = w.get(door).and_then(|o| o.conversation.clone()).filter(|d| !d.is_empty()).expect("the closet door has a line");
    assert!(w.start_conversation(door, pc, &line), "the closet door's line starts");
    assert!(w.conversation_view().is_some_and(|v| v.boxed), "the closet door's line is a message box");
    for _ in 0..30 {
        w.tick(DT);
        let answer = control.poll(&mut w, &env);
        assert!(answer.is_none(), "a message box stopped the walk: {:?}", answer.map(|r| r.message));
    }
    let after = leader_at(&w);
    assert!(after.truncate().distance(before.truncate()) > 0.5, "the leader walks on under the box: from {before:?} to {after:?}");
}
