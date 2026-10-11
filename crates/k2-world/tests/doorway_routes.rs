//! Routes against closed doors. A route leg never crosses an enabled blocker (a
//! wall, a placeable, a closed door's walkmesh): the slide refuses such a step, so
//! a planned route that crosses one is a route the walk cannot follow. Skips
//! (passes) when no install is found at K2_INSTALL or ../.. .

use glam::{Vec3, Vec3Swizzles};
use k2_walk::{SpotStep, FULL_BUDGET};
use k2_world::control::{Command, Control, Env, ObjectArgs};
use k2_world::{ObjectKind, World};
mod support;

/// 101per entered, with no intro run: doors in their module state.
fn world() -> Option<World> {
    let mut w = World::new(support::install()?).expect("the install loads into a world");
    w.enter_module("101per", None).expect("101per enters");
    let _ = w.drain_events();
    Some(w)
}

/// Opens the PeragusDoor1 doors (the player's way through them to the closet), as
/// door_talk does: a use opens each door once its swing has run.
fn open_peragus_doors(w: &mut World) {
    let pc = w.party_leader();
    let doors: Vec<u32> = w.iter().filter(|o| o.kind == ObjectKind::Door && o.tag == "PeragusDoor1").map(|o| o.id).collect();
    assert!(!doors.is_empty(), "101per has PeragusDoor1 doors");
    for &d in &doors {
        w.set_door_open(d, true, pc);
    }
    for _ in 0..120 {
        w.tick(1.0 / 30.0);
    }
}

/// The first enabled blocker the leg a-b crosses, as (its ends, room, mesh).
fn crossing(w: &World, a: Vec3, b: Vec3) -> Option<(Vec3, Vec3, bool, u32)> {
    let mid = (a + b) * 0.5;
    let reach = a.xy().distance(b.xy()) * 0.5 + 0.5;
    w.walk.blockers_near(mid, reach).into_iter().find(|&(p, q, _, _)| support::crosses(a.xy(), b.xy(), p.xy(), q.xy()))
}

/// 101per with the intro's talks run to their end, as door_talk's settled world is.
fn settled_world() -> Option<World> {
    let mut w = world()?;
    w.auto_dialog = true;
    for _ in 0..300 {
        w.tick(0.1);
    }
    w.auto_dialog = false;
    w.end_conversation(true);
    let _ = w.drain_events();
    Some(w)
}

/// The PeragusDoor1 nearest `p`.
fn nearest_peragus_door(w: &World, p: Vec3) -> u32 {
    w.iter()
        .filter(|o| o.kind == ObjectKind::Door && o.tag == "PeragusDoor1")
        .min_by(|a, b| a.position.truncate().distance(p.xy()).total_cmp(&b.position.truncate().distance(p.xy())))
        .map(|o| o.id)
        .expect("101per has PeragusDoor1 doors")
}

/// A leader that stands in reach of a closed door uses it from where it stands: its
/// own spot is its use stand, and the control interact does not walk.
#[test]
fn a_door_in_reach_of_the_leader_opens_from_where_the_leader_stands() {
    let Some(mut w) = settled_world() else { return };
    let pc = w.party_leader();
    let door = nearest_peragus_door(&w, Vec3::new(-13.35, -4.21, 0.0));
    open_peragus_doors_but(&mut w, door);
    w.place(pc, Vec3::new(-13.66, -1.25, 9.11));
    let at = w.get(pc).expect("the leader is in the world").position;
    assert!(w.in_use_reach(at, door), "the leader at {at:?} is out of reach of PeragusDoor1 {door}");
    assert_eq!(w.use_point(at, door), Some(at), "a leader in reach has its own spot as the use stand");
    let root = std::env::temp_dir();
    let env = Env { save_root: &root, paused: false, movie: false };
    let mut control = Control::default();
    let reply = match control.submit(&mut w, &env, Command::Interact(ObjectArgs { object: door })) {
        Some(reply) => reply,
        None => loop {
            w.tick(1.0 / 30.0);
            let _ = w.drain_events();
            if let Some(reply) = control.poll(&mut w, &env) {
                break reply;
            }
        },
    };
    assert!(reply.ok, "the interact on PeragusDoor1 {door} in reach: {} {:?}", reply.message, reply.feedback);
    for _ in 0..120 {
        w.tick(1.0 / 30.0);
    }
    let after = w.get(pc).expect("the leader is in the world").position;
    assert_eq!(after, at, "the leader moved from {at:?} to {after:?} while the door was used from where it stands");
    assert!(w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open), "PeragusDoor1 {door} did not open from where the leader stands");
}

/// Opens every PeragusDoor1 except `keep_shut`, as the player has by then.
fn open_peragus_doors_but(w: &mut World, keep_shut: u32) {
    let pc = w.party_leader();
    let doors: Vec<u32> = w.iter().filter(|o| o.kind == ObjectKind::Door && o.tag == "PeragusDoor1" && o.id != keep_shut).map(|o| o.id).collect();
    for &d in &doors {
        w.set_door_open(d, true, pc);
    }
    for _ in 0..120 {
        w.tick(1.0 / 30.0);
    }
}

/// The first leg of a route that crosses an enabled blocker, if any.
fn first_crossing(w: &World, points: &[Vec3]) -> Option<(usize, (Vec3, Vec3, bool, u32))> {
    points.windows(2).enumerate().find_map(|(i, leg)| crossing(w, leg[0], leg[1]).map(|c| (i, c)))
}

#[test]
fn a_route_to_the_closet_stand_crosses_no_closed_door() {
    let Some(mut w) = world() else { return };
    open_peragus_doors(&mut w);
    let pc = w.party_leader();
    let wake = w.by_tag("WP_wakeup_donnie", 0).expect("101per has WP_wakeup_donnie");
    let wake_at = w.get(wake).expect("the waypoint is in the world").position;
    w.place(pc, wake_at);
    let at = w.get(pc).expect("the leader is in the world").position;
    let closet = w.by_tag("MedClosetDoor", 0).expect("101per has MedClosetDoor");
    // With no stand in reach of the closed door there is no route, and the checks below
    // see only the leader's own point.
    let stand = w.use_point(at, closet).expect("the closet door has a use stand from the wake-up spot");
    assert_ne!(stand, at, "the closet door is out of the leader's reach, so its stand is another spot");
    let clearance = w.leader_clearance();

    // The route query (smoothed, as find_path gives it).
    let route = w.walk.find_path(at, stand, clearance).expect("a route to the closet stand");
    let mut legs = vec![at];
    legs.extend(route);
    if let Some((i, c)) = first_crossing(&w, &legs) {
        panic!("the find_path route to the closet stand crosses {c:?} on leg {i}");
    }

    // The steering search (the unsmoothed node chain the walk follows).
    let mut search = w.walk.spot_search(at, &[stand], clearance, FULL_BUDGET, |p| Some(p.xy().distance(stand.xy()))).expect("a steering search to the closet stand");
    let route = loop {
        match w.walk.spot_step(&mut search, 20_000) {
            SpotStep::Pending => {}
            SpotStep::Found(route) => break route,
            SpotStep::Miss(miss) => panic!("the steering search to the closet stand missed: {miss:?}"),
        }
    };
    let mut legs = vec![at];
    legs.extend(route);
    legs.push(stand);
    if let Some((i, c)) = first_crossing(&w, &legs) {
        panic!("the steering route to the closet stand crosses {c:?} on leg {i}");
    }
}
