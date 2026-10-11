//! Walkers and closed doors: a mover whose route runs through an unlocked closed door
//! opens it on the way and goes on. Skips (passes) when no install is found at
//! K2_INSTALL or ../.. .

use glam::{Vec2, Vec3};
use k2_world::object::{Action, Target};
use k2_world::{ObjectKind, World};
mod support;

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

/// The closed doors a route of `points` runs through.
fn doors_on(w: &World, points: &[Vec3]) -> Vec<u32> {
    points.windows(2).filter_map(|leg| w.door_across(leg[0], leg[1])).collect()
}

/// How the candidate points beside a door fell out, by the stage each stopped at.
#[derive(Debug, Default)]
struct Misses {
    /// No crossing direction: no segment across the door's centre crosses its blocker.
    normal: bool,
    /// No floor under the point.
    ground: usize,
    /// No use stand for the target from the point.
    stand: usize,
    /// No route from the point to the stand at the clearance.
    route: usize,
    /// A route to the stand that does not cross the door.
    around: usize,
}

/// A point beside `door` (either side of it, along its normal) from which a walk at
/// `clearance` to a stand for `target` crosses the door, with that stand.
fn door_crossing(w: &World, door: u32, target: u32, clearance: f32, misses: &mut Misses) -> Option<(Vec3, Vec3)> {
    let door_at = w.get(door)?.position;
    // The normal: the direction whose segment across the door's centre crosses the door's blocker.
    let normal = (0..36)
        .map(|k| {
            let t = k as f32 * std::f32::consts::TAU / 36.0;
            Vec2::new(t.cos(), t.sin())
        })
        .find(|u| {
            let a = Vec3::new(door_at.x + u.x * 0.6, door_at.y + u.y * 0.6, door_at.z + 0.5);
            let b = Vec3::new(door_at.x - u.x * 0.6, door_at.y - u.y * 0.6, door_at.z + 0.5);
            w.door_across(a, b) == Some(door)
        });
    let Some(normal) = normal else {
        misses.normal = true;
        return None;
    };
    let along = Vec2::new(-normal.y, normal.x);
    for r in [1.0_f32, 1.5, 2.0, 2.5] {
        for side in [1.0_f32, -1.0] {
            for slide in [0.0_f32, 0.4, -0.4] {
                let q = door_at.truncate() + normal * (side * r) + along * slide;
                let Some(h) = w.walk.ground_nearest(q.x, q.y, door_at.z) else {
                    misses.ground += 1;
                    continue;
                };
                let p = Vec3::new(q.x, q.y, h.z);
                let Some(stand) = w.use_point(p, target) else {
                    misses.stand += 1;
                    continue;
                };
                let Some(route) = w.walk.find_path(p, stand, clearance) else {
                    misses.route += 1;
                    continue;
                };
                let mut legs = vec![p];
                legs.extend(route);
                if doors_on(w, &legs).contains(&door) {
                    return Some((p, stand));
                }
                misses.around += 1;
            }
        }
    }
    None
}

#[test]
fn an_npc_route_through_an_unlocked_closed_door_opens_it() {
    let Some(mut w) = settled() else { return };
    let computer = w
        .iter()
        .find(|o| o.kind == ObjectKind::Placeable && w.name_of(o.id) == "Medical Computer")
        .map(|o| o.id)
        .expect("101per has the Medical Computer");
    let leader = w.party_leader();

    // The PeragusDoor1 nearest the bed side (the door between the bed and the computer room). Object
    // ids differ between the game and the harness, so the door is found by its tag and place.
    let bed_side = Vec2::new(-10.77, 6.78);
    let door = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Door && o.tag == "PeragusDoor1")
        .min_by(|a, b| a.position.truncate().distance(bed_side).total_cmp(&b.position.truncate().distance(bed_side)))
        .map(|o| o.id)
        .expect("101per has PeragusDoor1 doors");
    // The door is closed and unlocked and stands between the Medical Computer and the bed side.
    if w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open) {
        w.set_door_open(door, false, leader);
    }
    let door_state = w.get(door).and_then(|o| o.door()).map(|d| format!("open {}, locked {}, static {}", d.open, d.locked, d.is_static));

    // The creature stands on the far side of the door from the computer: a ground point beside
    // the door whose route to a stand for the computer crosses the door. The first creature
    // whose clearance has such a point is the walker; the rest say why they had none. The walker
    // must be at odds with no creature in the area, so it only walks (no attacks, no combat).
    let all: Vec<u32> = w.iter().filter(|o| o.kind == ObjectKind::Creature && o.creature().is_some_and(|c| !c.dead)).map(|o| o.id).collect();
    let mut report = Vec::new();
    let mut placed = None;
    for npc in all.iter().copied().filter(|&c| c != leader) {
        if let Some(enemy) = all.iter().copied().find(|&o| o != npc && (w.is_enemy(npc, o) || w.is_enemy(o, npc))) {
            report.push(format!("{} is at odds with {}", w.name_of(npc), w.name_of(enemy)));
            continue;
        }
        // The route at the clearance the creature walks with (nav.rs steer uses the same).
        let clearance = w.get(npc).and_then(|o| o.creature()).map_or(0.3, |c| c.radius.min(0.5));
        let mut misses = Misses::default();
        if let Some((at, stand)) = door_crossing(&w, door, computer, clearance, &mut misses) {
            placed = Some((npc, at, stand));
            break;
        }
        report.push(format!("{} (clearance {clearance}): {misses:?}", w.name_of(npc)));
    }
    let (npc, at, stand) = placed.unwrap_or_else(|| {
        panic!("no creature in 101per walks from beside door {door} to the Medical Computer through it; door {door} {door_state:?}; {}", report.join("; "))
    });
    w.place(npc, at);

    w.push_action(npc, Action::MoveTo { target: Target::Point(stand), run: false, range: 0.5, timeout: Some(120.0), elapsed: 0.0, stuck: 0.0 });
    // Every 3 s: where the NPC is, its queued action and whether the door is open (for a failure).
    let mut trace = Vec::new();
    for step in 0..(60.0 * 30.0) as usize {
        w.tick(1.0 / 30.0);
        let _ = w.drain_events();
        if step % 90 == 0 {
            let o = w.get(npc).expect("the creature is in the world");
            let open = w.get(door).and_then(|d| d.door()).is_some_and(|d| d.open);
            trace.push(format!("{:.0} s at {:?}, action {:?}, door open {open}", w.time, o.position, o.actions.front()));
        }
    }
    let end = w.get(npc).expect("the creature is in the world").position;
    assert!(w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open), "the NPC's route from {at:?} did not open closed door {door} on its way; the NPC stands at {end:?}, its stand is {stand:?}; {}", trace.join("; "));
    assert!(end.truncate().distance(stand.truncate()) <= 2.0, "the NPC stopped at {end:?}, not near its stand {stand:?}; {}", trace.join("; "));
}
