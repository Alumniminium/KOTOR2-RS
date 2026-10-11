//! Pathfinding in the world: scripted moves and party follow. Skips when no
//! install is found.

use std::io::Write;
use glam::{Vec2, Vec3, Vec3Swizzles};
use k2_res::Install;
use k2_world::object::{Action, Target};
use k2_world::World;
use k2_world::nav::UseStep;

const DT: f32 = 1.0 / 30.0;

/// Nodes a route check may expand. A search that finds no route exhausts its component, so each
/// failing check costs about this many expansions (tens of ms); the helpers below make few of them.
const PATH_CAP: usize = 20_000;
/// Route checks a helper may make before it gives up on finding a point.
const PROBES: usize = 150;

/// Whether a route from `from` to `to` is found within `PATH_CAP` nodes.
fn route_within_cap(w: &World, from: Vec3, to: Vec3) -> bool {
    matches!(w.walk.find_route_nodes_checked(from, to, 0.3, PATH_CAP), Ok(route) if !route.cut)
}

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(&dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP nav test: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    Some(World::new(install).unwrap_or_else(|e| panic!("{dir}: the install does not load into a world: {e:?}")))
}

/// A walkable point 20-40 m of path away from `from`.
fn far_point(w: &World, from: Vec3) -> (Vec3, Vec<Vec3>) {
    let (mn, mx) = w.walk.walkable_bounds();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut probes = 0;
    for _ in 0..20_000 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let (fx, fy) = ((seed & 0xffff) as f32 / 65535.0, ((seed >> 16) & 0xffff) as f32 / 65535.0);
        let (x, y) = (mn.x + fx * (mx.x - mn.x), mn.y + fy * (mx.y - mn.y));
        let Some(h) = w.walk.ground(x, y, from.z + 0.5) else { continue };
        let p = Vec3::new(x, y, h.z);
        let d = p.distance(from);
        if !(20.0..40.0).contains(&d) {
            continue;
        }
        if probes == PROBES {
            break;
        }
        probes += 1;
        // The route a walker takes: a locked door is a wall for it (see find_path_avoiding_locked).
        if route_within_cap(w, from, p)
            && let Some(path) = w.walk.find_path_avoiding_locked(from, p, 0.3)
            && path.len() >= 2
        {
            return (p, path);
        }
    }
    panic!("no far point within {PROBES} route checks");
}

#[test]
fn party_member_follows_the_leader_along_a_path() {
    let Some(mut w) = world() else { return };
    w.enter_module("201tel", None).unwrap();
    let leader = w.party_leader();
    let start = w.get(leader).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", start + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    w.party.push(kreia);
    let (goal, path) = far_point(&w, start);
    // 201TEL keeps restarting a bay-control cutscene conversation; skip it
    // like a player would, since nobody follows during a conversation.
    let advance = |w: &mut World| {
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
        w.tick(DT);
    };

    // Walk the leader along its path at run speed, as the Bevy layer would.
    let mut pos = start;
    for wp in path {
        // Bounded: a step the walls refuse leaves the leader in place, and the checks below report it.
        for _ in 0..(20.0 / DT) as usize {
            if (wp - pos).truncate().length() <= 0.05 {
                break;
            }
            let step = (wp - pos).truncate().clamp_length_max(5.4 * DT);
            pos = w.walk.move_slide(pos, step, 0.3);
            w.set_player_position(pos, 0.0);
            advance(&mut w);
        }
    }
    for _ in 0..(8.0 / DT) as usize {
        advance(&mut w);
    }
    let lp = w.get(leader).unwrap().position;
    let kp = w.get(kreia).unwrap().position;
    let d = (kp - lp).truncate().length();
    let _ = writeln!(std::io::stderr(), "leader {lp:?} (goal {goal:?}), kreia {kp:?}, gap {d:.2} m, kreia moved {:.1} m", kp.distance(start));
    assert!(d <= 3.5, "follower {d} m behind");
    assert!(kp.distance(start) > 10.0, "follower barely moved");
    assert!(w.walk.is_walkable(kp), "follower off the walkmesh at {kp:?}");
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

/// Paths to every placeable and door (their own origin, which sits inside
/// the footprint). A target counts as unreachable only when none of 24 start
/// points 8 m around it finds a route. Report only.
#[test]
#[ignore]
fn object_targets_report() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    let objs: Vec<(String, Vec3)> = w
        .iter()
        .filter(|o| matches!(o.kind, k2_world::ObjectKind::Placeable | k2_world::ObjectKind::Door))
        .map(|o| (o.tag.clone(), o.position))
        .collect();
    let mut none = Vec::new();
    for (tag, p) in &objs {
        let found = (0..24).any(|k| {
            let a = k as f32 * std::f32::consts::TAU / 24.0;
            let q = *p + Vec3::new(a.cos(), a.sin(), 0.0) * 8.0;
            let Some(h) = w.walk.ground(q.x, q.y, q.z + 1.0) else { return false };
            route_within_cap(&w, Vec3::new(q.x, q.y, h.z), *p)
        });
        if !found {
            none.push(format!("{tag} at {p:?}"));
        }
    }
    let _ = writeln!(std::io::stderr(), "101per: {} of {} object targets have no path: {none:?}", none.len(), objs.len());
}

#[test]
fn scripted_move_goes_around_walls() {
    let Some(mut w) = world() else { return };
    w.enter_module("201tel", None).unwrap();
    let leader = w.party_leader();
    let start = w.get(leader).unwrap().position;
    // Pick a target whose path bends (not a straight line).
    let (goal, path) = far_point(&w, start);
    assert!(path.len() >= 2);
    let npc = w.spawn_creature("p_kreia", start + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    w.push_action(npc, Action::MoveTo { target: Target::Point(goal), run: true, range: 0.3, timeout: None, elapsed: 0.0, stuck: 0.0 });
    let mut t = 0.0;
    while t < 30.0 && !w.get(npc).unwrap().actions.is_empty() {
        w.tick(DT);
        t += DT;
        let p = w.get(npc).unwrap().position;
        assert!(w.walk.is_walkable(p), "mover left the walkmesh at {p:?}");
    }
    let p = w.get(npc).unwrap().position;
    let _ = writeln!(std::io::stderr(), "scripted move: {:.1} m to go after {t:.1} s", (p - goal).truncate().length());
    assert!((p - goal).truncate().length() < 1.0, "mover ended {p:?}, goal {goal:?}");
}


/// A move to a placeable whose footprint no route enters ends at the nearest
/// reachable spot, instead of standing at the edge until the stuck timer runs
/// out.
#[test]
fn move_to_placeable_ends_at_the_nearest_spot() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    let (chair, cp) = w.iter().find(|o| o.tag == "Chair2").map(|o| (o.id, o.position)).expect("Chair2 in 101per");
    let from = (0..24)
        .find_map(|k| {
            let a = k as f32 * std::f32::consts::TAU / 24.0;
            let q = cp + Vec3::new(a.cos(), a.sin(), 0.0) * 8.0;
            let h = w.walk.ground(q.x, q.y, q.z + 1.0)?;
            let q = Vec3::new(q.x, q.y, h.z);
            route_within_cap(&w, q, cp).then_some(q)
        })
        .expect("a start point that reaches the chair");
    let npc = w.spawn_creature("p_kreia", from, 0.0).expect("p_kreia");
    w.push_action(npc, Action::MoveTo { target: Target::Object(chair), run: true, range: 0.5, timeout: None, elapsed: 0.0, stuck: 0.0 });
    let mut t = 0.0;
    while t < 20.0 && !w.get(npc).unwrap().actions.is_empty() {
        w.tick(DT);
        t += DT;
    }
    let p = w.get(npc).unwrap().position;
    let left = (p - cp).truncate().length();
    let _ = writeln!(std::io::stderr(), "move to Chair2: done after {t:.1} s, {left:.2} m from it");
    assert!(w.get(npc).unwrap().actions.is_empty(), "move still running after 20 s at {p:?}");
    assert!(t < 8.0, "move took {t:.1} s");
    assert!(left < 2.0, "ended {left:.2} m from the chair");
    assert!(w.walk.is_walkable(p), "ended off the walkmesh at {p:?}");
}

/// Proper 2D crossing of p0-p1 with q0-q1.
fn crosses(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> bool {
    let (r, s) = (p1 - p0, q1 - q0);
    let den = r.perp_dot(s);
    if den.abs() < 1e-9 {
        return false;
    }
    let t = (q0 - p0).perp_dot(s) / den;
    let u = (q0 - p0).perp_dot(r) / den;
    t > 1e-4 && t < 1.0 - 1e-4 && u > 1e-4 && u < 1.0 - 1e-4
}

/// True when the straight walk a -> b runs into a room wall.
fn wall_blocks(w: &World, a: Vec3, b: Vec3) -> bool {
    let n = ((b - a).truncate().length() / 0.25).ceil().max(1.0) as usize;
    let mut prev = a;
    for i in 1..=n {
        let p = a.lerp(b, i as f32 / n as f32);
        if w.walk.blockers_near(p, 0.5).iter().any(|&(s0, s1, room, _)| room && crosses(prev.xy(), p.xy(), s0.xy(), s1.xy())) {
            return true;
        }
        prev = p;
    }
    false
}

/// A walkable point 6-20 m from `from` whose straight walk runs into a room
/// wall, with a route around it.
fn wall_blocked_goal(w: &World, from: Vec3) -> Vec3 {
    let (mn, mx) = w.walk.walkable_bounds();
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut probes = 0;
    (0..20_000)
        .find_map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let (x, y) = (mn.x + (seed & 0xffff) as f32 / 65535.0 * (mx.x - mn.x), mn.y + ((seed >> 16) & 0xffff) as f32 / 65535.0 * (mx.y - mn.y));
            let h = w.walk.ground(x, y, from.z + 0.5)?;
            let p = Vec3::new(x, y, h.z);
            let d = p.distance(from);
            if !((6.0..20.0).contains(&d) && w.walk.same_region(from, p) && wall_blocks(w, from, p)) || probes == PROBES {
                return None;
            }
            probes += 1;
            (route_within_cap(w, from, p) && w.walk.find_path_avoiding_locked(from, p, 0.3).is_some()).then_some(p)
        })
        .unwrap_or_else(|| panic!("no wall-blocked goal 6-20 m away within {PROBES} route checks"))
}

/// A scripted move whose straight line runs into a wall (Ebon Hawk) must go
/// around it, and the mover must never step through the wall. The mover has
/// its AI scripts removed so only the move acts on it.
#[test]
fn scripted_move_goes_around_a_wall_on_the_ebon_hawk() {
    let Some(mut w) = world() else { return };
    w.enter_module("003ebo", None).unwrap();
    // Let the module's entry scripts settle; they clear actions on arrival.
    for _ in 0..(3.0 / DT) as usize {
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
        w.tick(DT);
    }
    let leader = w.party_leader();
    let start = w.get(leader).unwrap().position;
    let npc = w.spawn_creature("p_kreia", start + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    w.get_mut(npc).unwrap().scripts.clear();
    let from = w.get(npc).unwrap().position;
    let goal = wall_blocked_goal(&w, from);
    w.push_action(npc, Action::MoveTo { target: Target::Point(goal), run: true, range: 0.5, timeout: None, elapsed: 0.0, stuck: 0.0 });
    let mut t = 0.0;
    let mut prev = from;
    while t < 30.0 && !w.get(npc).unwrap().actions.is_empty() {
        w.tick(DT);
        t += DT;
        let p = w.get(npc).unwrap().position;
        assert!(w.walk.is_walkable(p), "mover left the walkmesh at {p:?}");
        assert!(!wall_blocks(&w, prev, p), "mover stepped through a wall from {prev:?} to {p:?}");
        prev = p;
    }
    let p = w.get(npc).unwrap().position;
    let _ = writeln!(std::io::stderr(), "ebon hawk wall detour: {:.1} m straight, done after {t:.1} s, {:.2} m from goal", from.distance(goal), (p - goal).truncate().length());
    assert!(w.get(npc).unwrap().actions.is_empty(), "move still running after 30 s at {p:?}");
    assert!((p - goal).truncate().length() < 1.0, "ended {p:?}, goal {goal:?}");
}

/// Click-to-move (play.rs) drives the leader itself: each frame it aims at
/// `steer_leader` and walks one step with `move_slide`. Past a wall on the
/// Ebon Hawk it must go round, never through, and arrive.
#[test]
fn steer_leader_walks_round_a_wall_on_the_ebon_hawk() {
    let Some(mut w) = world() else { return };
    w.enter_module("003ebo", None).unwrap();
    for _ in 0..(3.0 / DT) as usize {
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
        w.tick(DT);
    }
    let leader = w.party_leader();
    let from = w.get(leader).unwrap().position;
    let goal = wall_blocked_goal(&w, from);
    let mut pos = from;
    let mut t = 0.0;
    let mut stopped = false;
    while t < 60.0 && (pos - goal).truncate().length() > 0.5 {
        let Some(aim) = w.steer_leader(pos, goal) else {
            stopped = true;
            break;
        };
        let step = (aim - pos).truncate().clamp_length_max(1.75 * DT);
        let next = w.walk.move_slide(pos, step, 0.3);
        assert!(w.walk.is_walkable(next), "left the walkmesh at {next:?}");
        assert!(!wall_blocks(&w, pos, next), "stepped through a wall from {pos:?} to {next:?}");
        pos = next;
        w.tick(DT);
        t += DT;
    }
    let _ = writeln!(std::io::stderr(), "steer_leader: {:.1} m straight, {t:.1} s, {:.2} m from goal, stopped {stopped}", from.distance(goal), (pos - goal).truncate().length());
    assert!(!stopped, "no route reported for {goal:?}");
    assert!((pos - goal).truncate().length() <= 0.5, "stopped {pos:?}, goal {goal:?} after {t:.1} s");
}

/// A click on a placeable out of use reach walks to a spot within reach
/// (`use_point`) that the leader can reach. These three stalled short of reach
/// in play: 301nar MilLowMetalBoxLocked, 001ebo eng_star, 101per SecLoc. Each
/// is tried from a reachable point 8 m out.
#[test]
fn use_point_is_within_reach_for_the_stalled_placeables() {
    let Some(mut w) = world() else { return };
    for (module, tag) in [("301nar", "MilLowMetalBoxLocked"), ("001ebo", "eng_star"), ("101per", "SecLoc")] {
        w.enter_module(module, None).unwrap();
        let (id, tp) = w.iter().find(|o| o.tag.eq_ignore_ascii_case(tag)).map(|o| (o.id, o.position)).expect(tag);
        let start = (0..24)
            .find_map(|k| {
                let a = k as f32 * std::f32::consts::TAU / 24.0;
                let q = tp + Vec3::new(a.cos(), a.sin(), 0.0) * 8.0;
                let h = w.walk.ground(q.x, q.y, q.z + 1.0)?;
                let s = Vec3::new(q.x, q.y, h.z);
                route_within_cap(&w, s, tp).then_some(s)
            })
            .unwrap_or_else(|| panic!("no start 8 m from {tag}"));
        let stand = w.use_point(start, id).unwrap_or_else(|| panic!("{tag}: no spot within use reach"));
        let _ = writeln!(std::io::stderr(), "use point for {tag}: {:.2} m from it", stand.truncate().distance(tp.truncate()));
        assert!(w.in_use_reach(stand, id), "{tag}: {stand:?} is outside use reach");
        assert!(w.walk.find_path(start, stand, 0.3).is_some(), "{tag}: no route to {stand:?}");
    }
}

/// A walk that cannot reach its object ends with the game's own line (dialog.tlk
/// 1516), as a feedback event.
#[test]
fn an_unreachable_use_says_so_in_the_games_words() {
    let Some(mut w) = world() else { return };
    w.report_unreachable();
    let texts: Vec<String> = w
        .drain_events()
        .into_iter()
        .filter_map(|e| match e {
            k2_world::WorldEvent::Feedback { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["Unable to reach target!".to_owned()]);
}

/// A click's use search ends within the frame bound on every use target of
/// 506ond, reachable or not: one step a frame of 20k nodes, and the whole search
/// capped at k2-walk's full budget, so 400k nodes is 20 frames plus the frame
/// that finds the cap spent.
#[test]
fn a_click_on_any_use_target_of_506ond_resolves_within_the_frame_bound() {
    let Some(mut w) = world() else { return };
    w.enter_module("506ond", None).unwrap_or_else(|e| panic!("506ond: enter_module: {e:?}"));
    let from = w.get(w.party_leader()).expect("the leader").position;
    let targets: Vec<_> = w
        .iter()
        .filter(|o| match o.kind {
            k2_world::ObjectKind::Door => o.door().is_some_and(|d| !d.is_static),
            k2_world::ObjectKind::Placeable => o.placeable().is_some_and(|p| p.useable),
            _ => false,
        })
        .map(|o| (o.id, o.tag.clone()))
        .collect();
    assert!(!targets.is_empty(), "506ond has use targets");
    const FRAMES: usize = k2_walk::FULL_BUDGET / 20_000 + 1;
    let (mut stands, mut none) = (0, 0);
    for (id, tag) in &targets {
        let mut frames = 0;
        let outcome = match w.begin_use_search(from, *id) {
            None => None,
            Some(mut search) => loop {
                frames += 1;
                match w.step_use_search(&mut search) {
                    UseStep::Pending if frames < FRAMES => {}
                    step => break Some(step),
                }
            },
        };
        match outcome {
            Some(UseStep::Stand(at)) => {
                assert!(w.in_use_reach(at, *id), "{id} ({tag}): the stand {at:?} is not within use reach");
                stands += 1;
            }
            Some(UseStep::NoStand) | None => none += 1,
            Some(UseStep::Pending) => panic!("{id} ({tag}): the use search runs past {FRAMES} frames"),
        }
    }
    let _ = writeln!(std::io::stderr(), "506ond: {} use targets, {stands} with a stand, {none} without", targets.len());
}
