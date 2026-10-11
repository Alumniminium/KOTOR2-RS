//! Doors as placed by the GIT: closed blocks the doorway, open passes it
//! straight through the center. Skips when no install is found.

use glam::{Vec2, Vec3};
use k2_res::Install;
use k2_world::World;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

/// Runs the world until every door finished opening (longest clip is 7 s).
fn settle(w: &mut World) {
    for _ in 0..160 {
        w.tick(0.05);
    }
}

fn opening(w: &World, door: u32) -> f32 {
    w.get(door).unwrap().door().unwrap().opening
}

/// Walks 3 m through the door center along its passage axis (door-local Y),
/// from whichever side has ground. Returns metres advanced past the start.
fn walk_through(w: &World, center: Vec3, bearing: f32) -> f32 {
    // Door width runs along local X; passage is local +-Y.
    let n = Vec2::new(-bearing.sin(), bearing.cos());
    for side in [1.0f32, -1.0] {
        let d = n * side;
        let s = center.truncate() - d * 1.5;
        let Some(g) = w.walk.ground(s.x, s.y, center.z + 1.0) else { continue };
        let start = Vec3::new(s.x, s.y, g.z);
        let mut p = start;
        for _ in 0..12 {
            p = w.walk.move_slide(p, d * 0.25, 0.3);
        }
        return (p.truncate() - start.truncate()).dot(d);
    }
    panic!("no ground on either side of the door at {center}");
}

fn check(module: &str, tag: &str) {
    let Some(mut w) = world() else { return };
    w.enter_module(module, None).unwrap();
    let door = w.by_tag(tag, 0).unwrap_or_else(|| panic!("{module}: no door {tag}"));
    let (pos, bearing) = {
        let o = w.get(door).unwrap();
        (o.position, o.bearing)
    };
    if w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open) {
        w.set_door_open(door, false, door);
    }
    let closed = walk_through(&w, pos, bearing);
    w.set_door_open(door, true, door);
    settle(&mut w);
    let open = walk_through(&w, pos, bearing);
    eprintln!("{module} {tag}: closed {closed:.2} m, open {open:.2} m");
    assert!(closed < 1.5, "{module} {tag}: walked {closed} m through the closed door");
    assert!(open > 2.9, "{module} {tag}: open door still blocks the center: {open} m");
}

#[test]
fn peragus_door_opens_through_center() {
    check("101per", "PeragusDoor1");
}

#[test]
fn ebon_hawk_door_opens_through_center() {
    check("001ebo", "001EBODr1");
}

/// Opens every door of every module and walks through its center. Slow:
/// run with `K2_DOOR_SURVEY=1 cargo test -p k2-world --test doors survey -- --nocapture`.
#[test]
fn survey_all_doors_open() {
    if std::env::var("K2_DOOR_SURVEY").is_err() {
        return;
    }
    let Some(mut w) = world() else { return };
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let mods = Install::discover(dir).unwrap().module_names();
    let mut bad = 0;
    for m in mods {
        if w.enter_module(&m, None).is_err() {
            continue;
        }
        let doors: Vec<_> = w.iter().filter(|o| o.door().is_some()).map(|o| (o.id, o.tag.clone(), o.position, o.bearing, o.door().unwrap().model.clone())).collect();
        for (id, ..) in &doors {
            w.set_door_open(*id, true, *id);
        }
        settle(&mut w);
        for (id, tag, pos, bearing, model) in doors {
            let Some(model) = model else { continue };
            let _ = id;
            let n = Vec2::new(-bearing.sin(), bearing.cos());
            let best = [1.0f32, -1.0]
                .into_iter()
                .filter_map(|side| {
                    let far = pos.truncate() + n * side * 1.0;
                    w.walk.ground(far.x, far.y, pos.z + 1.0)?;
                    let d = n * side;
                    let s = pos.truncate() - d * 1.0;
                    let g = w.walk.ground(s.x, s.y, pos.z + 1.0)?;
                    let start = Vec3::new(s.x, s.y, g.z);
                    let mut p = start;
                    for _ in 0..8 {
                        p = w.walk.move_slide(p, d * 0.25, 0.3);
                    }
                    Some((p.truncate() - start.truncate()).dot(d))
                })
                .fold(f32::MIN, f32::max);
            if best > f32::MIN && best < 1.9 {
                bad += 1;
                eprintln!("BLOCKED {m} {tag} model {model} at {pos:?} bearing {bearing:.2}: {best:.2} m");
            }
        }
    }
    eprintln!("blocked doors: {bad}");
}

#[test]
fn debug_door() {
    let Ok(spec) = std::env::var("K2_DOOR_DEBUG") else { return };
    let (m, xy) = spec.split_once(':').unwrap();
    let (tx, ty) = xy.split_once(',').unwrap();
    let (tx, ty): (f32, f32) = (tx.parse().unwrap(), ty.parse().unwrap());
    let Some(mut w) = world() else { return };
    w.enter_module(m, None).unwrap();
    let doors: Vec<_> = w.iter().filter(|o| o.door().is_some()).map(|o| (o.id, o.position, o.bearing)).collect();
    let (id, pos, bearing) = *doors.iter().min_by(|a, b| a.1.truncate().distance(Vec2::new(tx, ty)).total_cmp(&b.1.truncate().distance(Vec2::new(tx, ty)))).unwrap();
    eprintln!("door at {pos:?} bearing {bearing}");
    w.set_door_open(id, true, id);
    settle(&mut w);
    eprintln!("handles {:?}", w.door_handle(id));
    for o in w.iter() {
        if o.position.truncate().distance(pos.truncate()) < 14.0 {
            eprintln!("near: {:?} tag {} tmpl {} at {:?} bearing {:.2} door {:?}", o.kind, o.tag, o.template, o.position, o.bearing, o.door().map(|d| (d.open, d.model.clone())).or(o.placeable().map(|p| (p.open, p.model.clone()))));
        }
    }
    let n = Vec2::new(-bearing.sin(), bearing.cos());
    for side in [1.0f32, -1.0] {
        let d = n * side;
        let s = pos.truncate() - d * 1.5;
        eprintln!("side {side}: ground at start {:?}", w.walk.ground(s.x, s.y, pos.z + 1.0).map(|h| (h.z, h.material)));
        let Some(g) = w.walk.ground(s.x, s.y, pos.z + 1.0) else { continue };
        let mut p = Vec3::new(s.x, s.y, g.z);
        for i in 0..14 {
            let q = w.walk.move_slide(p, d * 0.25, 0.3);
            let rel = (q.truncate() - pos.truncate()).dot(d);
            let gm = w.walk.ground(q.x, q.y, q.z + 0.5).map(|h| h.material);
            if (q - p).length() < 0.01 {
                for (a, b, room, mesh) in w.walk.blockers_near(q, 0.6) {
                    eprintln!("    blocker mesh {mesh} room={room} ({:.2},{:.2},{:.2}) -> ({:.2},{:.2},{:.2})", a.x, a.y, a.z, b.x, b.y, b.z);
                }
            }
            eprintln!("  step {i}: local y {rel:.2} z {:.2} mat {gm:?} moved {:.3}", q.z, (q - p).length());
            p = q;
        }
    }
}

/// 105per SecurityDoor: its OnOpen script destroys the invisible DoorInvBlk
/// placeable behind it. The destroyed placeable's walkmesh must go too.
#[test]
fn destroyed_placeable_frees_its_collider() {
    let Some(mut w) = world() else { return };
    w.enter_module("105per", None).unwrap();
    let door = w
        .iter()
        .filter(|o| o.door().is_some())
        .min_by(|a, b| a.position.truncate().distance(Vec2::new(-30.6, -11.18)).total_cmp(&b.position.truncate().distance(Vec2::new(-30.6, -11.18))))
        .map(|o| o.id)
        .expect("door");
    let blk = w.by_tag("doorinvblk", 0).expect("blocker");
    let (pos, bearing) = {
        let o = w.get(door).unwrap();
        (o.position, o.bearing)
    };
    w.set_door_open(door, true, door);
    settle(&mut w);
    let destroyed = w.get(blk).is_none_or(|o| o.destroyed);
    let walked = walk_through(&w, pos, bearing);
    eprintln!("blocker destroyed {destroyed}, walked {walked:.2} m");
    assert!(destroyed, "OnOpen no longer destroys the blocker");
    assert!(walked > 2.9, "destroyed blocker still collides: {walked} m");
}

fn peragus_door(w: &mut World) -> (u32, Vec3, f32) {
    w.enter_module("101per", None).unwrap();
    let door = w.by_tag("PeragusDoor1", 0).expect("door");
    let (pos, bearing) = {
        let o = w.get(door).unwrap();
        (o.position, o.bearing)
    };
    if w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open) {
        w.set_door_open(door, false, door);
    }
    (door, pos, bearing)
}

/// The walkmesh opens only when the opening clip has played, and blocks
/// again the moment closing starts or the door is closed mid opening.
#[test]
fn door_passable_only_after_opening_animation() {
    let Some(mut w) = world() else { return };
    let (door, pos, bearing) = peragus_door(&mut w);
    w.set_door_open(door, true, door);
    let secs = opening(&w, door);
    assert!(secs > 0.5, "no opening clip length: {secs}");
    assert!(w.get(door).unwrap().door().unwrap().open);
    assert!(walk_through(&w, pos, bearing) < 1.5, "passable at the start of opening");
    let mut t = 0.0;
    while t < secs * 0.5 {
        w.tick(0.05);
        t += 0.05;
    }
    assert!(walk_through(&w, pos, bearing) < 1.5, "passable half way through opening");
    while t < secs + 0.1 {
        w.tick(0.05);
        t += 0.05;
    }
    assert_eq!(opening(&w, door), 0.0);
    assert!(walk_through(&w, pos, bearing) > 2.9, "still blocked after the animation");
    // Closing blocks at once.
    w.set_door_open(door, false, door);
    assert!(walk_through(&w, pos, bearing) < 1.5, "passable while closing");
    // Closed while still opening: stays blocked, the timer is dropped.
    w.set_door_open(door, true, door);
    w.tick(0.2);
    w.set_door_open(door, false, door);
    assert_eq!(opening(&w, door), 0.0);
    settle(&mut w);
    assert!(walk_through(&w, pos, bearing) < 1.5, "opened by a cancelled transition");
}

/// Opening from the door's +Y side picks the second clip / DWK set; the
/// door still ends passable.
#[test]
fn door_side_follows_the_opener() {
    let Some(mut w) = world() else { return };
    let (door, pos, bearing) = peragus_door(&mut w);
    let pc = w.party_leader();
    let n = Vec3::new(-bearing.sin(), bearing.cos(), 0.0);
    w.get_mut(pc).unwrap().position = pos + n * 2.0;
    w.set_door_open(door, true, pc);
    assert_eq!(w.get(door).unwrap().door().unwrap().side, 2);
    settle(&mut w);
    assert!(walk_through(&w, pos, bearing) > 2.9);
    w.set_door_open(door, false, pc);
    w.get_mut(pc).unwrap().position = pos - n * 2.0;
    w.set_door_open(door, true, pc);
    assert_eq!(w.get(door).unwrap().door().unwrap().side, 1);
}

/// A save taken mid transition loads settled: an opening door is open and
/// passable, a closing one closed, and old saves (no new fields) still load.
#[test]
fn door_saved_mid_transition_loads_consistent() {
    let Some(mut w) = world() else { return };
    let (door, pos, bearing) = peragus_door(&mut w);
    w.set_door_open(door, true, door);
    w.tick(0.1);
    assert!(opening(&w, door) > 0.0);
    while w.save_blocked().is_some() {
        w.tick(0.05);
        assert!(opening(&w, door) > 0.0, "transition ended before the save was allowed");
    }
    let data = k2_world::save::SaveData::from_bytes(&w.save().to_bytes()).unwrap();
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let loaded = World::load(Install::discover(dir).unwrap(), &data).unwrap();
    let d = loaded.get(door).unwrap().door().unwrap();
    assert!(d.open);
    assert_eq!(d.opening, 0.0);
    assert!(walk_through(&loaded, pos, bearing) > 2.9, "mid-opening save loads blocked");
}

/// Module entry leaves no door in transition (initial open states and
/// entry scripts are instant).
#[test]
fn module_entry_doors_are_settled() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    assert!(w.iter().filter_map(|o| o.door()).all(|d| d.opening == 0.0));
}

/// Line of sight, perception and attacks go through the walkmesh enable
/// flags, so they wait for the opening animation like movement does.
#[test]
fn enemies_see_and_shoot_only_after_the_door_opened() {
    let Some(mut w) = world() else { return };
    let (door, pos, bearing) = peragus_door(&mut w);
    let leader = w.party_leader();
    let n = Vec3::new(-bearing.sin(), bearing.cos(), 0.0);
    // Player and hostile on opposite sides of the door, both on ground.
    let mut placed = None;
    for sign in [1.0f32, -1.0] {
        let (pp, hp) = (pos + n * 3.0 * sign, pos - n * 3.0 * sign);
        if let (Some(a), Some(b)) = (w.walk.ground(pp.x, pp.y, pos.z + 1.0), w.walk.ground(hp.x, hp.y, pos.z + 1.0)) {
            placed = Some((Vec3::new(pp.x, pp.y, a.z), Vec3::new(hp.x, hp.y, b.z)));
            break;
        }
    }
    let (pp, hp) = placed.expect("ground on both sides of the door");
    w.set_player_position(pp, 0.0);
    for p in w.party.clone() {
        if p != leader {
            w.get_mut(p).unwrap().position = pp;
        }
    }
    let foe = w.spawn_creature("g_sithtroop01", hp, bearing).expect("spawn hostile");
    assert!(w.is_enemy(foe, leader));
    let dt = 1.0 / 30.0;
    // The module's intro scripts move the leader; hold both in place until
    // the door is open (afterwards only the leader is held).
    let hold_foe = std::cell::Cell::new(true);
    let tick = |w: &mut World| {
        w.set_player_position(pp, 0.0);
        w.get_mut(leader).unwrap().actions.clear();
        if hold_foe.get() {
            let f = w.get_mut(foe).unwrap();
            f.position = hp;
            f.actions.clear();
        }
        w.tick(dt);
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
        w.drain_events().iter().any(|e| matches!(e, k2_world::WorldEvent::Damaged { by, .. } if *by == foe))
    };
    // Shut door: neither side sees the other, no shots.
    for _ in 0..150 {
        assert!(!tick(&mut w));
        assert!(!w.can_see(foe, leader) && !w.can_see(leader, foe), "sees through the closed door");
    }
    w.set_door_open(door, true, leader);
    let secs = opening(&w, door);
    assert!(secs > 0.5);
    let mut t = 0.0;
    while t < secs - 0.1 {
        assert!(!tick(&mut w), "shot through the door at {t:.2} s of {secs:.2} s");
        assert!(!w.can_see(foe, leader), "saw through the opening door at {t:.2} s");
        t += dt;
    }
    // Open: sight is back at once, and the hostile attacks within seconds.
    hold_foe.set(false);
    let mut seen_at = None;
    let mut shot_at = None;
    while t < secs + 12.0 && shot_at.is_none() {
        if tick(&mut w) {
            shot_at = Some(t);
        }
        if seen_at.is_none() && w.can_see(foe, leader) {
            seen_at = Some(t);
        }
        t += dt;
    }
    let seen_at = seen_at.expect("no line of sight through the open door");
    assert!(seen_at >= secs - 0.2, "saw at {seen_at:.2} before open at {secs:.2}");
    eprintln!("door open at {secs:.2} s, seen at {seen_at:.2} s, first shot at {shot_at:?}");
    assert!(shot_at.is_some_and(|s| s >= secs - 0.2), "hostile never attacked through the open door");
    // Closing blocks sight at once (the hostile has walked up by now).
    w.get_mut(foe).unwrap().position = hp;
    assert!(w.can_see(foe, leader));
    w.set_door_open(door, false, leader);
    assert!(!w.can_see(foe, leader));
}
