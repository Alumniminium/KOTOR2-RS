//! Doors and containers with a conversation, against the real install. A
//! closed door with a passing start talks and stays shut; a closed door whose
//! start fails opens; an open door closes with no line; a locked door says why
//! and stays shut; a container talks and then opens; an unlock opens the door at
//! once and says nothing. Skips (passes, printing why) when no install is found
//! at K2_INSTALL or ../.. . With K2_INSTALL set, an install that does not load is
//! a failure.

use std::io::Write;

use glam::{Vec2, Vec3};
use k2_script::ObjectId;
use k2_world::craft::SKILL_SECURITY;
use k2_world::object::{Action, Target};
use k2_world::{ObjectKind, ScriptCtx, World, WorldEvent};

mod support;

use support::{install, ring};

/// `module` entered with nothing run: the state at load.
fn fresh(module: &str) -> Option<World> {
    let mut w = World::new(install()?).expect("the install loads into a world");
    w.enter_module(module, None).unwrap();
    w.drain_events();
    Some(w)
}

/// `module` entered, its entry cutscenes run out, and no conversation open.
fn settled(module: &str) -> Option<World> {
    let mut w = World::new(install()?).expect("the install loads into a world");
    w.enter_module(module, None).unwrap();
    w.auto_dialog = true;
    for _ in 0..300 {
        w.tick(0.1);
    }
    w.auto_dialog = false;
    w.end_conversation(true);
    w.drain_events();
    Some(w)
}

fn door_open(w: &World, door: ObjectId) -> bool {
    w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open)
}

fn door_locked(w: &World, door: ObjectId) -> bool {
    w.get(door).and_then(|o| o.door()).is_some_and(|d| d.locked)
}

/// Whether a conversation is open or a bare line was said in `evs`.
fn said(w: &World, evs: &[WorldEvent]) -> bool {
    w.conversation.is_some() || evs.iter().any(|e| matches!(e, WorldEvent::BarkLine { .. }))
}

#[test]
fn a_closed_door_with_a_passing_start_talks_and_stays_shut() {
    let Some(mut w) = settled("232tel") else { return };
    let pc = w.party_leader();
    let door = w.by_tag("TelosDoor3A2", 0).expect("232tel has TelosDoor3A2");
    assert!(!door_open(&w, door), "TelosDoor3A2 starts shut (OpenState 0)");

    w.use_object(pc, door);
    let evs = w.drain_events();
    assert!(w.conversation.is_some(), "doorm's start passes, so its line is the answer");
    assert!(!door_open(&w, door), "a closed door with a passing start does not open");
    assert!(!evs.iter().any(|e| matches!(e, WorldEvent::BarkLine { .. })), "doorm is a conversation, not a bare line");
}

#[test]
fn an_open_door_with_a_conversation_closes_without_a_line() {
    let Some(mut w) = settled("232tel") else { return };
    let pc = w.party_leader();
    let door = w.by_tag("TelosDoor3A1", 0).expect("232tel has TelosDoor3A1");
    assert!(door_open(&w, door), "TelosDoor3A1 starts open (OpenState 1)");

    w.use_object(pc, door);
    let evs = w.drain_events();
    assert!(!door_open(&w, door), "the use closes the open door");
    assert!(!said(&w, &evs), "closing says nothing: the conversation is for a shut door");
}

#[test]
fn a_locked_door_says_why_and_stays_locked() {
    let Some(mut w) = settled("220tel") else { return };
    let pc = w.party_leader();
    let door = w
        .iter()
        .find(|o| o.kind == ObjectKind::Door && o.conversation.as_deref() == Some("sealeddoor"))
        .map(|o| o.id)
        .expect("220tel has a locked door with sealeddoor");
    assert!(door_locked(&w, door), "the door starts locked");

    w.use_object(pc, door);
    let evs = w.drain_events();
    assert!(said(&w, &evs), "the locked door's line is its answer");
    assert!(door_locked(&w, door), "a locked door stays locked");
    assert!(!door_open(&w, door), "a locked door stays shut");
}

#[test]
fn a_container_with_a_conversation_talks_then_opens() {
    let Some(mut w) = settled("001ebo") else { return };
    let pc = w.party_leader();
    let body = w
        .iter()
        .find(|o| o.kind == ObjectKind::Placeable && o.conversation.as_deref() == Some("kre_body"))
        .map(|o| o.id)
        .expect("001ebo has the body with kre_body");

    w.use_object(pc, body);
    let evs = w.drain_events();
    assert!(said(&w, &evs), "kre_body's start passes, so the body talks");
    assert_eq!(w.inventory.open_container, Some(body), "the container still opens after its line");
}

#[test]
fn an_unlock_opens_the_door_without_a_line() {
    let Some(mut w) = settled("101per") else { return };
    let pc = w.party_leader();
    let door = w.by_tag("MedClosetDoor", 0).expect("101per has MedClosetDoor");
    assert!(door_locked(&w, door), "MedClosetDoor starts locked");

    // OpenLockDC is 28. Out of combat the roll is 20, so rank 40 always passes.
    w.set_skill_rank(pc, SKILL_SECURITY, 40);
    w.try_unlock(pc, door).expect("the unlock succeeds");
    let evs = w.drain_events();
    assert!(!door_locked(&w, door), "the unlock clears the lock");
    assert!(door_open(&w, door), "the unlock opens the door at once");
    assert!(!said(&w, &evs), "the lock's line is gone once it is unlocked");
}

#[test]
fn a_closed_door_with_a_conversation_either_talks_or_opens() {
    let Some(mut w) = settled("232tel") else { return };
    let pc = w.party_leader();
    let doors: Vec<_> = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Door && o.conversation.as_deref().is_some_and(|c| !c.is_empty()))
        .filter(|o| o.door().is_some_and(|d| !d.locked && !d.is_static && !d.open && d.linked_to_module.as_deref().is_none_or(str::is_empty)))
        .map(|o| o.id)
        .collect();
    assert!(!doors.is_empty(), "232tel has closed doors with a conversation");

    let mut tested = 0;
    for door in doors {
        // An earlier use may have opened this door (a door script can open another).
        if door_open(&w, door) {
            continue;
        }
        let tag = w.get(door).map(|o| o.tag.clone()).unwrap_or_default();
        w.use_object(pc, door);
        let evs = w.drain_events();
        let open = door_open(&w, door);
        let talked = said(&w, &evs);
        assert!(open != talked, "{tag}: open {open}, said {talked}: a closed door opens or talks, not both or neither");
        tested += 1;
        w.end_conversation(true);
    }
    assert!(tested > 0, "at least one closed door was used");
}

#[test]
fn a_static_door_talks_and_never_opens_on_use() {
    let Some(mut w) = settled("222tel") else { return };
    let pc = w.party_leader();
    let door = w.by_tag("d_cantina", 0).expect("222tel has the cantina door");
    assert!(w.get(door).and_then(|o| o.door()).is_some_and(|d| d.is_static), "d_cantina is static");
    assert!(!door_open(&w, door), "d_cantina starts shut");

    w.use_object(pc, door);
    let evs = w.drain_events();
    assert!(said(&w, &evs), "the cantina door's line (doorcant) is its answer");
    assert!(!door_open(&w, door), "a static door never opens on use");
}

#[test]
fn a_closed_door_whose_start_fails_opens() {
    let Some(mut w) = settled("101per") else { return };
    let pc = w.party_leader();
    let door = w.by_tag("GuardRoomDr", 0).expect("101per has GuardRoomDr");

    // grdrmdr's start is c_talkedto: true while local bool 10 (the talked flag,
    // read by UT_GetTalkedToBooleanFlag) is unset. Nothing on grdrmdr or its door
    // sets that flag, so the first use talks, and the test then sets it by hand.
    w.use_object(pc, door);
    assert!(w.conversation.is_some(), "the first use starts grdrmdr");
    assert!(!door_open(&w, door), "the talk keeps the door shut");
    w.end_conversation(false);

    w.get_mut(door).expect("the door is in the world").local_bools.insert(10, true);
    w.use_object(pc, door);
    let evs = w.drain_events();
    assert!(door_open(&w, door), "with the flag set, the start fails and the same use opens the door");
    assert!(!said(&w, &evs), "opening says nothing");
}

#[test]
fn a_successful_unlock_fires_onclick() {
    let Some(mut w) = settled("101per") else { return };
    let pc = w.party_leader();
    let door = w.by_tag("MedClosetDoor", 0).expect("101per has MedClosetDoor");
    // k_xp60 gives 50 XP to the first PC. The door's OnOpen is cleared so that
    // only the OnClick the unlock fires can give the XP.
    let obj = w.get_mut(door).expect("the door is in the world");
    obj.scripts.insert("OnOpen".into(), String::new());
    obj.scripts.insert("OnClick".into(), "k_xp60".into());

    w.set_skill_rank(pc, SKILL_SECURITY, 40);
    w.try_unlock(pc, door).expect("the unlock succeeds");
    let evs = w.drain_events();
    assert!(evs.iter().any(|e| matches!(e, WorldEvent::XpGained { amount: 50, .. })), "OnClick ran on the unlock");
}

#[test]
fn a_static_door_unlock_clears_the_lock_and_talks_without_opening() {
    let Some(mut w) = settled("153har") else { return };
    let pc = w.party_leader();
    let door = w
        .iter()
        .find(|o| o.kind == ObjectKind::Door && o.conversation.as_deref() == Some("blastdr"))
        .map(|o| o.id)
        .expect("153har has a static locked door with blastdr");
    assert!(door_locked(&w, door), "the door starts locked");
    // The door needs a key this test does not carry, so the key requirement is
    // switched off and the Security skill opens the lock (OpenLockDC is 7).
    w.get_mut(door).and_then(|o| o.door_mut()).expect("the door has a door state").key_required = false;

    w.set_skill_rank(pc, SKILL_SECURITY, 40);
    w.try_unlock(pc, door).expect("the unlock succeeds");
    let evs = w.drain_events();
    assert!(!door_locked(&w, door), "the unlock clears the lock");
    assert!(!door_open(&w, door), "a static door does not open on unlock");
    assert!(said(&w, &evs), "the static door talks as a use does (blastdr has no start condition)");
}

/// Opens every PeragusDoor1 by use, as a player would, and lets the walk follow.
fn use_peragus_doors(w: &mut World, pc: ObjectId) {
    let doors: Vec<ObjectId> = w.iter().filter(|o| o.kind == ObjectKind::Door && o.tag == "PeragusDoor1").map(|o| o.id).collect();
    assert!(!doors.is_empty(), "101per has PeragusDoor1 doors");
    for &d in &doors {
        w.use_object(pc, d);
    }
    for _ in 0..120 {
        w.tick(1.0 / 30.0);
    }
    for &d in &doors {
        assert!(door_open(w, d), "a PeragusDoor1 opens by use");
    }
}

#[test]
fn the_exit_to_102per_is_reachable_from_the_guardroom_speech_point() {
    let Some(mut w) = fresh("101per") else { return };
    let pc = w.party_leader();
    let radius = w.get(pc).and_then(|o| o.creature()).expect("the leader is a creature").radius;
    // a_kreiaspch1 moves the PC to WP_player_speech, beside GuardRoomDr.
    let speech = w.by_tag("WP_player_speech", 0).expect("101per has WP_player_speech");
    let from = w.get(speech).expect("the waypoint is in the world").position;

    open_hatch_by_script(&mut w);
    use_peragus_doors(&mut w, pc);
    let guard = w.by_tag("GuardRoomDr", 0).expect("101per has GuardRoomDr");
    assert!(!door_open(&w, guard), "GuardRoomDr stays shut");

    let exit = w.by_tag("To_102PER", 0).expect("101per has the exit trigger To_102PER");
    let poly = w.get(exit).and_then(|o| o.trigger()).expect("To_102PER is a trigger").polygon.clone();
    let goal = walkable_point_in(&w, &poly).expect("To_102PER has a walkable point inside its polygon");
    let route = w.walk.find_path(from, goal, radius);
    assert!(route.is_some(), "no walk route from {from:?} to {goal:?}: {}", route_line(&w, from, goal, radius));

    // A closed GuardRoomDr must also block on the walk grid for its absence from the
    // route to mean anything: its live segments count as blockers, and opening it removes them.
    let at = w.get(guard).expect("the door is in the world").position;
    let closed = w.walk.blockers_near(at, 2.0).iter().filter(|b| !b.2).count();
    w.set_door_open(guard, true, pc);
    for _ in 0..120 {
        w.tick(1.0 / 30.0);
    }
    let opened = w.walk.blockers_near(at, 2.0).iter().filter(|b| !b.2).count();
    assert!(opened < closed, "GuardRoomDr is not a walk blocker when closed (door blockers {closed}, open {opened})");
}

/// The entry is a walkmesh pocket: the Exile starts lying in the medbay (two
/// walkable triangles, cut off by a non-walkable wall), and the Awaken trigger
/// (k_awaken, OnEnter) moves the PC out of it. Once the entry cutscenes run out,
/// the leader is out of the pocket and can walk to the exit, with the
/// PeragusDoor1s opened by use as a player opens them.
#[test]
fn the_exit_to_102per_is_reachable_after_the_intro() {
    let Some(mut w) = settled("101per") else { return };
    let pc = w.party_leader();
    let start = w.by_tag("WP_player_start", 0).and_then(|s| w.get(s)).map(|s| s.position).expect("101per has WP_player_start");
    let at = w.get(pc).expect("the leader is in the world").position;
    let radius = w.get(pc).and_then(|o| o.creature()).expect("the leader is a creature").radius;
    // The pocket is about 2.5 m across. same_region is no test of it: its goal snap
    // takes any node of the leader's region within 3 m of the target, and the main
    // floor has nodes that close to the start.
    let away = Vec2::new(at.x, at.y).distance(Vec2::new(start.x, start.y));
    let _ = writeln!(std::io::stderr(), "leader after the intro: {at:?}, {away:.2} m from WP_player_start {start:?}");
    assert!(away > 3.0, "the intro left the leader at {at:?}, beside its start {start:?}");
    assert!(w.walk.ground(at.x, at.y, at.z).is_some(), "the leader {at:?} stands on no walk ground");
    // The emergency hatch (102PERDoor) gates the exit; a_door102per opens it in the story.
    open_hatch_by_script(&mut w);
    use_peragus_doors(&mut w, pc);
    // MedBayDoor gates the wake-up spot: its dialog says to bash it with a plasma torch.
    cut_medbay_door_with_torch(&mut w, pc);
    let at = w.get(pc).expect("the leader is in the world").position;
    let (_, _, goal) = exit_points(&w);
    assert!(w.walk.same_region(at, goal), "the leader and the exit {goal:?} are in different walk regions: {}", route_line(&w, at, goal, radius));
    let route = w.walk.find_path(at, goal, radius);
    assert!(route.is_some(), "no walk route from the leader {at:?} to the exit {goal:?}: {}", route_line(&w, at, goal, radius));
    // A walk stops within 0.5 m of its target, and the walkable goal above sits 0.5 m
    // inside the trigger's south edge, so the leader can stop outside the trigger. The
    // walk goes to the trigger's middle instead. Entering it (tick.rs run_triggers: the
    // leader's xy inside the polygon, alive) puts the leader in `inside`, and a transition
    // trigger then requests the change to its module (request_transition sets
    // pending_transition).
    let exit = w.by_tag("To_102PER", 0).expect("101per has the exit trigger To_102PER");
    let poly = w.get(exit).and_then(|o| o.trigger()).expect("To_102PER is a trigger").polygon.clone();
    let mid = poly.iter().fold(Vec3::ZERO, |a, p| a + Vec3::new(p[0], p[1], p[2])) / poly.len() as f32;
    let mid = ground_point(&w, mid.truncate(), mid.z).expect("To_102PER has walkable ground at its middle");
    walk_leg(&mut w, pc, mid, "into To_102PER");
    w.tick(1.0 / 30.0);
    let registered = w.get(exit).and_then(|o| o.trigger()).is_some_and(|t| t.inside.contains(&pc));
    assert!(registered, "the leader at {mid:?} is not inside To_102PER: its polygon {poly:?}");
    let transit = w.pending_transition.as_ref().map(|t| t.module.clone());
    assert_eq!(transit.as_deref(), Some("102per"), "entering To_102PER did not request the change to 102per");
}

/// The walk from the leader to a walkable point inside the To_102PER trigger:
/// the leader's position, its collision radius, and the goal.
fn exit_points(w: &World) -> (Vec3, f32, Vec3) {
    let leader = w.party_leader();
    let from = w.get(leader).expect("the leader is in the world").position;
    let radius = w.get(leader).and_then(|o| o.creature()).expect("the leader is a creature").radius;
    let exit = w.by_tag("To_102PER", 0).expect("101per has the exit trigger To_102PER");
    let poly = w.get(exit).and_then(|o| o.trigger()).expect("To_102PER is a trigger").polygon.clone();
    let goal = walkable_point_in(w, &poly).expect("To_102PER has a walkable point inside its polygon");
    (from, radius, goal)
}

/// A point of the trigger polygon (corners in game coordinates) that lies inside
/// it and on the walkmesh: a 0.5 m grid over the polygon's bounds.
fn walkable_point_in(w: &World, poly: &[[f32; 3]]) -> Option<Vec3> {
    let xy: Vec<Vec2> = poly.iter().map(|p| Vec2::new(p[0], p[1])).collect();
    let lo = xy.iter().fold(Vec2::splat(f32::MAX), |a, &p| a.min(p));
    let hi = xy.iter().fold(Vec2::splat(f32::MIN), |a, &p| a.max(p));
    let z = poly.iter().map(|p| p[2]).sum::<f32>() / poly.len() as f32;
    let mut y = lo.y;
    while y <= hi.y {
        let mut x = lo.x;
        while x <= hi.x {
            if inside(Vec2::new(x, y), &xy)
                && let Some(h) = w.walk.ground(x, y, z)
            {
                return Some(Vec3::new(x, y, h.z));
            }
            x += 0.5;
        }
        y += 0.5;
    }
    None
}

/// Even-odd test of `p` against the polygon `poly` in the plane.
fn inside(p: Vec2, poly: &[Vec2]) -> bool {
    let n = poly.len();
    let mut hit = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            hit = !hit;
        }
    }
    hit
}

/// Whether `from` and `to` share a walkmesh region, and what the checked route
/// search says about the way between them at the full budget and at ten times it.
fn route_line(w: &World, from: Vec3, to: Vec3, radius: f32) -> String {
    let same = w.walk.same_region(from, to);
    let at = |budget: usize| match w.walk.find_route_nodes_checked(from, to, radius, budget) {
        Ok(r) => format!("route of {} nodes, cut {}", r.nodes.len(), r.cut),
        Err(miss) => format!("{miss:?}"),
    };
    format!(
        "same region {same}; checked search {} (x10: {}); find_path {}",
        at(k2_walk::FULL_BUDGET),
        at(k2_walk::FULL_BUDGET * 10),
        w.walk.find_path(from, to, radius).is_some()
    )
}

/// Distance in the plane from `p` to the polyline `pts`.
fn dist_to_route(p: Vec3, pts: &[Vec3]) -> f32 {
    let p = p.truncate();
    pts.windows(2).map(|s| dist_to_segment(p, s[0].truncate(), s[1].truncate())).fold(f32::INFINITY, f32::min)
}

/// A copy of k2-world's nav.rs `dist_to_seg`, which is private there.
fn dist_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

fn door_tag(w: &World, d: ObjectId) -> String {
    w.get(d).map(|o| o.tag.clone()).unwrap_or_default()
}

fn opens_none(_: &World, _: ObjectId) -> bool {
    false
}

fn opens_guardroom(w: &World, d: ObjectId) -> bool {
    door_tag(w, d) == "GuardRoomDr"
}

/// Unlocked and not static: the doors a use opens without a key or a script.
fn opens_by_use(w: &World, d: ObjectId) -> bool {
    w.get(d).and_then(|o| o.door()).is_some_and(|s| !s.locked && !s.is_static)
}

fn opens_by_use_but_guardroom(w: &World, d: ObjectId) -> bool {
    opens_by_use(w, d) && door_tag(w, d) != "GuardRoomDr"
}

fn opens_all(_: &World, _: ObjectId) -> bool {
    true
}

/// The walk route to the exit with each door `pick` selects opened the way a
/// use does, the walk followed for a few seconds.
fn route_with_doors_open(w: &mut World, pick: fn(&World, ObjectId) -> bool) -> Option<Vec<Vec3>> {
    let leader = w.party_leader();
    let doors: Vec<ObjectId> = w.iter().filter(|o| o.kind == ObjectKind::Door).map(|o| o.id).collect();
    for d in doors {
        if pick(w, d) {
            w.set_door_open(d, true, leader);
        }
    }
    for _ in 0..120 {
        w.tick(1.0 / 30.0);
    }
    let (from, radius, to) = exit_points(w);
    w.walk.find_path(from, to, radius)
}

/// Diagnostic for the exit route in 101per. Prints the doors, the route with
/// each set of doors opened, and the doors and placeables near each route.
/// Run with --ignored --nocapture; it asserts nothing.
#[test]
#[ignore]
fn diagnose_the_exit_route_in_101per() {
    let Some(w) = fresh("101per") else { return };
    let (from, radius, to) = exit_points(&w);
    println!("entry {from:?} radius {radius} exit {to:?}");
    let doors: Vec<ObjectId> = w.iter().filter(|o| o.kind == ObjectKind::Door).map(|o| o.id).collect();
    for &d in &doors {
        let o = w.get(d).expect("door");
        let s = o.door().expect("door state");
        println!(
            "door {:<18} {:<16} at ({:8.2}, {:8.2}) locked {} open {} static {} key {} conv {:?} OnOpen {:?} OnFailToOpen {:?}",
            o.tag,
            o.template,
            o.position.x,
            o.position.y,
            s.locked,
            s.open,
            s.is_static,
            s.key_required,
            o.conversation,
            o.script("OnOpen"),
            o.script("OnFailToOpen")
        );
    }

    let scenarios: [(&str, fn(&World, ObjectId) -> bool); 5] = [
        ("nothing opened", opens_none),
        ("GuardRoomDr opened", opens_guardroom),
        ("every unlocked, non-static door opened", opens_by_use),
        ("the same with GuardRoomDr left shut", opens_by_use_but_guardroom),
        ("every door opened", opens_all),
    ];
    for (name, pick) in scenarios {
        let Some(mut w) = fresh("101per") else { return };
        match route_with_doors_open(&mut w, pick) {
            None => println!("{name}: no route"),
            Some(route) => {
                println!("{name}: route of {} waypoints, ends {:?}", route.len(), route.last());
                let mut pts = vec![from];
                pts.extend(route.iter().copied());
                let near: Vec<ObjectId> = w.iter().filter(|o| o.kind == ObjectKind::Door).map(|o| o.id).collect();
                for d in near {
                    let o = w.get(d).expect("door");
                    let dist = dist_to_route(o.position, &pts);
                    if dist < 2.5 {
                        let s = o.door().expect("door state");
                        println!("  door {} ({}) {dist:.2} m from the route: locked {} static {} open now {}", o.tag, o.template, s.locked, s.is_static, s.open);
                    }
                }
                let placeables: Vec<ObjectId> = w.iter().filter(|o| o.kind == ObjectKind::Placeable).map(|o| o.id).collect();
                for p in placeables {
                    let o = w.get(p).expect("placeable");
                    let dist = dist_to_route(o.position, &pts);
                    if dist < 1.5 {
                        println!("  placeable {} ({}) {dist:.2} m from the route", o.tag, o.template);
                    }
                }
            }
        }
    }
}

/// Connectivity diagnostic for the exit in 101per: whether the entry, the goal
/// inside To_102PER and the GuardRoomDr speech point share a walkmesh region, and
/// what the checked route search says at full and at ten times the budget. Doors
/// are listed by the walkmesh side they lie on (8 directions 1.5 m out: E entry's
/// region, w another walkable region, - not walkable). Run with --ignored --nocapture.
#[test]
#[ignore]
fn diagnose_the_exit_connectivity_in_101per() {
    let Some(mut w) = fresh("101per") else { return };
    let stats = w.walk.prepare_nav();
    println!("nav: {} nodes, {} edges, {} components", stats.nodes, stats.edges, stats.components);

    let (from, radius, goal) = exit_points(&w);
    let exit = w.by_tag("To_102PER", 0).expect("the exit trigger");
    let poly = w.get(exit).and_then(|o| o.trigger()).expect("trigger").polygon.clone();
    let centre = poly.iter().fold(Vec3::ZERO, |a, p| a + Vec3::new(p[0], p[1], p[2])) / poly.len() as f32;
    let speech_wp = w.by_tag("WP_player_speech", 0).expect("101per has WP_player_speech");
    let speech = w.get(speech_wp).expect("the waypoint is in the world").position;
    println!("entry {from:?} radius {radius}");
    println!("trigger corners {poly:?}");
    println!("centre {centre:?}: ground {:?}", w.walk.ground(centre.x, centre.y, centre.z).map(|h| h.z));
    println!("goal {goal:?}: ground {:?}", w.walk.ground(goal.x, goal.y, goal.z).map(|h| h.z));
    println!("speech {speech:?}: ground {:?}", w.walk.ground(speech.x, speech.y, speech.z).map(|h| h.z));
    println!("entry -> centre: {}", route_line(&w, from, centre, radius));
    println!("entry -> goal: {}", route_line(&w, from, goal, radius));
    println!("entry -> speech: {}", route_line(&w, from, speech, radius));
    println!("speech -> goal: {}", route_line(&w, speech, goal, radius));

    let doors: Vec<ObjectId> = w.iter().filter(|o| o.kind == ObjectKind::Door).map(|o| o.id).collect();
    for &d in &doors {
        let o = w.get(d).expect("door");
        let mut sides = String::new();
        for q in ring(o.position.truncate(), 1.5) {
            let (x, y) = (q.x, q.y);
            sides.push(match w.walk.ground(x, y, o.position.z) {
                None => '-',
                Some(_) if w.walk.same_region(from, Vec3::new(x, y, o.position.z)) => 'E',
                Some(_) => 'w',
            });
        }
        println!("door {:<18} {:<16} at ({:8.2}, {:8.2}) sides {sides}", o.tag, o.template, o.position.x, o.position.y);
    }

    let leader = w.party_leader();
    for &d in &doors {
        w.set_door_open(d, true, leader);
    }
    for _ in 0..120 {
        w.tick(1.0 / 30.0);
    }
    println!("with every door open:");
    println!("entry -> goal: {}", route_line(&w, from, goal, radius));
    println!("speech -> goal: {}", route_line(&w, speech, goal, radius));
}

/// The door at `pos` (its load position, which identifies it across worlds).
fn door_at(w: &World, pos: Vec3) -> Option<ObjectId> {
    w.iter().find(|o| o.kind == ObjectKind::Door && o.position == pos).map(|o| o.id)
}

/// Which single door, opened on top of the PeragusDoor1s, gives the GuardRoomDr
/// speech point a walk route to the exit. Each other door is opened alone in a
/// fresh world and the route is searched again. Run with --ignored --nocapture.
#[test]
#[ignore]
fn diagnose_which_door_unblocks_the_speech_route() {
    let Some(w0) = fresh("101per") else { return };
    let others: Vec<(String, String, Vec3)> = w0
        .iter()
        .filter(|o| o.kind == ObjectKind::Door && o.tag != "PeragusDoor1")
        .map(|o| (o.tag.clone(), o.template.clone(), o.position))
        .collect();

    let mut unblocking = Vec::new();
    for (tag, template, pos) in &others {
        let Some(mut w) = fresh("101per") else { return };
        let pc = w.party_leader();
        let radius = w.get(pc).and_then(|o| o.creature()).expect("the leader is a creature").radius;
        let speech = w.by_tag("WP_player_speech", 0).expect("101per has WP_player_speech");
        let from = w.get(speech).expect("the waypoint is in the world").position;
        let exit = w.by_tag("To_102PER", 0).expect("the exit trigger");
        let poly = w.get(exit).and_then(|o| o.trigger()).expect("trigger").polygon.clone();
        let goal = walkable_point_in(&w, &poly).expect("a walkable point in the exit");

        use_peragus_doors(&mut w, pc);
        let before = w.walk.find_path(from, goal, radius).is_some();
        let d = door_at(&w, *pos).expect("the door is in the world");
        let was_open = door_open(&w, d);
        w.set_door_open(d, true, pc);
        for _ in 0..120 {
            w.tick(1.0 / 30.0);
        }
        let after = w.walk.find_path(from, goal, radius).is_some();
        println!("{tag} ({template}) at ({:.2}, {:.2}): open at load {was_open}, route before {before}, route with it open {after}", pos.x, pos.y);
        if after && !before {
            unblocking.push(tag.clone());
        }
    }
    println!("doors that alone give the speech point a route to the exit: {unblocking:?}");
}

/// The story way through MedBayDoor, which the wake-up spot needs open: its dialog
/// (strref 74335) says to equip a plasma torch and bash the door with it, and its
/// hardness stops any other weapon. The torch is in LowCorpse by the medbay: the
/// leader loots it there, equips it, and bashes the door until it opens.
fn cut_medbay_door_with_torch(w: &mut World, pc: ObjectId) {
    // The medical log's dialog opens the morgue (a_doormor), as open_hatch_by_script
    // runs the hatch's opener.
    let morgue = w.by_tag("MorgueDoor", 0).expect("101per has MorgueDoor");
    assert!(w.run_script("a_doormor", pc, ScriptCtx::default()).is_some(), "a_doormor runs");
    for _ in 0..120 {
        w.tick(1.0 / 30.0);
    }
    assert!(door_open(w, morgue), "a_doormor opens MorgueDoor");
    // The torch's corpse (the one with its template; other LowCorpse objects hold other loot)
    // lies in the morgue, behind MorgueDoor from the medbay.
    let corpse = w.iter().find(|o| o.tag == "LowCorpse" && o.template == "g_tresgencorp004").map(|o| o.id).expect("101per has the torch corpse g_tresgencorp004");
    let at = w.get(corpse).expect("the corpse is in the world").position;
    let (dp, bearing) = w.get(morgue).map(|o| (o.position, o.bearing)).expect("MorgueDoor is in the world");
    let n = Vec2::new(-bearing.sin(), bearing.cos());
    // Outside is the side of the door away from the corpse; inside is the corpse's side.
    let s = if n.dot(at.truncate() - dp.truncate()) > 0.0 { -1.2 } else { 1.2 };
    let outside = ground_point(w, dp.truncate() + n * s, dp.z).expect("ground outside MorgueDoor");
    let inside = ground_point(w, dp.truncate() - n * s, dp.z).expect("ground inside MorgueDoor");
    walk_leg(w, pc, outside, "to the morgue door");
    walk_leg(w, pc, inside, "through the morgue door");
    // A point 1.5 m from the corpse that the leader can walk to, then the player's way:
    // use the corpse (its loot window opens), then take everything.
    let me = w.get(pc).expect("the leader is in the world").position;
    let spot = ring(at.truncate(), 1.5)
        .into_iter()
        .find_map(|c| {
            let p = ground_point(w, c, at.z)?;
            w.walk.find_path(me, p, 0.3).map(|_| p)
        })
        .expect("ground beside the corpse that the leader can walk to");
    walk_leg(w, pc, spot, "to the corpse");
    w.use_object(pc, corpse);
    let loot: Vec<String> = w.inventory_of(corpse).into_iter().filter_map(|i| w.get(i)).map(|o| o.template.clone()).collect();
    let taken = w.take_all(corpse, pc);
    let party: Vec<String> = w.inventory_of(pc).into_iter().filter_map(|i| w.get(i)).map(|o| o.template.clone()).collect();
    let torch = taken.iter().copied().find(|&i| w.get(i).is_some_and(|o| o.template == "plasmatorch"));
    let Some(torch) = torch else {
        panic!("no plasma torch reached the leader: the corpse held {loot:?}, took {taken:?}, the party holds {party:?} after the take");
    };
    w.equip(pc, torch, k2_world::stats::SLOT_RIGHTWEAPON).expect("the torch equips");
    assert!(w.equipment(pc).contains(&(k2_world::stats::SLOT_RIGHTWEAPON, torch)), "the torch is not in the right-weapon slot: {:?}", w.equipment(pc));
    // The game's way: the attack on the door walks the leader to its own standing point
    // (bash.rs: a stand on a side of the door the leader can path to, or the attack is
    // dropped). The leader stays where the corpse leg left it; no stand is picked here.
    let door = w.by_tag("MedBayDoor", 0).expect("101per has MedBayDoor");
    assert!(!door_open(w, door), "MedBayDoor is open before the bash");
    let hp_before = w.object_hp(door).expect("MedBayDoor has hit points");
    w.player_attack(door);
    let mut t = 0.0;
    let mut ticks = 0;
    let mut samples = Vec::new();
    let mut barks = Vec::new();
    while t < 120.0 && !door_open(w, door) {
        w.tick(1.0 / 30.0);
        t += 1.0 / 30.0;
        ticks += 1;
        barks.extend(w.drain_events().into_iter().filter_map(|e| match e {
            WorldEvent::Bark { text, .. } => Some(text),
            _ => None,
        }));
        if ticks % 30 == 0 {
            let at = w.get(pc).map(|o| o.position);
            let front = w.get(pc).and_then(|o| o.actions.front()).map(|a| format!("{a:?}"));
            let door_at = w.get(door).map(|o| o.position);
            let dist = at.zip(door_at).map(|(a, d)| (a - d).truncate().length());
            let _ = writeln!(
                std::io::stderr(),
                "  bash t {t:.0} s: leader {at:?}, {dist:.1?} m from the door, front action {front:?}, door hp {:?}",
                w.object_hp(door)
            );
            samples.push(format!("t {t:.0} s: leader {at:?}, front action {front:?}, door hp {:?}", w.object_hp(door)));
        }
    }
    let _ = writeln!(std::io::stderr(), "bash loop: {ticks} ticks, {t:.1} s of game time, door hp {hp_before:?} -> {:?}", w.object_hp(door));
    assert!(ticks > 0, "the bash loop never ticked");
    assert!(
        door_open(w, door),
        "the plasma torch did not cut MedBayDoor open in {t:.0} s; samples {samples:?}; barks {barks:?}; combat {:?}",
        w.combat.log
    );
    let hp_after = w.object_hp(door).expect("MedBayDoor has hit points");
    assert!(hp_after.destroyed || hp_after.current < hp_before.current, "the bash did not hurt MedBayDoor: {hp_before:?} -> {hp_after:?}");
    // The walk goes through a door once its swing is done (tick_doors), not the moment
    // it reports open: wait the swing out, as the game does, before routing through it.
    let mut swing = 0.0;
    while swing < 10.0 && w.get(door).and_then(|o| o.door()).is_some_and(|d| d.opening > 0.0) {
        w.tick(1.0 / 30.0);
        swing += 1.0 / 30.0;
    }
    let _ = writeln!(std::io::stderr(), "bash swing: {swing:.1} s after MedBayDoor reported open");
}

/// Walks the leader to `to` the way a click walk does: a MoveTo action, ticked until
/// the action ends. The leg's route is checked first, so a failure names the leg.
fn walk_leg(w: &mut World, pc: ObjectId, to: Vec3, leg: &str) {
    let from = w.get(pc).expect("the leader is in the world").position;
    let radius = w.get(pc).and_then(|o| o.creature()).expect("the leader is a creature").radius;
    // find_path ends at the nearest reachable spot within 3 m when `to` is out of reach,
    // so the check is that the route really ends at `to`.
    let route = w.walk.find_path(from, to, radius);
    let end = route.as_ref().map(|p| p.last().copied().unwrap_or(from));
    assert!(
        end.is_some_and(|e| e.truncate().distance(to.truncate()) <= 0.6),
        "leg {leg}: no walk route ends at {to:?} from {from:?} (route ends at {end:?}): {}",
        route_line(w, from, to, radius)
    );
    let route = route.unwrap_or_default();
    let route_m: f32 = std::iter::once(from).chain(route.iter().copied()).collect::<Vec<_>>().windows(2).map(|s| (s[1] - s[0]).truncate().length()).sum();
    let _ = writeln!(std::io::stderr(), "walk leg {leg}: route of {} points, {route_m:.1} m, from {from:?} to {to:?}", route.len());
    w.get_mut(pc).expect("the leader is in the world").actions.clear();
    // No timeout: a timed MoveTo places the mover at its target when the time runs out
    // (tick.rs), so a leg could "arrive" by teleport. Untimed, only a walk reaches it.
    w.push_action(pc, Action::MoveTo { target: Target::Point(to), run: false, range: 0.5, timeout: None, elapsed: 0.0, stuck: 0.0 });
    let mut t = 0.0;
    let mut ticks = 0;
    while t < 120.0 && w.get(pc).is_some_and(|o| !o.actions.is_empty()) {
        w.tick(1.0 / 30.0);
        t += 1.0 / 30.0;
        ticks += 1;
        if ticks % 30 == 0 {
            let at = w.get(pc).expect("the leader is in the world").position;
            let near: Vec<String> = w
                .iter()
                .filter(|o| o.kind == ObjectKind::Creature && o.id != pc && o.creature().is_some_and(|c| !c.dead))
                .filter(|o| (o.position - at).truncate().length() < 3.0)
                .map(|o| format!("{} {:.1} m", o.tag, (o.position - at).truncate().length()))
                .collect();
            let front = w.get(pc).and_then(|o| o.actions.front()).map(|a| format!("{a:?}")).unwrap_or_default();
            let speed = w.get(pc).and_then(|o| o.creature()).map_or(0.0, |c| c.walk_speed);
            let _ = writeln!(
                std::io::stderr(),
                "  leg {leg} t {t:.0} s: leader {at:?}, {:.1} m to go, walk speed {speed:.2} x {:.2}, near {near:?}, action {front}",
                (at - to).truncate().length(),
                w.speed_factor(pc)
            );
        }
    }
    let at = w.get(pc).expect("the leader is in the world").position;
    let _ = writeln!(std::io::stderr(), "walk leg {leg}: {:.1} s of game time, stopped at {at:?}", t);
    assert!(
        at.truncate().distance(to.truncate()) <= 0.6,
        "leg {leg}: the leader stopped at {at:?}, not {to:?}, after {t:.0} s; front action {:?}",
        w.get(pc).and_then(|o| o.actions.front()).map(|a| format!("{a:?}"))
    );
}

/// The walkable point at `p`, on the floor near height `z`.
fn ground_point(w: &World, p: Vec2, z: f32) -> Option<Vec3> {
    w.walk.ground(p.x, p.y, z + 1.0).map(|h| Vec3::new(p.x, p.y, h.z))
}

/// From the wake-up spot (where the intro leaves the PC): which single door, opened
/// on top of the hatch and the PeragusDoor1s, gives the leader a walk route to the
/// exit; failing that, which pair of the locked doors. Each trial is a fresh settled
/// world. Run with --ignored --nocapture; it asserts nothing.
#[test]
#[ignore]
fn diagnose_which_door_unblocks_the_wakeup_route() {
    let Some(w0) = settled("101per") else { return };
    let others: Vec<(String, String, Vec3, bool)> = w0
        .iter()
        .filter(|o| o.kind == ObjectKind::Door && o.tag != "PeragusDoor1" && o.tag != "102PERDoor")
        .map(|o| (o.tag.clone(), o.template.clone(), o.position, o.door().is_some_and(|d| d.locked)))
        .collect();
    let (base, line) = wakeup_route_with(&[]);
    println!("baseline (hatch and PeragusDoor1s open): route {base}; {line}");

    let mut unblocking = Vec::new();
    for (tag, template, pos, locked) in &others {
        let (route, _) = wakeup_route_with(&[*pos]);
        println!("{tag} ({template}) at ({:.2}, {:.2}), locked at load {locked}: route {route}", pos.x, pos.y);
        if route {
            unblocking.push(tag.clone());
        }
    }
    println!("single doors giving the wake-up spot a route to the exit: {unblocking:?}");
    if unblocking.is_empty() {
        let locked: Vec<(String, Vec3)> = others.iter().filter(|o| o.3).map(|o| (o.0.clone(), o.2)).collect();
        for i in 0..locked.len() {
            for j in i + 1..locked.len() {
                let (route, _) = wakeup_route_with(&[locked[i].1, locked[j].1]);
                if route {
                    println!("pair {} and {}: route {route}", locked[i].0, locked[j].0);
                }
            }
        }
        println!("pairs of the {} locked doors were tried", locked.len());
    }
}

/// Whether the leader at the wake-up spot has a walk route to the exit with the
/// hatch and the PeragusDoor1s open and the doors at `doors` opened too, and what
/// the checked search says. A fresh settled world per call.
fn wakeup_route_with(doors: &[Vec3]) -> (bool, String) {
    let Some(mut w) = settled("101per") else { panic!("a wake-up trial needs the install and 101per") };
    let pc = w.party_leader();
    open_hatch_by_script(&mut w);
    use_peragus_doors(&mut w, pc);
    for &p in doors {
        let d = door_at(&w, p).expect("the door is in the world");
        w.set_door_open(d, true, pc);
    }
    for _ in 0..120 {
        w.tick(1.0 / 30.0);
    }
    let from = w.get(pc).expect("the leader is in the world").position;
    let radius = w.get(pc).and_then(|o| o.creature()).expect("the leader is a creature").radius;
    let (_, _, goal) = exit_points(&w);
    (w.walk.find_path(from, goal, radius).is_some(), route_line(&w, from, goal, radius))
}

/// Runs the hatch's opener the way Atton's dialog does (node E41 of 101atton runs
/// a_door102per with Atton as the caller), then ticks until the hatch has opened.
fn open_hatch_by_script(w: &mut World) {
    let atton = w.by_tag("Atton", 0).unwrap_or(w.party_leader());
    assert!(w.run_script("a_door102per", atton, ScriptCtx::default()).is_some(), "a_door102per runs");
    for _ in 0..120 {
        w.tick(1.0 / 30.0);
    }
    let hatch = w.by_tag("102PERDoor", 0).expect("101per has 102PERDoor");
    assert!(door_open(w, hatch), "a_door102per opens the hatch");
}

/// Walks the leader into the trigger whose OnEnter is `script`: the leader is
/// placed on a walkable point inside the trigger, then the world ticks, so the
/// trigger's OnEnter runs the way a walk into it would.
fn enter_trigger_with(w: &mut World, script: &str) {
    let trigger = w
        .iter()
        .find(|o| o.kind == ObjectKind::Trigger && o.script("ScriptOnEnter") == Some(script))
        .map(|o| o.id)
        .unwrap_or_else(|| panic!("101per has a trigger whose OnEnter is {script}"));
    let poly = w.get(trigger).and_then(|o| o.trigger()).expect("a trigger has a state").polygon.clone();
    let spot = walkable_point_in(w, &poly).expect("the trigger has a walkable point inside it");
    let pc = w.party_leader();
    w.place(pc, spot);
    // One tick first: the trigger must register the leader before its OnEnter can run.
    w.tick(1.0 / 30.0);
    let registered = w.get(trigger).and_then(|o| o.trigger()).is_some_and(|t| t.inside.contains(&pc));
    assert!(registered, "the trigger did not register the leader placed at {spot:?} (leader now at {:?})", w.get(pc).map(|o| o.position));
    for _ in 0..119 {
        w.tick(1.0 / 30.0);
    }
}

/// The OnEnter of a_door105per (and a_door103per, the same shape) closes and locks
/// 102PERDoor, then unlocks and opens the trigger's own hatch (Hatch_105PERDoor or
/// Hatch_103PERDoor). The trace of the 105 trigger confirms the calls.
fn check_side_trigger(script: &str, side_hatch: &str) {
    let Some(mut w) = settled("101per") else { return };
    // The emergency hatch is open first (a_door102per, Atton's scene), so the
    // trigger's close and lock have something to act on.
    open_hatch_by_script(&mut w);
    let emergency = w.by_tag("102PERDoor", 0).expect("101per has 102PERDoor");
    let hatch = w.by_tag(side_hatch, 0).expect("101per has the side hatch");
    assert!(!door_open(&w, hatch), "{side_hatch} starts shut");
    enter_trigger_with(&mut w, script);
    assert!(door_open(&w, hatch), "the trigger's OnEnter ({script}) opens {side_hatch}");
    assert!(!door_open(&w, emergency), "{script} closes 102PERDoor");
    assert!(door_locked(&w, emergency), "{script} locks 102PERDoor");
}

#[test]
fn walking_into_the_105per_trigger_opens_its_hatch_and_seals_the_emergency_hatch() {
    check_side_trigger("a_door105per", "Hatch_105PERDoor");
}

#[test]
fn walking_into_the_103per_trigger_opens_its_hatch_and_seals_the_emergency_hatch() {
    check_side_trigger("a_door103per", "Hatch_103PERDoor");
}

/// The body with kre_body in 001ebo: a placeable with an inventory and a talk.
fn kre_body(w: &World) -> ObjectId {
    w.iter()
        .find(|o| o.kind == ObjectKind::Placeable && o.conversation.as_deref() == Some("kre_body"))
        .map(|o| o.id)
        .expect("001ebo has the body with kre_body")
}

#[test]
fn destroying_the_open_container_clears_it_so_saving_is_not_blocked() {
    let Some(mut w) = settled("001ebo") else { return };
    let pc = w.party_leader();
    let body = kre_body(&w);
    assert!(w.open_container(pc, body).is_ok(), "the body opens as a container");
    assert_eq!(w.save_blocked(), Some("a menu is open"));

    w.destroy(body);
    assert_eq!(w.inventory.open_container, None, "a destroyed container is not the open one");
    assert_ne!(w.save_blocked(), Some("a menu is open"), "the destroyed container does not block saving");
}

#[test]
fn a_container_held_under_a_talk_blocks_saving_as_the_talk_does() {
    let Some(mut w) = settled("001ebo") else { return };
    let pc = w.party_leader();
    let body = kre_body(&w);
    w.use_object(pc, body);
    assert!(w.conversation.is_some(), "kre_body talks before the container opens");
    assert_eq!(w.inventory.open_container, Some(body), "the container is open under the talk");
    assert_eq!(w.save_blocked(), Some("in conversation"));
    assert_eq!(w.exit_save_blocked(), Some("in conversation"), "no window is shown, so the reason is the talk");
}
