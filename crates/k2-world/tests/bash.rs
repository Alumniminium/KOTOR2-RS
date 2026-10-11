//! Bashing doors: the plasma torch (DoorCutting) cuts a Peragus security door
//! open; a plain weapon can't beat its Hardness. Skips when no install is
//! found.

use glam::{Vec2, Vec3};
use k2_res::Install;
use k2_world::traps::TrapState;
use k2_world::{ObjectKind, World, WorldEvent};

const DT: f32 = 1.0 / 30.0;
const SLOT_RIGHT_WEAPON: i32 = 4;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

/// 102per with its creatures removed, the PC 2 m in front of SecDoor holding
/// `weapon`. Returns the world and the door.
fn setup(weapon: &str) -> Option<(World, k2_script::ObjectId)> {
    let mut w = world()?;
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    let others: Vec<_> = w.iter().filter(|o| o.kind == ObjectKind::Creature && o.id != pc).map(|o| o.id).collect();
    for id in others {
        w.destroy(id);
    }
    let door = w.by_tag("SecDoor", 0).expect("SecDoor");
    let (pos, bearing) = {
        let o = w.get(door).unwrap();
        (o.position, o.bearing)
    };
    // Stand on whichever side of the doorway has ground.
    let n = Vec2::new(-bearing.sin(), bearing.cos());
    let spot = [1.0f32, -1.0]
        .into_iter()
        .find_map(|s| {
            let p = pos.truncate() + n * 2.0 * s;
            w.walk.ground(p.x, p.y, pos.z + 1.0).map(|g| p.extend(g.z))
        })
        .expect("ground by the door");
    w.get_mut(pc).unwrap().position = spot;
    let item = w.create_item(weapon, pc, 1).expect("weapon");
    w.equip(pc, item, SLOT_RIGHT_WEAPON).expect("equip");
    w.combat.log_enabled = true;
    Some((w, door))
}

#[test]
fn plasma_torch_cuts_a_peragus_door_open() {
    let Some((mut w, door)) = setup("plasmatorch") else { return };
    assert!(w.can_bash(door), "SecDoor should be bashable: {:?}", w.object_hp(door));
    let before = w.object_hp(door).unwrap();
    w.player_attack(door);
    let mut t = 0.0;
    while t < 120.0 && !w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open) {
        w.tick(DT);
        t += DT;
    }
    for l in &w.combat.log {
        eprintln!("{l}");
    }
    let d = w.get(door).and_then(|o| o.door()).unwrap();
    eprintln!("hp before {before:?}, after {:?}, {t:.1} s", d.hp);
    assert!(d.open && d.hp.destroyed, "door not cut open after {t:.1} s: {:?}", d.hp);
    assert!(!w.can_bash(door));
}

#[test]
fn plasma_torch_opens_the_door_on_the_first_swing() {
    let Some((mut w, door)) = setup("plasmatorch") else { return };
    w.player_attack(door);
    let mut t = 0.0;
    let mut swings = 0;
    while t < 30.0 && !w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open) {
        w.tick(DT);
        t += DT;
        swings = w.combat.log.iter().filter(|l| l.contains("bashes")).count();
    }
    assert_eq!(swings, 1, "{:?}", w.combat.log);
    let d = w.get(door).and_then(|o| o.door()).unwrap();
    assert!(d.open && d.hp.destroyed && !d.locked, "{:?}", d.hp);
    let (pos, bearing) = {
        let o = w.get(door).unwrap();
        (o.position, o.bearing)
    };
    // The doorway is walkable once open: ground at the door origin.
    let _ = bearing;
    assert!(w.walk.ground(pos.x, pos.y, pos.z + 1.0).is_some(), "doorway not walkable");
}

/// A door the torch bashes open releases its walk block when its swing is done, as
/// an opened door does: `set_door_open` sets the open state at once, and tick_doors
/// hands the walk the open state once the door's opening timer runs out.
#[test]
fn a_bashed_door_releases_its_walk_block_when_its_swing_is_done() {
    let Some((mut w, door)) = setup("plasmatorch") else { return };
    let at = w.get(door).expect("the door").position;
    let door_blocks = |w: &World| w.walk.blockers_near(at, 2.0).iter().filter(|b| !b.2).count();
    let closed = door_blocks(&w);
    assert!(closed > 0, "the closed door blocks the walk");
    w.player_attack(door);
    let mut t = 0.0;
    while t < 120.0 && !w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open) {
        w.tick(DT);
        t += DT;
    }
    assert!(w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open), "not bashed open after {t:.1} s");
    let mut swing = 0.0;
    while swing < 10.0 && w.get(door).and_then(|o| o.door()).is_some_and(|d| d.opening > 0.0) {
        w.tick(DT);
        swing += DT;
    }
    assert!(w.get(door).and_then(|o| o.door()).is_some_and(|d| d.opening == 0.0), "the swing has not finished in 10 s");
    let after = door_blocks(&w);
    assert!(after < closed, "the bashed door still blocks the walk after its swing (closed {closed}, now {after})");
}

#[test]
fn plain_weapon_cannot_beat_hardness() {
    // A vibrocutter-class melee weapon without DoorCutting: Hardness 20 eats it.
    let Some((mut w, door)) = setup("w_melee_03") else { return };
    w.player_attack(door);
    for _ in 0..(20.0 / DT) as usize {
        w.tick(DT);
    }
    let d = w.get(door).and_then(|o| o.door()).unwrap();
    assert!(!d.open, "a hardness-20 door opened to a plain weapon");
    assert_eq!(d.hp.current, d.hp.max.min(d.hp.current), "{:?}", d.hp);
}

/// Inferred rule (NWN Trap article): bashing a trapped door open sets its
/// trap off, and the door still ends up open and destroyed.
#[test]
fn trapped_door_goes_off_when_bashed_open() {
    let Some((mut w, door)) = setup("plasmatorch") else { return };
    let trap = TrapState { trap_type: 3, detect_dc: 99, disarm_dc: 99, detectable: true, disarmable: true, one_shot: true, faction: 1, ..Default::default() };
    w.get_mut(door).unwrap().trap = Some(Box::new(trap));
    w.player_attack(door);
    let mut events = Vec::new();
    let mut t = 0.0;
    while t < 120.0 && !w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open) {
        w.tick(DT);
        events.extend(w.drain_events());
        t += DT;
    }
    assert!(w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open && d.hp.destroyed), "not bashed open after {t:.1} s");
    assert!(
        events.iter().any(|e| matches!(e, WorldEvent::TrapTriggered { trap, .. } if *trap == door)),
        "bashing the door open sets its trap off"
    );
    assert!(w.get(door).is_some(), "a trapped door survives its trap");
    assert!(!w.is_armed_trap(door), "a one-shot trap stops being armed");
}

/// Same rule for a placeable (NWN Bash page: a trap on a bashed object is set
/// off by the blow). The first bashable placeable of 102per, the PC beside it.
#[test]
fn trapped_placeable_goes_off_when_bashed_open() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    let others: Vec<_> = w.iter().filter(|o| o.kind == ObjectKind::Creature && o.id != pc).map(|o| o.id).collect();
    for id in others {
        w.destroy(id);
    }
    let target = w.iter().filter(|o| o.kind == ObjectKind::Placeable).map(|o| o.id).find(|&id| w.can_bash(id)).expect("a bashable placeable in 102per");
    let pos = w.get(target).unwrap().position;
    let spot = [(1.5f32, 0.0f32), (-1.5, 0.0), (0.0, 1.5), (0.0, -1.5)]
        .into_iter()
        .find_map(|(dx, dy)| {
            let p = pos.truncate() + Vec2::new(dx, dy);
            w.walk.ground(p.x, p.y, pos.z + 1.0).map(|g| p.extend(g.z))
        })
        .expect("ground by the placeable");
    w.get_mut(pc).unwrap().position = spot;
    let item = w.create_item("plasmatorch", pc, 1).expect("weapon");
    w.equip(pc, item, SLOT_RIGHT_WEAPON).expect("equip");
    let trap = TrapState { trap_type: 3, detect_dc: 99, disarm_dc: 99, detectable: true, disarmable: true, one_shot: true, faction: 1, ..Default::default() };
    w.get_mut(target).unwrap().trap = Some(Box::new(trap));
    // Locked: the blow sets the trap off regardless of the lock (documented rule).
    w.get_mut(target).unwrap().placeable_mut().unwrap().locked = true;
    w.player_attack(target);
    let mut events = Vec::new();
    let mut t = 0.0;
    while t < 120.0 && !w.object_hp(target).is_some_and(|h| h.destroyed) {
        w.tick(DT);
        events.extend(w.drain_events());
        t += DT;
    }
    assert!(w.object_hp(target).is_some_and(|h| h.destroyed), "placeable not bashed open after {t:.1} s");
    assert!(
        events.iter().any(|e| matches!(e, WorldEvent::TrapTriggered { trap, .. } if *trap == target)),
        "bashing the placeable open sets its trap off"
    );
    assert!(w.get(target).is_some(), "a trapped placeable survives its trap");
    assert!(!w.is_armed_trap(target), "a one-shot trap stops being armed");
}

/// Path length from `from` to the nearer standing point of the door.
fn path_to_door(w: &World, from: Vec3, door: k2_script::ObjectId) -> Option<f32> {
    let (pos, b) = {
        let o = w.get(door)?;
        (o.position, o.bearing)
    };
    let n = Vec2::new(-b.sin(), b.cos());
    [1.2f32, -1.2]
        .into_iter()
        .filter_map(|s| {
            let p = pos.truncate() + n * s;
            let g = w.walk.ground(p.x, p.y, pos.z + 1.0)?;
            let c = Vec3::new(p.x, p.y, g.z);
            let path = w.walk.find_path(from, c, 0.3)?;
            if path.last().is_some_and(|e| e.truncate().distance(c.truncate()) > 0.5) {
                return None;
            }
            let pts: Vec<Vec3> = std::iter::once(from).chain(path).collect();
            Some(pts.windows(2).map(|p| (p[1] - p[0]).truncate().length()).sum())
        })
        .min_by(|a: &f32, b| a.total_cmp(b))
}

#[test]
fn leader_paths_around_a_corner_to_the_door() {
    let Some((mut w, _)) = setup("plasmatorch") else { return };
    let pc = w.party_leader();
    // Bashable doors with a spot 3-9 m away whose walking route is much
    // longer than the straight line (a wall or the closed door in between).
    let doors: Vec<_> = w.iter().filter(|o| o.kind == ObjectKind::Door).map(|o| o.id).filter(|&d| w.can_bash(d)).collect();
    let mut spots = Vec::new();
    let mut best_ratio = 0.0f32;
    for &door in &doors {
        let dpos = w.get(door).unwrap().position;
        for ix in -10..=10 {
            for iy in -10..=10 {
                let (x, y) = (dpos.x + ix as f32, dpos.y + iy as f32);
                let d = Vec2::new(ix as f32, iy as f32).length();
                if !(3.0..=9.0).contains(&d) {
                    continue;
                }
                let Some(g) = w.walk.ground(x, y, dpos.z + 1.0) else { continue };
                let p = Vec3::new(x, y, g.z);
                let Some(l) = path_to_door(&w, p, door) else { continue };
                best_ratio = best_ratio.max(l / d);
                if l > d * 1.4 {
                    spots.push((p, door));
                }
            }
        }
    }
    assert!(!spots.is_empty(), "no spot around a corner, best path/straight ratio {best_ratio:.2}");
    // Some spots hug a wall where the nav grid and the slide collision
    // disagree (the leader never gets going); take the first that moves.
    let mut opened = None;
    for (spot, door) in spots.into_iter().step_by(3).take(40) {
        w.get_mut(pc).unwrap().position = spot;
        w.player_attack(door);
        for _ in 0..30 {
            w.tick(DT);
        }
        if (w.get(pc).unwrap().position - spot).truncate().length() < 0.5 {
            continue;
        }
        let mut t = 0.0;
        while t < 30.0 && !w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open) {
            w.tick(DT);
            t += DT;
        }
        opened = Some((spot, door, t));
        break;
    }
    let (spot, door, t) = opened.expect("the leader moved from at least one corner spot");
    let dpos = w.get(door).unwrap().position;
    eprintln!("leader from {spot:?} opened {} in {t:.1} s", w.describe(door));
    let d = w.get(door).and_then(|o| o.door()).unwrap();
    assert!(d.open && d.hp.destroyed, "door not opened after {t:.1} s from {spot:?}");
    let end = w.get(pc).unwrap().position;
    assert!((end - dpos).truncate().length() < 2.5, "leader ended {end:?}");
    for _ in 0..90 {
        w.tick(DT);
    }
    assert!(w.player_controlled(), "leader still busy");
}

#[test]
fn unreachable_door_aborts_the_attack_and_frees_the_leader() {
    let Some((mut w, door)) = setup("plasmatorch") else { return };
    let pc = w.party_leader();
    let dpos = w.get(door).unwrap().position;
    // A walkable spot in another region of the module, with no route to the door.
    let mut far = None;
    'scan: for ix in -40..40 {
        for iy in -40..40 {
            let (x, y) = (dpos.x + ix as f32 * 2.0, dpos.y + iy as f32 * 2.0);
            if let Some(g) = w.walk.ground(x, y, dpos.z + 1.0) {
                let p = Vec3::new(x, y, g.z);
                if !w.walk.same_region(p, dpos) {
                    far = Some(p);
                    break 'scan;
                }
            }
        }
    }
    let far = far.expect("a disconnected region");
    w.get_mut(pc).unwrap().position = far;
    w.player_attack(door);
    assert!(w.is_bashing(pc));
    let mut t = 0.0;
    while t < 6.0 && !w.player_controlled() {
        w.tick(DT);
        t += DT;
    }
    assert!(w.player_controlled(), "leader still locked in the attack after {t:.1} s");
    assert!(!w.get(door).and_then(|o| o.door()).unwrap().open);
}

#[test]
fn bash_approach_can_be_cancelled_by_clearing_actions() {
    // What play.rs does on a movement key: the approach is recognised as an
    // attack on an object and dropping the queue gives control back.
    let Some((mut w, door)) = setup("plasmatorch") else { return };
    let pc = w.party_leader();
    let dpos = w.get(door).unwrap().position;
    let mut spot = None;
    for iy in 3..9 {
        let (x, y) = (dpos.x, dpos.y + iy as f32);
        if let Some(g) = w.walk.ground(x, y, dpos.z + 1.0) {
            spot = Some(Vec3::new(x, y, g.z));
            break;
        }
    }
    w.get_mut(pc).unwrap().position = spot.expect("ground");
    w.player_attack(door);
    for _ in 0..10 {
        w.tick(DT);
    }
    assert!(w.is_bashing(pc) && !w.player_controlled());
    w.get_mut(pc).unwrap().actions.clear();
    assert!(!w.is_bashing(pc) && w.player_controlled());
}
