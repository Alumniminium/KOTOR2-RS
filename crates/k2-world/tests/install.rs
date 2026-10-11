//! Tests against the real install. They skip (pass) when no install is
//! found at K2_INSTALL or ../.. (the repo inside the game folder).

use glam::Vec3;
use k2_res::Install;
use k2_world::{Action, ObjectKind, Target, World, WorldEvent};

const DT: f32 = 1.0 / 30.0;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

fn ticks(w: &mut World, seconds: f32) {
    for _ in 0..(seconds / DT) as usize {
        w.tick(DT);
    }
}

#[test]
fn peragus_loads_with_player_on_ground() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    let p = w.get(w.party_leader()).unwrap().position;
    let g = w.walk.ground(p.x, p.y, p.z + 1.0).expect("ground under the entry point");
    assert!((g.z - p.z).abs() < 0.05);
    assert!(w.iter().filter(|o| o.kind == ObjectKind::Door).count() > 10);
    ticks(&mut w, 10.0);
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

#[test]
fn door_transition_keeps_globals_and_places_party_at_waypoint() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    w.globals.set_number("K_TEST_ONLY", 7);
    let lift = w.by_tag("PERDoor105", 0).expect("turbolift door");
    let pc = w.party_leader();
    w.use_object(pc, lift);
    let t = w.pending_transition.clone().expect("transition requested");
    assert_eq!(t.module, "105per");
    w.enter_module(&t.module, t.waypoint_tag.as_deref()).unwrap();
    assert_eq!(w.party_leader(), pc, "player keeps its id");
    assert_eq!(w.globals.number("K_TEST_ONLY"), 7);
    let wp = w.by_tag(t.waypoint_tag.as_deref().unwrap(), 0).expect("destination waypoint");
    let (a, b) = (w.get(pc).unwrap().position, w.get(wp).unwrap().position);
    assert!(a.truncate().distance(b.truncate()) < 0.01, "{a} vs {b}");
}

#[test]
fn atton_conversation_offers_choices_and_filters_used_replies() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    let atton = w.by_tag("Atton", 0).unwrap();
    let pc = w.party_leader();
    w.talk_to(pc, atton);
    let mut lines = 0;
    let mut max_choices = 0;
    while let Some(v) = w.conversation_view() {
        lines += 1;
        max_choices = max_choices.max(v.replies.len());
        if lines > 100 {
            break;
        }
        if v.replies.is_empty() {
            w.end_conversation(false);
        } else {
            w.choose_reply(0);
        }
    }
    assert!(lines >= 5, "only {lines} lines");
    assert!(max_choices >= 4);
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
    assert!(w.conversation.is_none());
}

#[test]
fn scripted_move_walks_on_the_walkmesh() {
    let Some(mut w) = world() else { return };
    w.enter_module("201tel", None).unwrap();
    let pc = w.party_leader();
    let start = w.get(pc).unwrap().position;
    // A point a few meters away that a straight walk reaches.
    let dest = (0..16)
        .map(|k| {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            start + Vec3::new(a.cos(), a.sin(), 0.0) * 4.0
        })
        .find(|&d| {
            let mut p = start;
            for _ in 0..80 {
                let step = (d - p).truncate().clamp_length_max(0.1);
                p = w.walk.move_slide(p, step, 0.3);
            }
            p.truncate().distance(d.truncate()) < 0.3
        })
        .expect("reachable point near the entry");
    w.push_action(pc, Action::MoveTo { target: Target::Point(dest), run: false, range: 0.3, timeout: None, elapsed: 0.0, stuck: 0.0 });
    ticks(&mut w, 6.0);
    let end = w.get(pc).unwrap().position;
    assert!(w.get(pc).unwrap().actions.is_empty());
    assert!(end.truncate().distance(dest.truncate()) < 0.5, "did not arrive: {start} -> {end}, wanted {dest}");
    assert!(w.walk.is_walkable(end), "ended off the walkmesh at {end}");
}

#[test]
fn doors_open_and_report() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    w.drain_events();
    let door = w.iter().find(|o| o.conversation.is_none() && o.door().is_some_and(|d| !d.locked && !d.open && d.linked_to_module.is_none())).unwrap().id;
    let pc = w.party_leader();
    w.use_object(pc, door);
    assert!(w.get(door).unwrap().door().unwrap().open);
    assert!(w.drain_events().iter().any(|e| matches!(e, WorldEvent::DoorChanged { open: true, .. })));
}

#[test]
fn prologue_switches_control_to_t3m4() {
    let Some(mut w) = world() else { return };
    w.enter_module("001ebo", None).unwrap();
    ticks(&mut w, 1.0);
    let leader = w.party_leader();
    assert_eq!(w.get(leader).unwrap().tag, "T3M4");
    assert!(w.stashed_pc.is_some());
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}
