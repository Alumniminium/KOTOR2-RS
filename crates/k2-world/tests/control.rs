//! The remote control API (control.rs) against the real install: a new game
//! in the Ebon Hawk prologue, driven only by Command values. Saves go to
//! K2_SAVE_DIR (or a temp dir), never to the player's save folder. Skips
//! (passes) when no install is found at K2_INSTALL or ../.. .

use std::io::Write;
use std::path::PathBuf;

use glam::{Vec2, Vec3, Vec3Swizzles};
use k2_res::Install;
use k2_walk::STEP_DOWN;
mod support;

use k2_world::control::{
    ChooseReplyArgs, Command, Condition, EndDialogArgs, EquipArgs, LevelUpArgs, LoadArgs, MoveToArgs, ObjectArgs, Observation, ObserveArgs, SaveArgs,
    SkillAction, UseSkillArgs, WaitUntilArgs,
};
use k2_world::object::{Action, Target};
use k2_world::{ObjectKind, World};
use support::{beside_open_placeable, leader_beside, leader_radius, walkable_point_at, Session, BESIDE};

fn install() -> Option<Install> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    Install::discover(dir).ok()
}

fn save_root(tag: &str) -> PathBuf {
    let base = std::env::var_os("K2_SAVE_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(format!("k2-control-{tag}-{}", std::process::id()))
}

fn session(tag: &str) -> Option<Session> {
    let Some(install) = install() else {
        let _ = writeln!(std::io::stderr(), "SKIP {tag}: no install (set K2_INSTALL)");
        return None;
    };
    Some(Session::new(install, save_root(tag)).expect("a new world from the install"))
}

fn observe(s: &mut Session) -> Observation {
    observe_within(s, 40.0)
}

fn observe_within(s: &mut Session, radius: f32) -> Observation {
    let r = s.execute(Command::Observe(ObserveArgs { radius: Some(radius) }));
    assert!(r.ok, "{}", r.message);
    serde_json::from_value(r.data.expect("observe data")).expect("observation")
}

fn ok(r: &k2_world::control::Reply) {
    assert!(r.ok, "expected ok: {} (feedback {:?})", r.message, r.feedback);
}

fn id_of(o: &Observation, tag: &str) -> u32 {
    o.nearby.iter().find(|n| n.tag.eq_ignore_ascii_case(tag)).unwrap_or_else(|| panic!("no {tag} nearby")).id
}

/// Conversation lines played before the conversation is ended with end_dialog, as a
/// player who has had enough of it would end it.
const LINES_BEFORE_END: usize = 12;

/// Answers scripted dialogs (taking the reply starting with `prefer` when it
/// is offered, else Continue or the first reply) until the party is settled:
/// wait_until idle answers once the party has been in control, with nothing
/// scripted pending, for half a second of game time. The world runs between
/// lines (a paused conversation waits on time). Gives up after bounded steps,
/// saying what kept it busy.
fn play_until_idle(s: &mut Session, prefer: Option<&str>) {
    let mut lines = 0;
    for _ in 0..4000 {
        let o = observe(s);
        if let Some(c) = &o.conversation {
            lines += 1;
            if lines > LINES_BEFORE_END {
                ok(&s.execute(Command::EndDialog(EndDialogArgs::default())));
                lines = 0;
                continue;
            }
            let preferred = prefer.and_then(|p| c.replies.iter().find(|r| r.text.starts_with(p)));
            if let Some(r) = preferred {
                ok(&s.execute(Command::ChooseReply(ChooseReplyArgs { index: r.index })));
            } else if c.replies.iter().all(|r| r.text == "[Continue]") {
                ok(&s.execute(Command::ContinueDialog));
            } else {
                ok(&s.execute(Command::ChooseReply(ChooseReplyArgs { index: 0 })));
            }
            s.advance(0.5);
            continue;
        }
        lines = 0;
        // Idle does not look at conversations: one that began during the wait is played next.
        let idle = s.execute(Command::WaitUntil(WaitUntilArgs { condition: Condition::Idle, timeout: Some(2.0) }));
        if idle.ok && observe(s).conversation.is_none() {
            return;
        }
        if !idle.ok {
            s.advance(0.5);
        }
    }
    panic!("control did not come back: {}", idle_state(s));
}

/// What kept the world from going idle, for a test that gave up waiting.
fn idle_state(s: &mut Session) -> String {
    let o = observe(s);
    let w = &s.world;
    let conversation = match (&o.conversation, &w.conversation) {
        (Some(c), Some(live)) => format!(
            "conversation {} at entry {:?}, paused {}, replies {:?}",
            c.dialog,
            live.entry,
            live.paused,
            c.replies.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        ),
        _ => "no conversation".to_owned(),
    };
    let actions: Vec<String> = w
        .party
        .iter()
        .filter_map(|&id| w.get(id))
        .flat_map(|obj| obj.actions.iter().map(|a| format!("{a:?}").chars().take(80).collect::<String>()))
        .collect();
    format!(
        "world time {:.1} s; {conversation}; settled {}; pending transition {}; party actions {actions:?}",
        w.time,
        w.settled(),
        w.pending_transition.is_some()
    )
}

#[test]
fn prologue_observe_move_interact_talk_save_load() {
    let Some(mut s) = session("prologue") else { return };
    s.new_game("001ebo", "Test").unwrap();

    // Bad requests answer with errors and leave the game alone.
    assert!(!s.execute(Command::MoveTo(MoveToArgs::default())).ok);
    assert!(!s.execute(Command::ChooseReply(ChooseReplyArgs { index: 0 })).ok);
    assert!(!s.execute(Command::Interact(ObjectArgs { object: 0xFFFF_FF })).ok);
    assert!(!s.execute(Command::Save(SaveArgs { slot: "save-0001".into(), name: None })).ok, "only mcp- slots");

    let o = observe(&mut s);
    assert_eq!(o.module, "001ebo");
    assert_eq!(o.party.len(), 1);
    assert!(o.party[0].leader && o.party[0].hp > 0, "{:?}", o.party[0]);

    // The prologue starts its own dialog; wait for it, then play it through.
    s.advance(0.5);
    ok(&s.execute(Command::WaitUntil(WaitUntilArgs { condition: Condition::Dialog, timeout: Some(30.0) })));
    play_until_idle(&mut s, Some("[Continue the Prologue"));

    // Walk to the galaxy map, use it: a conversation starts.
    let o = observe(&mut s);
    let map = id_of(&o, "Galaxymap");
    let before = o.party[0].position;
    let walked = s.execute(Command::MoveTo(MoveToArgs { object: Some(map), ..Default::default() }));
    ok(&walked);
    let after = observe(&mut s).party[0].position;
    assert!((after[0] - before[0]).abs() + (after[1] - before[1]).abs() > 1.0, "the leader moved");
    let used = s.execute(Command::Interact(ObjectArgs { object: map }));
    ok(&used);
    let o = observe(&mut s);
    let talk = o.conversation.clone().expect("galaxy map conversation");
    assert_eq!(talk.dialog, "glxy_map");
    ok(&s.execute(Command::ContinueDialog));
    assert!(observe(&mut s).conversation.is_none(), "conversation ended");

    // Save, change the game, load: the state digest is the saved one.
    let saved = s.execute(Command::Save(SaveArgs { slot: "mcp-prologue".into(), name: Some("Prologue test".into()) }));
    ok(&saved);
    let digest = s.world.state_digest();
    s.advance(5.0);
    assert_ne!(s.world.state_digest(), digest, "time passed");
    let listed = s.execute(Command::ListSaves);
    ok(&listed);
    let slots = listed.data.unwrap()["slots"].as_array().unwrap().clone();
    assert!(slots.iter().any(|v| v["slot"] == "mcp-prologue"), "{slots:?}");
    ok(&s.execute(Command::Load(LoadArgs { slot: "mcp-prologue".into() })));
    assert_eq!(s.world.state_digest(), digest, "loaded state matches the saved one");

    // Leveling needs experience: the explicit refusal path.
    let leader = observe(&mut s).party[0].id;
    assert!(!s.execute(Command::LevelUp(LevelUpArgs { creature: leader, auto: Some(true), ..Default::default() })).ok);
    assert!(!s.execute(Command::Equip(EquipArgs { item: 0xFFFF, slot: None })).ok);
    let _ = std::fs::remove_dir_all(save_root("prologue"));
}

#[test]
fn talk_and_exit_from_peragus() {
    let Some(mut s) = session("peragus") else { return };
    s.new_game("101per", "Exile").unwrap();
    // The party starts in the medbay tank; the awakening sequence (101awake)
    // gets them out. Wait for it to start, then play it through.
    s.advance(0.5);
    ok(&s.execute(Command::WaitUntil(WaitUntilArgs { condition: Condition::Dialog, timeout: Some(60.0) })));
    play_until_idle(&mut s, None);
    let o = observe_within(&mut s, 100.0);
    assert_eq!(o.module, "101per");
    let atton = id_of(&o, "Atton");
    // The walk from the wake-up spot does not reach Atton: the leader is put on
    // walk ground beside him first. Then it walks the last metres and talks.
    leader_beside(&mut s, atton, &BESIDE);
    ok(&s.execute(Command::TalkTo(ObjectArgs { object: atton })));
    assert_eq!(observe(&mut s).conversation.map(|c| c.speaker.name), Some("Atton".to_owned()));
    play_until_idle(&mut s, None);
    assert!(observe(&mut s).conversation.is_none());

    // The turbolift door leads to 105per; the party walks there and goes through.
    // (The door is looked up in the world: it is too far for observe's nearby list.)
    let lift = s.world.by_tag("PERDoor105", 0).expect("turbolift door");
    leader_beside(&mut s, lift, &BESIDE);
    let exit = s.execute(Command::Transition(ObjectArgs { object: lift }));
    ok(&exit);
    assert_eq!(observe(&mut s).module, "105per");
    let _ = std::fs::remove_dir_all(save_root("peragus"));
}

#[test]
fn host_verbs_are_refused_by_the_world_layer() {
    let Some(mut s) = session("host") else { return };
    s.new_game("101per", "Exile").unwrap();
    for cmd in [
        Command::Info,
        Command::Quit,
        Command::TraceStop,
        Command::KeyPress(k2_world::control::KeyPressArgs { key: "KeyW".into(), modifiers: None, hold_ms: None }),
    ] {
        let r = s.execute(cmd);
        assert!(!r.ok && r.message.contains("host verb"), "{}", r.message);
    }
    let _ = std::fs::remove_dir_all(save_root("host"));
}

/// The prologue has a gap between its intro and its "controls" talk: the party
/// is in control (the old predicate held) while a scripted resume is still
/// pending. wait_until idle must not answer there: the party is not settled
/// before the controls talk has begun.
#[test]
fn idle_waits_out_the_prologue_gap_before_the_controls_talk() {
    let Some(mut s) = session("gap") else { return };
    s.new_game("001ebo", "Test").unwrap();
    let mut controls_at = None;
    let mut settled_at = None;
    // Frame by frame, answering each talk the frame it comes up.
    for _ in 0..(120 * 30) {
        if let Some(c) = observe(&mut s).conversation {
            if c.dialog == "controls" {
                controls_at.get_or_insert(s.world.time);
            }
            let r = if c.replies.iter().all(|r| r.text == "[Continue]") { Command::ContinueDialog } else { Command::ChooseReply(ChooseReplyArgs { index: 0 }) };
            ok(&s.execute(r));
            continue;
        }
        s.world.tick(support::DT);
        let _ = s.world.drain_events();
        if settled_at.is_none() && s.world.settled() {
            settled_at = Some(s.world.time);
        }
        if controls_at.is_some() && settled_at.is_some() {
            break;
        }
    }
    let controls_at = controls_at.expect("the controls talk comes up");
    let settled_at = settled_at.expect("the party settles after the talk");
    assert!(settled_at >= controls_at, "settled at {settled_at:.2} s, before the controls talk at {controls_at:.2} s");
    // The verb agrees once the talk is over and the party settles.
    while let Some(c) = observe(&mut s).conversation {
        let r = if c.replies.iter().all(|r| r.text == "[Continue]") { Command::ContinueDialog } else { Command::ChooseReply(ChooseReplyArgs { index: 0 }) };
        ok(&s.execute(r));
    }
    ok(&s.execute(Command::WaitUntil(WaitUntilArgs { condition: Condition::Idle, timeout: Some(30.0) })));
    let _ = std::fs::remove_dir_all(save_root("gap"));
}

/// A wait that names no timeout gives up after one game second. The leader is
/// walking a few metres, which takes longer than that, so the wait ends on its limit.
#[test]
fn a_wait_without_a_timeout_gives_up_after_one_game_second() {
    let Some(mut s) = peragus_in_control("wait-default") else { return };
    beside_open_placeable(&mut s);
    let from = s.leader_position();
    let goal = [3.0, 4.0, 5.0, 6.0].into_iter().find_map(|r| walkable_point_at(&s.world, from, r)).expect("a walkable point 3 to 6 m from the leader");
    let leader = s.world.party_leader();
    s.world.get_mut(leader).expect("the leader is in the world").actions.clear();
    s.world.push_action(leader, Action::MoveTo { target: Target::Point(goal), run: false, range: 0.5, timeout: None, elapsed: 0.0, stuck: 0.0 });
    let started = s.world.time;
    let r = s.execute(Command::WaitUntil(WaitUntilArgs { condition: Condition::Idle, timeout: None }));
    let waited = s.world.time - started;
    assert!(!r.ok && r.message.contains("timed out after 1 s"), "{}", r.message);
    assert!((0.9..=1.2).contains(&waited), "waited {waited:.2} game s");
}

#[test]
fn stop_cancels_a_module_change_that_is_queued() {
    let Some(mut s) = session("queued") else { return };
    s.new_game("101per", "Exile").unwrap();
    s.world.pending_transition = Some(k2_world::PendingTransition { module: "105per".into(), waypoint_tag: None, movies: Vec::new() });
    let r = s.execute(Command::Stop);
    ok(&r);
    assert!(r.message.contains("105per"), "{}", r.message);
    assert!(s.world.pending_transition.is_none());
    let _ = std::fs::remove_dir_all(save_root("queued"));
}

/// Interact follows the pointer's rules: a placeable that is not useable is refused,
/// and observe says which objects run a script when used.
#[test]
fn interact_refuses_what_the_pointer_cannot_use() {
    let Some(mut s) = session("useable") else { return };
    s.new_game("101per", "Exile").unwrap();
    let o = observation_within(&mut s, 100.0);
    let inert = o.nearby.iter().find(|n| n.kind == "placeable" && !n.useable);
    if let Some(n) = inert {
        let r = s.execute(Command::Interact(ObjectArgs { object: n.id }));
        assert!(!r.ok && r.message.contains("cannot be used"), "{}", r.message);
    }
    let scripted = o.nearby.iter().filter(|n| n.scripted_use).count();
    assert!(scripted <= o.nearby.len(), "every nearby object reports scripted_use");
    let _ = std::fs::remove_dir_all(save_root("useable"));
}

fn observation_within(s: &mut Session, radius: f32) -> Observation {
    let r = s.execute(Command::Observe(ObserveArgs { radius: Some(radius) }));
    ok(&r);
    serde_json::from_value(r.data.expect("observe data")).expect("observation")
}

/// The engine's message for an object the leader cannot get within use reach of.
const UNREACHABLE: &str = "Unable to reach target!";

/// 101per with the wake-up talks played through: the leader is in control.
fn peragus_in_control(tag: &str) -> Option<Session> {
    let mut s = session(tag)?;
    s.new_game("101per", "Exile").unwrap();
    s.advance(0.5);
    ok(&s.execute(Command::WaitUntil(WaitUntilArgs { condition: Condition::Dialog, timeout: Some(60.0) })));
    play_until_idle(&mut s, None);
    Some(s)
}

/// Reach of a usable placeable: REACH (1.8) plus its radius (0.8).
const PLACEABLE_REACH: f32 = 2.6;

/// What is around the leader, for a walk that stopped: the walk-mesh edges
/// within a metre (room or object, the mesh) and the creatures within two.
fn surroundings(s: &Session) -> String {
    let w = &s.world;
    let at = s.leader_position();
    let leader = w.party_leader();
    let edges: Vec<String> = w.walk.blockers_near(at, 1.0).iter().map(|(a, b, room, mesh)| format!("{a:?}-{b:?} room {room} mesh {mesh}")).collect();
    let creatures: Vec<String> = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && o.id != leader && o.position.truncate().distance(at.truncate()) < 2.0)
        .map(|o| format!("{} at {:?} dead {}", w.name_of(o.id), o.position, o.creature().is_some_and(|c| c.dead)))
        .collect();
    let talk = w.conversation.as_ref().map_or("no conversation".to_string(), |c| format!("conversation with {} at entry {:?}", w.name_of(c.owner), c.entry));
    format!("leader at {at:?}; walk edges within 1 m [{}]; creatures within 2 m [{}]; {talk}", edges.join(", "), creatures.join(", "))
}

/// A walk that must have arrived: on failure the reply and what is around the leader.
fn ok_walk(s: &Session, r: &k2_world::control::Reply) {
    assert!(r.ok, "expected ok: {} (feedback {:?}); {}", r.message, r.feedback, surroundings(s));
}

/// A walk command as the player gives it: a conversation that is running, or that starts on
/// the way (a walk is interrupted by one), is played out through the control API, then the
/// command runs again.
fn through_talk(s: &mut Session, cmd: Command) -> k2_world::control::Reply {
    let r = s.execute(cmd.clone());
    if r.message.starts_with("in conversation") || r.message.starts_with("refused: a cutscene") || r.message.starts_with("interrupted: a cutscene") {
        play_until_idle(s, None);
        return s.execute(cmd);
    }
    r
}

/// interact on an object out of reach walks the leader into reach before it uses
/// the object: the leader moves step by step, and the use comes at the first step
/// it stands within reach.
#[test]
fn interact_walks_into_reach_before_it_uses_the_object() {
    let Some(mut s) = peragus_in_control("walk-use") else { return };
    let anchor = beside_open_placeable(&mut s);
    let at = s.leader_position();
    let target = s.world.get(anchor).expect("the placeable is in the world").position;
    let obj = Vec2::new(target.x, target.y);
    let start = Vec2::new(at.x, at.y).distance(obj);
    assert!(start > PLACEABLE_REACH, "the leader starts {start:.2} m from {target:?}, in reach already");
    let (r, trace) = s.execute_traced(Command::Interact(ObjectArgs { object: anchor }));
    ok_walk(&s, &r);
    assert!(trace.len() > 3, "the leader walked for {} steps", trace.len());
    // The use comes on the first step the leader stands in reach: it walked before it used.
    let first_in = trace.iter().position(|p| Vec2::new(p.x, p.y).distance(obj) <= PLACEABLE_REACH);
    assert_eq!(first_in, Some(trace.len() - 1), "the use came before the leader stood in reach ({} steps, last at {:?})", trace.len(), trace.last());
}

/// An object with no walkable spot in reach is refused before anything happens:
/// the leader does not move, and the object is not used.
#[test]
fn interact_with_no_spot_in_reach_is_refused_and_uses_nothing() {
    let Some(mut s) = peragus_in_control("no-spot") else { return };
    let target = beside_open_placeable(&mut s);
    let from = s.leader_position();
    // Off the walkmesh, 200 m out: no spot is within reach of it.
    s.world.place(target, from + Vec3::new(200.0, 0.0, 0.0));
    let open_before = s.world.get(target).and_then(|x| x.placeable().map(|p| p.open));
    let r = s.execute(Command::Interact(ObjectArgs { object: target }));
    assert!(!r.ok && r.message == UNREACHABLE, "{}", r.message);
    let open_after = s.world.get(target).and_then(|x| x.placeable().map(|p| p.open));
    assert_eq!(open_after, open_before, "the object was used");
    let moved = s.leader_position().truncate().distance(from.truncate());
    assert!(moved < 0.01, "the leader moved {moved:.2} m toward an unreachable object");
}

/// move_to a point the walkmesh does not reach says so, and where the leader stopped.
#[test]
fn move_to_a_point_it_cannot_reach_says_so() {
    let Some(mut s) = peragus_in_control("move-far") else { return };
    let from = s.leader_position();
    let far = [from.x + 200.0, from.y, from.z];
    let r = s.execute(Command::MoveTo(MoveToArgs { point: Some(far), ..Default::default() }));
    assert!(!r.ok, "{}", r.message);
    assert!(r.message.starts_with("could not reach point"), "{}", r.message);
    assert!(r.message.contains("stopped at"), "{}", r.message);
}

/// move_to a walkable point the route reaches: the leader arrives within the
/// verdict's tolerance, and the reply says it arrived.
#[test]
fn move_to_a_reachable_point_arrives() {
    let Some(mut s) = peragus_in_control("move-near") else { return };
    beside_open_placeable(&mut s);
    let from = s.leader_position();
    let goal = [6.0, 4.0, 3.0, 5.0, 8.0].into_iter().find_map(|r| walkable_point_at(&s.world, from, r)).expect("a walkable point 3 to 8 m from the leader");
    let r = s.execute(Command::MoveTo(MoveToArgs { point: Some(goal.to_array()), ..Default::default() }));
    ok_walk(&s, &r);
    assert!(r.message.starts_with("arrived near point"), "{}", r.message);
    let end = s.leader_position();
    let short = end.truncate().distance(goal.truncate());
    assert!(short <= 1.0, "arrived {short:.2} m from {goal:?}, at {end:?}");
}

/// use_skill walks the leader into reach of its target before it tries the skill,
/// as interact does. The placeable is not locked, so the try itself may answer no.
#[test]
fn use_skill_walks_into_reach_before_it_tries() {
    let Some(mut s) = peragus_in_control("walk-skill") else { return };
    let anchor = beside_open_placeable(&mut s);
    let (r, trace) = s.execute_traced(Command::UseSkill(UseSkillArgs { skill: SkillAction::Unlock, target: anchor }));
    let target = s.world.get(anchor).expect("the placeable is in the world").position;
    let obj = Vec2::new(target.x, target.y);
    assert!(trace.len() > 3, "the leader walked for {} steps: {}", trace.len(), r.message);
    let first_in = trace.iter().position(|p| Vec2::new(p.x, p.y).distance(obj) <= PLACEABLE_REACH);
    assert!(first_in.is_some(), "the leader never stood in reach of {target:?} ({} steps): {}; {}", trace.len(), r.message, surroundings(&s));
    assert_eq!(first_in, Some(trace.len() - 1), "the skill was tried after the leader stood in reach ({} steps): {}", trace.len(), r.message);
}

/// Diagnostic, not a check (run with --ignored --nocapture): the leader put where
/// the round-3 walks stopped, (-3.0, -3.1), by MedClosetDoor. Prints the walk edges
/// and doors round the spot, the stand and route to the placeable at (0.85, -3.11),
/// the route replayed a step at a time with the slide the walk uses, and the
/// reply of a walk to the stand.
#[test]
#[ignore]
fn diagnose_the_walk_stop_by_the_closet_door() {
    let Some(mut s) = peragus_in_control("diag-closet") else { return };
    let say = |line: String| {
        let _ = writeln!(std::io::stderr(), "{line}");
    };
    let spot = Vec3::new(-3.0, -3.1, 9.07);
    let leader = s.world.party_leader();
    s.world.place(leader, spot);
    let at = s.leader_position();
    say(format!("placed: {}", surroundings(&s)));
    for (a, b, room, mesh) in s.world.walk.blockers_near(at, 3.0) {
        say(format!("edge {a:?}-{b:?} room {room} mesh {mesh}"));
    }
    for o in s.world.iter().filter(|o| o.kind == ObjectKind::Door && o.position.truncate().distance(at.truncate()) < 6.0) {
        let d = o.door();
        say(format!("door {} at {:?} open {:?} locked {:?}", s.world.name_of(o.id), o.position, d.map(|d| d.open), d.map(|d| d.locked)));
    }
    let target = s.world.iter().find(|o| o.kind == ObjectKind::Placeable && o.position.truncate().distance(Vec2::new(0.85, -3.11)) < 1.5).map(|o| o.id);
    say(format!("target {:?}", target.map(|t| s.world.name_of(t))));
    let stand = target.and_then(|t| s.world.use_point(at, t));
    say(format!("stand {stand:?}"));
    if let Some(stand) = stand {
        let route = s.world.walk.find_path(at, stand, leader_radius(&s.world)).unwrap_or_default();
        say(format!("route to stand {route:?}"));
        // The route replayed with the slide the walk uses, a step at a time.
        let mut pos = at;
        'legs: for aim in &route {
            for _ in 0..400 {
                let to = (*aim - pos).truncate();
                if to.length() < 0.1 {
                    continue 'legs;
                }
                let next = s.world.walk.move_slide(pos, to.normalize() * 0.08, leader_radius(&s.world).min(0.5));
                if (next - pos).truncate().length() < 1e-3 {
                    say(format!("slide stalls at {pos:?} toward {aim:?}; edges within 0.8 m {:?}", s.world.walk.blockers_near(pos, 0.8)));
                    break 'legs;
                }
                pos = next;
            }
        }
        say(format!("replayed route ends at {pos:?}"));
        let r = s.execute(Command::MoveTo(MoveToArgs { point: Some(stand.to_array()), ..Default::default() }));
        say(format!("walk to stand: {} ({})", r.message, surroundings(&s)));
    }
    say(format!("ground at (-1.0, -3.1): {:?}", s.world.walk.ground(-1.0, -3.1, 9.07).map(|h| h.z)));
}

/// The object blocker nearest `at` within a metre, as its segment's ends. Walls
/// are left out: a closed door shows as an object blocker.
fn nearest_object_blocker(w: &World, at: Vec3) -> Option<(Vec2, Vec2)> {
    let gap = |a: Vec3, b: Vec3| (at.xy() - closest_on_seg(at.xy(), a.xy(), b.xy())).length();
    w.walk
        .blockers_near(at, 1.0)
        .into_iter()
        .filter(|&(_, _, room, _)| !room)
        .min_by(|x, y| gap(x.0, x.1).total_cmp(&gap(y.0, y.1)))
        .map(|(a, b, _, _)| (a.xy(), b.xy()))
}

/// The signed distance of `p` from the line a-b, positive on the left of a to b.
fn side_of(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    (b - a).perp_dot(p - a) / (b - a).length().max(1e-6)
}

/// Opens the PeragusDoor1 doors the leader can reach, through the control API, until
/// none is reachable from where the leader stands. Each round takes the nearest closed
/// door whose use stand is reachable now (a door behind a shut one waits its turn); the
/// interact walks the leader there and uses it, and the door must then be open.
/// Returns the doors opened, in order.
/// The use stand for `target` from `from`, when the route there runs through no locked
/// door: a walk that stops at a locked door does not reach the stand.
fn use_point_unlocked(w: &World, from: Vec3, target: u32) -> Option<Vec3> {
    let stand = w.use_point(from, target)?;
    let mut legs = vec![from];
    legs.extend(w.walk.find_path(from, stand, w.leader_clearance()).unwrap_or_default());
    let locked = legs.windows(2).filter_map(|leg| w.door_across(leg[0], leg[1])).any(|d| w.get(d).and_then(|o| o.door()).is_some_and(|d| d.locked));
    (!locked).then_some(stand)
}

fn open_peragus_doors(s: &mut Session) -> Vec<u32> {
    let mut opened = Vec::new();
    // Each pass opens one door, so the passes stop at the doors there are.
    for _ in 0..16 {
        let here = s.leader_position();
        let mut closed: Vec<(u32, f32)> = s
            .world
            .iter()
            .filter(|o| o.kind == ObjectKind::Door && o.tag == "PeragusDoor1" && !o.door().is_some_and(|d| d.open))
            .map(|o| (o.id, o.position.truncate().distance(here.truncate())))
            .collect();
        closed.sort_by(|a, b| a.1.total_cmp(&b.1));
        let Some(door) = closed.iter().map(|c| c.0).find(|&d| use_point_unlocked(&s.world, here, d).is_some()) else {
            break;
        };
        let (r, _) = s.execute_traced(Command::Interact(ObjectArgs { object: door }));
        s.advance(4.0);
        let open = s.world.get(door).and_then(|o| o.door()).is_some_and(|d| d.open);
        assert!(open, "PeragusDoor1 {door} did not open: {}; {}", r.message, surroundings(s));
        opened.push(door);
    }
    opened
}

/// Regression: the leader beside the Kreia bed (about 0.66 m from it, on walkable
/// ground, the save position after the intro) walks out. Every walk from that spot
/// answered at once before. Points a few metres off are reached, the Medical Computer
/// is used from in reach, and an unlocked door is walked to.
#[test]
fn the_leader_beside_the_kreia_bed_walks_out() {
    let Some(mut s) = peragus_in_control("bed-out") else { return };
    let spot = Vec3::new(-1.29, 19.93, 9.02);
    let pc = s.world.party_leader();
    for p in [[-4.0_f32, 16.0, 9.0], [-1.3, 17.0, 9.0], [1.5, 19.9, 9.0]] {
        s.world.place(pc, spot);
        let r = through_talk(&mut s, Command::MoveTo(MoveToArgs { point: Some(p), ..Default::default() }));
        ok_walk(&s, &r);
    }

    let computer = s.world.iter().find(|o| o.kind == ObjectKind::Placeable && s.world.name_of(o.id) == "Medical Computer").map(|o| o.id);
    let computer = computer.expect("101per has the Medical Computer");
    // The closed doors the route to the computer runs through: each must be open after the walk.
    let doors_on_route: Vec<u32> = match s.world.use_point(spot, computer) {
        Some(stand) => match s.world.walk.find_path(spot, stand, s.world.leader_clearance()) {
            Some(route) => {
                let mut legs = vec![spot];
                legs.extend(route);
                legs.windows(2).filter_map(|leg| s.world.door_across(leg[0], leg[1])).collect()
            }
            None => Vec::new(),
        },
        None => Vec::new(),
    };
    s.world.place(pc, spot);
    let r = through_talk(&mut s, Command::Interact(ObjectArgs { object: computer }));
    let at = s.leader_position();
    assert_ne!(r.message, UNREACHABLE, "the Medical Computer from beside the bed: {}; {}", r.message, surroundings(&s));
    // The use may start a conversation: the player plays it out before the next command.
    play_until_idle(&mut s, None);
    assert!(s.world.in_use_reach(at, computer), "the leader at {at:?} is out of reach of the Medical Computer after the walk");
    for &door in &doors_on_route {
        assert!(s.world.get(door).and_then(|o| o.door()).is_some_and(|d| d.open), "the walk to the Medical Computer did not open door {door} on its way");
    }

    let door = s
        .world
        .iter()
        .filter(|o| o.kind == ObjectKind::Door && o.door().is_some_and(|d| !d.locked && !d.is_static))
        .min_by(|a, b| a.position.truncate().distance(spot.xy()).total_cmp(&b.position.truncate().distance(spot.xy())))
        .map(|o| o.id)
        .expect("101per has an unlocked door");
    s.world.place(pc, spot);
    let r = through_talk(&mut s, Command::MoveTo(MoveToArgs { object: Some(door), ..Default::default() }));
    ok_walk(&s, &r);
    let at = s.leader_position();
    assert!(s.world.in_use_reach(at, door), "the leader at {at:?} is out of reach of door {door} after the walk");
}

/// Regression (live): the Medical Computer is used from four metres away in its room. From
/// a floor spot 4 m from it that has a use stand, the interact walks to the stand and uses it.
#[test]
fn the_medical_computer_is_used_from_four_metres_in_its_room() {
    let Some(mut s) = peragus_in_control("computer-4m") else { return };
    let computer = medical_computer(&s);
    let c = s.world.get(computer).expect("the computer is in the world").position;
    let pc = s.world.party_leader();
    let spot = (0..16)
        .map(|k| {
            let t = k as f32 * std::f32::consts::TAU / 16.0;
            Vec3::new(c.x + 4.0 * t.cos(), c.y + 4.0 * t.sin(), c.z)
        })
        .filter_map(|q| s.world.walk.ground_nearest(q.x, q.y, c.z).map(|h| Vec3::new(q.x, q.y, h.z)))
        .find(|&q| s.world.use_point(q, computer).is_some());
    let spot = spot.expect("a floor spot 4 m from the Medical Computer has a use stand");
    s.world.place(pc, spot);
    let r = s.execute(Command::Interact(ObjectArgs { object: computer }));
    assert_ne!(r.message, UNREACHABLE, "the Medical Computer from 4 m away: {}; {}", r.message, surroundings(&s));
    let at = s.leader_position();
    assert!(s.world.in_use_reach(at, computer), "the leader at {at:?} is out of reach of the Medical Computer after the walk");
}

/// Regression (playtest): from beside the Kolto Tank (about 0.31 m from it, on walkable
/// floor, reached by keyboard) a walk to (-1.3, 19.9, 9) routes out.
#[test]
fn the_leader_beside_the_kolto_tank_walks_out() {
    let Some(mut s) = peragus_in_control("tank-out") else { return };
    let pc = s.world.party_leader();
    s.world.place(pc, Vec3::new(0.98, 22.98, 9.02));
    let r = s.execute(Command::MoveTo(MoveToArgs { point: Some([-1.3, 19.9, 9.0]), ..Default::default() }));
    ok_walk(&s, &r);
}

/// The Medical Computer of 101per, by name.
fn medical_computer(s: &Session) -> u32 {
    s.world.iter().find(|o| o.kind == ObjectKind::Placeable && s.world.name_of(o.id) == "Medical Computer").map(|o| o.id).expect("101per has the Medical Computer")
}

/// A walk to `point`, then an interact with the Medical Computer, from where the leader stands.
fn walk_to_point_then_computer(s: &mut Session, point: [f32; 3], computer: u32, when: &str) {
    let from = s.leader_position();
    let to = Vec3::from(point);
    let region = s.world.walk.same_region(from, to);
    let parts = (s.world.walk.component_at(from), s.world.walk.component_at(to));
    let route = s.world.walk.find_path(from, to, s.world.leader_clearance()).map(|r| r.len());
    let r = through_talk(s, Command::MoveTo(MoveToArgs { point: Some(point), ..Default::default() }));
    assert!(r.ok, "{when}: the walk to {point:?} from {from:?} (same region {region}, components {parts:?}, route {route:?}): {}; {}", r.message, surroundings(s));
    let r = through_talk(s, Command::Interact(ObjectArgs { object: computer }));
    assert_ne!(r.message, UNREACHABLE, "{when}: the Medical Computer: {}; {}", r.message, surroundings(s));
    // The use may start a conversation: the player plays it out, so no save is made in one.
    play_until_idle(s, None);
}

/// Regression (playtest): a loaded save walks the leader as a new game does. Walks from
/// open floor failed at once after a load. The walks are made before the save, then
/// after the save is loaded through the real save path (a temp directory, K2_SAVE_DIR).
#[test]
fn a_loaded_save_walks_the_leader_as_a_new_game_does() {
    let Some(mut s) = peragus_in_control("save-walk") else { return };
    let point = [-0.5_f32, 14.4, 9.0];
    let computer = medical_computer(&s);
    // The open-floor spot the walks start from, before the save and after the load.
    let pc = s.world.party_leader();
    s.world.place(pc, Vec3::new(2.47, 15.74, 9.02));
    walk_to_point_then_computer(&mut s, point, computer, "before the save");

    let saved = s.execute(Command::Save(SaveArgs { slot: "mcp-walk-test".into(), name: Some("walk test".into()) }));
    ok(&saved);
    let loaded = s.execute(Command::Load(LoadArgs { slot: "mcp-walk-test".into() }));
    ok(&loaded);
    // The playtest spot: open floor, away from placeables.
    let pc = s.world.party_leader();
    s.world.place(pc, Vec3::new(2.47, 15.74, 9.02));
    let computer = medical_computer(&s);
    walk_to_point_then_computer(&mut s, point, computer, "after the load");
    let _ = std::fs::remove_dir_all(save_root("save-walk"));
}

/// Regression: a walk never crosses a closed door's blocker. The leader starts on the
/// medbay floor, at the intro's wake-up waypoint (the script moves it there; the test
/// places it). A walk to the placeable across the closed Medical Storage door answers
/// "Unable to reach target!" without crossing. A walk to the door itself stands the
/// leader in reach of it on the medbay side, without crossing. The walk back to the
/// start arrives.
#[test]
fn walk_past_the_closed_medical_storage_door_stays_on_its_side() {
    let Some(mut s) = peragus_in_control("closet-walk") else { return };
    let wake = s.world.by_tag("WP_wakeup_donnie", 0).expect("101per has WP_wakeup_donnie");
    let wake_at = s.world.get(wake).expect("the waypoint is in the world").position;
    let leader = s.world.party_leader();
    s.world.place(leader, wake_at);
    let start = s.leader_position();
    // The Peragus doors stand between the wake-up spot and the closet: they are opened
    // in order, nearest first, as the player would go through them.
    let opened = open_peragus_doors(&mut s);
    let door = s.world.by_tag("MedClosetDoor", 0).expect("101per has MedClosetDoor");
    // The doors on the way to the closet are the ones the leader could reach: the closet's stand must be reachable now.
    assert!(!opened.is_empty(), "no PeragusDoor1 is reachable from the wake-up spot");
    let here = s.leader_position();
    assert!(s.world.use_point(here, door).is_some(), "no stand for MedClosetDoor is reachable from {here:?} after opening {opened:?}");
    let door_at = s.world.get(door).expect("the door is in the world").position;
    let (a, b) = nearest_object_blocker(&s.world, door_at).expect("an object blocker at the closed door");
    let side = side_of(a, b, start.xy()).signum();
    let stays = |trace: &Vec<Vec3>| trace.iter().all(|p| side_of(a, b, p.xy()) * side > -1e-3);

    // Across the locked door the route runs into it: the walk stops at the door (the locked-door
    // feedback) and does not cross.
    let target = s.world.iter().find(|o| o.kind == ObjectKind::Placeable && o.position.truncate().distance(Vec2::new(0.85, -3.11)) < 1.5).map(|o| o.id);
    let target = target.expect("101per has a placeable at (0.85, -3.11)");
    let (r, trace) = s.execute_traced(Command::Interact(ObjectArgs { object: target }));
    assert!(r.message.contains("locked"), "the walk across the locked closet door should stop at it: {}", r.message);
    assert!(stays(&trace), "the walk to the placeable crossed the closed door's blocker {a:?}-{b:?}");

    // The door itself: the leader walks to a stand in reach of it, on the medbay side.
    let (r, trace) = s.execute_traced(Command::Interact(ObjectArgs { object: door }));
    let moved = (s.leader_position() - start).truncate().length();
    assert!(moved > 1.0, "the walk to the door moved the leader {moved:.2} m from {start:?}: {}; {}", r.message, surroundings(&s));
    let at_door = s.leader_position().truncate().distance(door_at.truncate());
    assert!(at_door <= 3.0, "the leader stopped {at_door:.2} m from the door, out of its reach: {}", r.message);
    assert_ne!(r.message, UNREACHABLE, "the walk to the door found no stand in reach");
    assert!(stays(&trace), "the walk to the door crossed the closed door's blocker {a:?}-{b:?}: {}", r.message);

    // Back to the start on the medbay floor.
    let (r, trace) = s.execute_traced(Command::MoveTo(MoveToArgs { point: Some(start.to_array()), ..Default::default() }));
    ok_walk(&s, &r);
    assert!(stays(&trace), "the walk back to the start {start:?} crossed the closed door's blocker {a:?}-{b:?}");
    let end = s.leader_position();
    let short = end.truncate().distance(start.truncate());
    assert!(short <= 1.0, "arrived {short:.2} m from the start {start:?}, at {end:?}");
}

/// The closest point of the segment a-b to p.
fn closest_on_seg(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    a + ab * t
}

/// True when the open segments p0-p1 and q0-q1 cross (proper crossings only).
fn crosses(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> bool {
    let d1 = (p1 - p0).perp_dot(q0 - p0);
    let d2 = (p1 - p0).perp_dot(q1 - p0);
    let d3 = (q1 - q0).perp_dot(p0 - q0);
    let d4 = (q1 - q0).perp_dot(p1 - q0);
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// One sub-step of the slide (`move_slide`, the step a walk takes), replayed with the
/// walk mesh's public queries: the candidate, each blocker that pushes it out (with
/// its distance against the radius), the ground under the pushed candidate, and the
/// blocker that refuses the step. The rules are the slide's; the numbers come from
/// the same blockers and ground queries.
fn explain_slide_step(w: &World, pos: Vec3, delta: Vec2, radius: f32) -> Vec<String> {
    let mut out = vec![format!("pos {pos:?}, delta {delta:?} (step {:.3} m), radius {radius}", delta.length())];
    let mut cand = pos.xy() + delta;
    for pass in 0..4 {
        let near = w.walk.blockers_near(Vec3::new(cand.x, cand.y, pos.z), radius + 0.5);
        let mut moved = false;
        for &(a, b, room, mesh) in &near {
            let c = closest_on_seg(cand, a.xy(), b.xy());
            let dist = (cand - c).length();
            if dist < radius {
                out.push(format!("  push pass {pass}: {a:?}-{b:?} mesh {mesh} room {room}: {dist:.3} m < radius {radius} from candidate {cand:?}"));
                let n = if dist > 1e-6 { (cand - c) / dist } else { Vec2::X };
                cand = c + n * (radius + 1e-3);
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    out.push(format!("  candidate after push-out {cand:?}"));
    match w.walk.ground(cand.x, cand.y, pos.z) {
        None => out.push("  refused: no walkable ground under the candidate".to_owned()),
        Some(h) => {
            out.push(format!("  ground under the candidate z {:.3} (from z {:.3}; a drop past {STEP_DOWN} m is refused)", h.z, pos.z));
            if h.z < pos.z - STEP_DOWN {
                out.push("  refused: the drop is too far".to_owned());
            }
            for (a, b, room, mesh) in w.walk.blockers_near(Vec3::new(cand.x, cand.y, h.z), (cand - pos.xy()).length() + 0.1) {
                if crosses(pos.xy(), cand, a.xy(), b.xy()) {
                    out.push(format!("  refused: the step crosses {a:?}-{b:?} mesh {mesh} room {room}"));
                }
            }
        }
    }
    out
}

/// Diagnostic, not a check (run with --ignored --nocapture): the first step of the
/// control walk from (-3.013, -3.147, 9.07) toward the stand by the closet placeable.
/// Prints the aim the walk steers to, the slide's decisions for that step, and the
/// position walk_leader_toward actually gives the leader.
#[test]
#[ignore]
fn diagnose_the_first_slide_step_by_the_closet_door() {
    let Some(mut s) = peragus_in_control("diag-step") else { return };
    let say = |line: String| {
        let _ = writeln!(std::io::stderr(), "{line}");
    };
    let spot = Vec3::new(-3.013, -3.147, 9.07);
    let leader = s.world.party_leader();
    s.world.place(leader, spot);
    let at = s.leader_position();
    let (radius, speed) = s.world.get(leader).and_then(|o| o.creature()).map_or((0.3, 1.75), |c| (c.radius, c.walk_speed));
    say(format!("leader at {at:?}, creature radius {radius}, walk speed {speed}"));
    let target = s.world.iter().find(|o| o.kind == ObjectKind::Placeable && o.position.truncate().distance(Vec2::new(0.85, -3.11)) < 1.5).map(|o| o.id);
    let target = target.expect("101per has a placeable at (0.85, -3.11)");
    let stand = s.world.use_point(at, target).expect("a stand within use reach of it");
    say(format!("stand {stand:?}"));
    // The control walk steers every frame; the first frame with a route gives its aim.
    let mut aim = None;
    for _ in 0..400 {
        let steered = s.world.steer_leader(at, stand);
        if !s.world.leader_route_pending() {
            aim = steered;
            break;
        }
    }
    let Some(aim) = aim else {
        say("steer_leader gave no aim".to_owned());
        return;
    };
    say(format!("aim {aim:?}"));
    let to = (aim - at).truncate();
    let step = (speed * support::DT).min(to.length());
    let delta = to / to.length() * step;
    for line in explain_slide_step(&s.world, at, delta, radius.min(0.5)) {
        say(line);
    }
    s.world.walk_leader_toward(aim, support::DT, false);
    say(format!("walk_leader_toward gave the leader {:?}", s.leader_position()));
    say(surroundings(&s));
}
