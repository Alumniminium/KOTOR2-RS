//! Pathfinding tests against the real install. Skip when no install exists.

use std::io::Write;
use std::time::Instant;

use glam::{Affine3A, Quat, Vec2, Vec3, Vec3Swizzles};
use k2_res::{Install, ResKey, Resources};
use k2_walk::{DoorState, FULL_BUDGET, Materials, RouteMiss, STEP_DOWN, STEP_UP, SpotStep, WalkMesh, WalkMeshBuilder};
use rakata_core::ResourceType;
use rakata_formats::gff::GffValue;

fn resources() -> Option<Resources> {
    if let Ok(dir) = std::env::var("K2_INSTALL") {
        let install = Install::discover(&dir).unwrap_or_else(|e| panic!("K2_INSTALL={dir} is not an install: {e:?}"));
        return Some(Resources::open(install).unwrap_or_else(|e| panic!("K2_INSTALL={dir} does not open: {e:?}")));
    }
    for c in ["../../..", "../..", ".."] {
        if let Ok(i) = Install::discover(c) {
            return Some(Resources::open(i).unwrap_or_else(|e| panic!("{c}: the install does not open: {e:?}")));
        }
    }
    let _ = writeln!(std::io::stderr(), "SKIP k2-walk nav tests: no KotOR2 install (set K2_INSTALL)");
    None
}

fn area(res: &mut Resources, module: &str) -> (WalkMesh, Vec3) {
    res.load_module(module).unwrap_or_else(|e| panic!("{module}: load_module: {e:?}"));
    let mats = res.get(&ResKey::new("surfacemat", ResourceType::TwoDa)).unwrap().expect("surfacemat.2da is in the install");
    let lyt = res.get(&ResKey::new(module, ResourceType::Lyt)).unwrap().unwrap_or_else(|| panic!("{module}: no LYT"));
    let lyt = rakata_formats::read_lyt_from_bytes(&lyt).unwrap_or_else(|e| panic!("{module}: LYT does not parse: {e:?}"));
    let mut b = WalkMeshBuilder::new(Materials::from_2da_bytes(&mats).expect("surfacemat.2da parses"));
    for room in &lyt.rooms {
        if let Some(w) = res.get(&ResKey::new(&room.model, ResourceType::Wok)).unwrap() {
            b.add_room(&rakata_formats::read_bwm_from_bytes(&w).unwrap());
        }
    }
    let ifo = res.get(&ResKey::new("module", ResourceType::Ifo)).unwrap().unwrap_or_else(|| panic!("{module}: no module.ifo"));
    let ifo = rakata_formats::read_gff_from_bytes(&ifo).unwrap_or_else(|e| panic!("{module}: module.ifo does not parse: {e:?}"));
    let f = |n: &str| match ifo.root.field(n) {
        Some(GffValue::Single(v)) => *v,
        _ => panic!("{module}: module.ifo has no float {n}"),
    };
    (b.build(), Vec3::new(f("Mod_Entry_X"), f("Mod_Entry_Y"), f("Mod_Entry_Z")))
}

struct Rng(u64);
impl Rng {
    fn f(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Random walkable points (on the room floor at the highest layer).
fn random_points(wm: &WalkMesh, rng: &mut Rng, n: usize) -> Vec<Vec3> {
    let (mn, mx) = wm.bounds();
    let mut out = Vec::new();
    for _ in 0..n * 200 {
        if out.len() == n {
            break;
        }
        let (x, y) = (mn.x + rng.f() * (mx.x - mn.x), mn.y + rng.f() * (mx.y - mn.y));
        if let Some(h) = wm.ground(x, y, mx.z + 10.0) {
            out.push(Vec3::new(x, y, h.z));
        }
    }
    out
}

/// Proper 2D crossing of p0-p1 with q0-q1 (touching does not count).
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

/// The live blocker (true = room wall, false = object) that the straight
/// walk a -> b crosses, if any. Samples every 0.25 m, as an actor would.
fn blocked_line(wm: &WalkMesh, a: Vec3, b: Vec3) -> Option<bool> {
    let n = ((b - a).truncate().length() / 0.25).ceil().max(1.0) as usize;
    let mut prev = a;
    for i in 1..=n {
        let p = a.lerp(b, i as f32 / n as f32);
        for (s0, s1, room, _) in wm.blockers_near(p, 0.5) {
            if crosses(prev.xy(), p.xy(), s0.xy(), s1.xy()) {
                return Some(room);
            }
        }
        prev = p;
    }
    None
}

/// Walking each segment in 0.25 m steps, as an actor would (Z follows the
/// ground within step limits), never leaves walkable ground, never crosses a
/// live blocker, and each waypoint is reached at its own height.
fn path_on_ground(wm: &WalkMesh, from: Vec3, path: &[Vec3]) -> Result<(), String> {
    let mut a = from;
    let mut z = from.z;
    for &b in path {
        let len = (b - a).truncate().length();
        let n = (len / 0.25).ceil().max(1.0) as usize;
        for i in 1..=n {
            let p = a.lerp(b, i as f32 / n as f32);
            match wm.ground(p.x, p.y, z) {
                Some(h) if h.z >= z - k2_walk::STEP_DOWN => z = h.z,
                other => return Err(format!("off ground at {p:?} z {z} (segment {a:?} -> {b:?}, ground {other:?})")),
            }
        }
        if (z - b.z).abs() > 0.6 {
            return Err(format!("reached waypoint {b:?} at z {z}"));
        }
        if let Some(room) = blocked_line(wm, a, b) {
            return Err(format!("segment {a:?} -> {b:?} crosses a {} blocker", if room { "wall" } else { "object" }));
        }
        a = b;
    }
    Ok(())
}

#[test]
fn random_paths_stay_on_ground() {
    let Some(mut res) = resources() else { return };
    for module in ["101per", "201tel", "301nar", "701kor", "003ebo"] {
        let (wm, _) = area(&mut res, module);
        let t = Instant::now();
        let stats = wm.prepare_nav();
        let build = t.elapsed();
        let mut rng = Rng(0x1234_5678_9abc_def1);
        let pts = random_points(&wm, &mut rng, 160);
        let (mut pairs, mut found, mut worst) = (0, 0, 0.0f64);
        let mut total = 0.0f64;
        for pair in pts.chunks(2) {
            let [a, b] = [pair[0], pair[1]];
            if !wm.same_region(a, b) {
                continue;
            }
            pairs += 1;
            let t = Instant::now();
            let p = wm.find_path(a, b, 0.3);
            let ms = t.elapsed().as_secs_f64() * 1e3;
            total += ms;
            worst = worst.max(ms);
            if let Some(p) = p {
                found += 1;
                path_on_ground(&wm, a, &p).unwrap_or_else(|e| panic!("{module}: {e}"));
                let end = *p.last().unwrap_or(&a);
                assert!(end.truncate().distance(b.truncate()) <= 3.01, "{module}: path ends {end:?}, goal {b:?}");
            }
        }
        let _ = writeln!(
            std::io::stderr(),
            "{module}: nav {} nodes {} edges {} comps, built in {:.0} ms; {found}/{pairs} same-region pairs found, avg {:.2} ms, worst {worst:.2} ms",
            stats.nodes,
            stats.edges,
            stats.components,
            build.as_secs_f64() * 1e3,
            total / pairs.max(1) as f64
        );
        assert!(found * 100 >= pairs * 85, "{module}: only {found}/{pairs} paths found");
    }
}

/// Pairs whose straight line runs into a wall (101per, Ebon Hawk). The
/// route must go around, through a doorway, and never cross the wall.
#[test]
fn walls_on_the_straight_line_are_routed_around() {
    let Some(mut res) = resources() else { return };
    for module in ["101per", "003ebo"] {
        let (wm, _) = area(&mut res, module);
        wm.prepare_nav();
        let mut rng = Rng(7);
        let pts = random_points(&wm, &mut rng, 400);
        let (mut blocked, mut found) = (0, 0);
        for pair in pts.chunks(2) {
            let [a, b] = [pair[0], pair[1]];
            if !wm.same_region(a, b) || blocked_line(&wm, a, b) != Some(true) {
                continue;
            }
            blocked += 1;
            if let Some(p) = wm.find_path(a, b, 0.3) {
                found += 1;
                path_on_ground(&wm, a, &p).unwrap_or_else(|e| panic!("{module}: {e}"));
                let len: f32 = std::iter::once(a).chain(p.iter().copied()).collect::<Vec<_>>().windows(2).map(|w| (w[1] - w[0]).truncate().length()).sum();
                let straight = a.truncate().distance(b.truncate());
                assert!(len < straight * 6.0 + 20.0, "{module}: detour {len:.1} m for {straight:.1} m");
            }
        }
        let _ = writeln!(std::io::stderr(), "{module}: {found}/{blocked} wall-blocked straight lines routed");
        assert!(blocked >= 5, "{module}: too few wall-blocked pairs ({blocked})");
        assert!(found * 100 >= blocked * 90, "{module}: only {found}/{blocked} wall-blocked pairs routed");
    }
}

/// Follows each path the way the world does: aim at the next waypoint, step
/// with `move_slide` at the actor radius, and count how many reach the goal.
fn follow(wm: &WalkMesh, from: Vec3, path: &[Vec3], radius: f32) -> Result<Vec3, Vec3> {
    let mut pos = from;
    for &wp in path {
        let budget = (pos.truncate().distance(wp.truncate()) / 0.05) as usize * 3 + 60;
        let mut ok = false;
        for _ in 0..budget {
            let d = (wp - pos).truncate();
            if d.length() <= 0.35 {
                ok = true;
                break;
            }
            pos = wm.move_slide(pos, d.normalize() * 0.05, radius);
            if let Some(h) = wm.ground(pos.x, pos.y, pos.z + 0.5) {
                pos.z = h.z;
            }
        }
        if !ok {
            return Err(pos);
        }
    }
    Ok(pos)
}

/// Random pairs on the real areas: a path must be followable by an actor of
/// the same radius (creatures use hitradius, 0.3 to 0.5 m).
#[test]
fn paths_are_followable_by_actors() {
    let Some(mut res) = resources() else { return };
    for module in ["101per", "201tel", "003ebo"] {
        let (wm, _) = area(&mut res, module);
        wm.prepare_nav();
        let mut rng = Rng(42);
        let pts = random_points(&wm, &mut rng, 300);
        for radius in [0.3f32, 0.5] {
            let (mut pairs, mut unrouted, mut stuck) = (0, 0, Vec::new());
            for pair in pts.chunks(2) {
                let [a, b] = [pair[0], pair[1]];
                if !wm.same_region(a, b) {
                    continue;
                }
                let Some(path) = wm.find_path(a, b, radius) else {
                    unrouted += 1;
                    continue;
                };
                pairs += 1;
                if let Err(at) = follow(&wm, a, &path, radius) {
                    stuck.push((a, b, at));
                }
            }
            let _ = writeln!(std::io::stderr(), "{module} r {radius}: {unrouted} same-region pairs have no route; {} of {pairs} followed paths got stuck {:?}", stuck.len(), stuck.iter().take(3).collect::<Vec<_>>());
            assert!(unrouted * 100 <= (pairs + unrouted) * 15, "{module} r {radius}: {unrouted} of {} same-region pairs have no route", pairs + unrouted);
            assert!(stuck.len() * 100 <= pairs * 5, "{module} r {radius}: {} of {pairs} paths not followable", stuck.len());
        }
    }
}

/// Times the farthest same-region pair of 301nar: its distance in metres, its
/// waypoint count, and the median of ten searches in milliseconds.
fn cross_area_timing() -> Option<(f32, usize, f64)> {
    let mut res = resources()?;
    let (wm, _) = area(&mut res, "301nar");
    wm.prepare_nav();
    let mut rng = Rng(99);
    let pts = random_points(&wm, &mut rng, 400);
    // The farthest same-region pair is the cross-area case.
    let mut best = (0.0, pts[0], pts[0]);
    for (i, &a) in pts.iter().enumerate() {
        for &b in &pts[i + 1..] {
            let d = a.distance(b);
            if d > best.0 && wm.same_region(a, b) {
                best = (d, a, b);
            }
        }
    }
    let (d, a, b) = best;
    let mut times = Vec::new();
    let mut path = None;
    for _ in 0..10 {
        let t = Instant::now();
        path = wm.find_path(a, b, 0.3);
        times.push(t.elapsed().as_secs_f64() * 1e3);
    }
    times.sort_by(f64::total_cmp);
    let path = path.expect("cross-area path");
    path_on_ground(&wm, a, &path).unwrap();
    Some((d, path.len(), times[5]))
}

#[test]
fn cross_area_path_speed() {
    let Some((d, waypoints, median)) = cross_area_timing() else { return };
    let _ = writeln!(std::io::stderr(), "301nar cross-area {d:.0} m: {waypoints} waypoints, median {median:.2} ms");
}

/// The time budget of the cross-area route: 2 ms in release, 10 ms in debug builds.
/// Ignored by default: a parallel test run loads the machine and pushes the median
/// past any fixed bound, so the budget is checked alone: cargo test --release -p
/// k2-walk --test nav -- --ignored cross_area_path_within_budget
#[test]
#[ignore]
fn cross_area_path_within_budget() {
    let Some((_, _, median)) = cross_area_timing() else { return };
    let bound = if cfg!(debug_assertions) { 10.0 } else { 2.0 };
    assert!(median < bound, "cross-area path took {median:.2} ms (bound {bound} ms)");
}

#[test]
fn islands_fail_fast() {
    let Some(mut res) = resources() else { return };
    let (wm, entry) = area(&mut res, "101per");
    wm.prepare_nav();
    // The 101per entry point sits on a small walkmesh island under the bed.
    let e = Vec3::new(entry.x, entry.y, wm.ground(entry.x, entry.y, entry.z + 0.5).unwrap().z);
    let mut rng = Rng(5);
    let far = random_points(&wm, &mut rng, 200).into_iter().find(|p| p.distance(e) > 15.0 && !wm.same_region(e, *p)).expect("a point off the island");
    let t = Instant::now();
    let p = wm.find_path(e, far, 0.3);
    let us = t.elapsed().as_secs_f64() * 1e6;
    let _ = writeln!(std::io::stderr(), "island query: {us:.0} us");
    assert!(p.is_none());
    let bound = if cfg!(debug_assertions) { 5_000.0 } else { 500.0 };
    assert!(us < bound, "unreachable query took {us} us (bound {bound} us)");
}

#[test]
fn closed_door_reroutes_or_fails() {
    let Some(mut res) = resources() else { return };
    let (mut wm, _) = area(&mut res, "101per");
    let mut rng = Rng(11);
    let pts = random_points(&wm, &mut rng, 300);
    // A pair 6-12 m apart whose path is a straight line: put a door across it.
    let (a, b, path) = pts
        .chunks(2)
        .filter(|c| (6.0..12.0).contains(&c[0].distance(c[1])) && wm.same_region(c[0], c[1]))
        .find_map(|c| wm.find_path(c[0], c[1], 0.3).filter(|p| p.len() == 1).map(|p| (c[0], c[1], p)))
        .expect("a straight pair");
    assert_eq!(path.len(), 1);
    let dir = (b - a).truncate().normalize();
    let mid = a.lerp(b, 0.5);
    let yaw = dir.y.atan2(dir.x) + std::f32::consts::FRAC_PI_2;
    let xf = Affine3A::from_rotation_translation(Quat::from_rotation_z(yaw), mid);
    let get = |n: &str| res.get(&ResKey::new(n, ResourceType::Dwk)).unwrap().map(|b| rakata_formats::read_bwm_from_bytes(&b).unwrap());
    let door = wm.add_door(get("dor_dan010").as_ref(), get("dor_dan011").as_ref(), get("dor_dan012").as_ref(), xf);
    // Closed: either no path, or one that goes around (never through the leaf).
    let blocked = wm.find_path(a, b, 0.3);
    if let Some(p) = &blocked {
        let mut prev = a;
        for &q in p {
            let s = (q - prev).truncate();
            let (n0, n1) = ((prev - mid).truncate().dot(dir), (q - mid).truncate().dot(dir));
            if n0 < 0.0 && n1 > 0.0 {
                let t = -n0 / (n1 - n0);
                let cross = prev.truncate() + s * t;
                assert!(cross.distance(mid.truncate()) > 2.4, "path passes through the closed door at {cross:?}");
            }
            prev = q;
        }
    }
    wm.set_door(door, DoorState::Open1);
    let open = wm.find_path(a, b, 0.3).expect("path with the door open");
    let _ = writeln!(std::io::stderr(), "door test: closed -> {:?} waypoints, open -> {} waypoints", blocked.map(|p| p.len()), open.len());
    let _ = Vec2::ZERO;
}

#[test]
#[ignore]
fn bounds_report() {
    let Some(mut res) = resources() else { return };
    let mods = res.install.module_names();
    for m in mods {
        let (wm, _) = area(&mut res, &m);
        let (mn, mx) = wm.bounds();
        let (wmn, wmx) = wm.walkable_bounds();
        let _ = writeln!(std::io::stderr(), "{m}: all {:.0}x{:.0} walkable {:.0}x{:.0}", mx.x - mn.x, mx.y - mn.y, wmx.x - wmn.x, wmx.y - wmn.y);
    }
}

#[test]
#[ignore]
fn build_times() {
    let Some(mut res) = resources() else { return };
    for m in ["421dxn", "950cor", "201tel"] {
        let t = Instant::now();
        let (wm, _) = area(&mut res, m);
        let walk = t.elapsed();
        let t = Instant::now();
        let s = wm.prepare_nav();
        let _ = writeln!(std::io::stderr(), "{m}: walkmesh {:.0} ms, nav {:.0} ms, {} nodes", walk.as_secs_f64() * 1e3, t.elapsed().as_secs_f64() * 1e3, s.nodes);
    }
}

/// Checks the toolset's path points (module .pth, GFF) against our routes:
/// every point should stand on walkable ground and every stored connection
/// should be routable. Report only.
#[test]
#[ignore]
fn pth_report() {
    let Some(mut res) = resources() else { return };
    for module in ["101per", "201tel", "003ebo"] {
        let (wm, _) = area(&mut res, module);
        wm.prepare_nav();
        let bytes = res.get(&ResKey::new(module, ResourceType::Pth)).unwrap().unwrap_or_else(|| panic!("{module}: no PTH"));
        let root = rakata_formats::read_gff_from_bytes(&bytes).unwrap();
        let num = |s: &rakata_formats::gff::GffStruct, n: &str| match s.field(n) {
            Some(GffValue::Single(v)) => *v,
            Some(GffValue::Int32(v)) => *v as f32,
            Some(GffValue::UInt32(v)) => *v as f32,
            _ => f32::NAN,
        };
        let GffValue::List(points) = root.root.field("Path_Points").unwrap() else { panic!("{module}: Path_Points is not a list") };
        let GffValue::List(conns) = root.root.field("Path_Conections").unwrap() else { panic!("{module}: Path_Conections is not a list") };
        let pts: Vec<Vec3> = points
            .iter()
            .map(|p| {
                let (x, y) = (num(p, "X"), num(p, "Y"));
                let z = wm.ground(x, y, 100.0).map_or(f32::NAN, |h| h.z);
                Vec3::new(x, y, z)
            })
            .collect();
        let off = pts.iter().filter(|p| !p.z.is_finite()).count();
        let (mut tried, mut routed, mut worst_ratio) = (0, 0, 0.0f32);
        for (i, p) in points.iter().enumerate() {
            let first = num(p, "First_Conection") as usize;
            let count = num(p, "Conections") as usize;
            for c in conns.iter().skip(first).take(count) {
                let j = num(c, "Destination") as usize;
                let (a, b) = (pts[i], pts[j]);
                if !a.z.is_finite() || !b.z.is_finite() || !wm.same_region(a, b) {
                    continue;
                }
                tried += 1;
                if let Some(path) = wm.find_path(a, b, 0.3) {
                    routed += 1;
                    let len: f32 = std::iter::once(a).chain(path.iter().copied()).collect::<Vec<_>>().windows(2).map(|w| (w[1] - w[0]).truncate().length()).sum();
                    worst_ratio = worst_ratio.max(len / a.truncate().distance(b.truncate()).max(0.5));
                }
            }
        }
        let _ = writeln!(
            std::io::stderr(),
            "{module}: {} path points ({off} off walkable ground), {} connections, {routed}/{tried} routable, worst route/straight {worst_ratio:.2}",
            pts.len(),
            conns.len()
        );
    }
}

/// A bounded search gives up when its budget runs out, and with the full
/// budget it is exactly `find_path`. The unsmoothed node chain is walkable and
/// ends within the goal snap radius.
#[test]
fn budgets_bound_the_search_and_the_node_chain_is_walkable() {
    let Some(mut res) = resources() else { return };
    let (wm, _) = area(&mut res, "201tel");
    wm.prepare_nav();
    let mut rng = Rng(0x51ce_d00d_0001);
    let pts = random_points(&wm, &mut rng, 200);
    let (mut checked, mut unrouted) = (0, 0);
    for pair in pts.chunks(2) {
        let [a, b] = [pair[0], pair[1]];
        if a.distance(b) < 8.0 || !wm.same_region(a, b) {
            continue;
        }
        let Some(full) = wm.find_path(a, b, 0.3) else {
            unrouted += 1;
            continue;
        };
        checked += 1;
        assert_eq!(wm.find_path_within(a, b, 0.3, 400_000), Some(full), "the full budget is find_path");
        assert_eq!(wm.find_path_within(a, b, 0.3, 1), None, "one expansion cannot route {a:?} -> {b:?}");
        let nodes = wm.find_route_nodes_within(a, b, 0.3, 400_000).expect("the node chain of a routed pair");
        path_on_ground(&wm, a, &nodes).unwrap_or_else(|e| panic!("node chain: {e}"));
        let end = *nodes.last().expect("a chain ends somewhere");
        assert!(end.truncate().distance(b.truncate()) <= 3.01, "the chain ends {end:?}, goal {b:?}");
        if checked == 10 {
            break;
        }
    }
    assert!(checked >= 5, "only {checked} routed pairs in 201tel");
    let _ = writeln!(std::io::stderr(), "budgets: {checked} routed pairs in 201tel checked, {unrouted} same-region pairs without a route");
}

/// A goal whose nearest passable node is not reached: `astar` then ends the
/// route on the first candidate it reaches, within the snap radius and on the
/// ground. Counts how often a route ends well short of its goal.
#[test]
fn goals_no_route_enters_end_on_a_reached_candidate() {
    let Some(mut res) = resources() else { return };
    let mut short_total = 0;
    for module in ["101per", "201tel", "301nar"] {
        let (wm, _) = area(&mut res, module);
        wm.prepare_nav();
        let mut rng = Rng(0x0fa1_1bac_0001);
        let pts = random_points(&wm, &mut rng, 400);
        let (mut short, mut routed, mut unrouted) = (0, 0, 0);
        for pair in pts.chunks(2) {
            let [a, b] = [pair[0], pair[1]];
            if !wm.same_region(a, b) {
                continue;
            }
            let Some(p) = wm.find_path(a, b, 0.3) else {
                unrouted += 1;
                continue;
            };
            routed += 1;
            let end = *p.last().unwrap_or(&a);
            if end.truncate().distance(b.truncate()) > 1.0 {
                short += 1;
                assert!(end.truncate().distance(b.truncate()) <= 3.01, "{module}: route ends {end:?}, goal {b:?}");
                path_on_ground(&wm, a, &p).unwrap_or_else(|e| panic!("{module}: {e}"));
            }
        }
        let _ = writeln!(std::io::stderr(), "{module}: {short} routes end more than 1 m short of their goal, {unrouted} same-region pairs have no route");
        assert!(routed * 100 >= (routed + unrouted) * 85, "{module}: only {routed} of {} same-region pairs routed", routed + unrouted);
        short_total += short;
    }
    assert!(short_total > 0, "no route in the sample ended short: the fallback is not exercised");
}

/// A budget cut is never reported as unreachable: a pair the full budget
/// routes is routed by the bounded search too, or refused with `OverBudget`.
/// A cut route ends on the ground within the snap radius. Long pairs on 201tel
/// are where the 20k budget cuts.
#[test]
fn a_budget_cut_is_not_unreachable() {
    let Some(mut res) = resources() else { return };
    let (wm, _) = area(&mut res, "201tel");
    wm.prepare_nav();
    let mut rng = Rng(0x0b0d_6e70_0001);
    let pts = random_points(&wm, &mut rng, 400);
    let (mut sampled, mut routed, mut cut, mut refused) = (0, 0, 0, 0);
    for pair in pts.chunks(2) {
        let [a, b] = [pair[0], pair[1]];
        if a.distance(b) < 30.0 || !wm.same_region(a, b) {
            continue;
        }
        sampled += 1;
        if wm.find_route_nodes_checked(a, b, 0.3, FULL_BUDGET).is_err() {
            continue;
        }
        routed += 1;
        match wm.find_route_nodes_checked(a, b, 0.3, 20_000) {
            Ok(route) => {
                cut += usize::from(route.cut);
                path_on_ground(&wm, a, &route.nodes).unwrap_or_else(|e| panic!("cut route: {e}"));
                let end = *route.nodes.last().expect("a route ends somewhere");
                assert!(end.truncate().distance(b.truncate()) <= 3.01, "cut route ends {end:?}, goal {b:?}");
            }
            Err(RouteMiss::OverBudget) => refused += 1,
            Err(RouteMiss::Unreachable) => panic!("the full budget routes {a:?} -> {b:?}, the bounded search says unreachable"),
        }
        if routed == 60 {
            break;
        }
    }
    let _ = writeln!(std::io::stderr(), "budget cut: {routed} of {sampled} long pairs routed in full, {cut} cut, {refused} refused");
    assert!(routed * 100 >= sampled * 85, "only {routed} of {sampled} long pairs routed in full in 201tel");
    assert!(routed >= 10, "only {routed} long pairs in 201tel");
    assert!(cut + refused >= 1, "the 20k budget never cut a route: the test exercises nothing");
}

/// Every chain segment keeps clear of room walls by the radius: edges check
/// their wall gap, not only their nodes. The least sampled gap is the radius,
/// less a little for sampling.
#[test]
fn route_chains_keep_clear_of_room_walls() {
    let Some(mut res) = resources() else { return };
    for module in ["201tel", "101per"] {
        let (wm, _) = area(&mut res, module);
        wm.prepare_nav();
        let mut rng = Rng(0x0c1e_a4a4_0002);
        let pts = random_points(&wm, &mut rng, 200);
        let (mut sampled, mut routes, mut least) = (0, 0, f32::INFINITY);
        for pair in pts.chunks(2) {
            let [a, b] = [pair[0], pair[1]];
            if a.distance(b) < 8.0 || !wm.same_region(a, b) {
                continue;
            }
            sampled += 1;
            let Ok(route) = wm.find_route_nodes_checked(a, b, 0.3, FULL_BUDGET) else { continue };
            routes += 1;
            for seg in route.nodes.windows(2) {
                let (p, q) = (seg[0], seg[1]);
                let n = ((q - p).truncate().length() / 0.05).ceil().max(1.0) as usize;
                for i in 0..=n {
                    least = least.min(room_wall_gap(&wm, p.lerp(q, i as f32 / n as f32)));
                }
            }
        }
        let _ = writeln!(std::io::stderr(), "{module}: {routes} of {sampled} sampled pairs routed, least wall gap {least:.3} m");
        assert!(routes * 100 >= sampled * 85, "{module}: only {routes} of {sampled} sampled pairs routed");
        assert!(routes >= 5, "{module}: only {routes} chains");
        assert!(least >= 0.3 - 0.02, "{module}: a chain passes {least:.3} m from a room wall (radius 0.3)");
    }
}

/// Planar distance from `p` to the nearest room wall at its height, up to 1 m
/// (1 m when there is none).
fn room_wall_gap(wm: &WalkMesh, p: Vec3) -> f32 {
    wm.blockers_near(p, 1.0)
        .iter()
        .filter(|&&(s0, s1, room, _)| room && s0.z.min(s1.z) <= p.z + STEP_UP && s0.z.max(s1.z) >= p.z - STEP_DOWN)
        .map(|&(s0, s1, _, _)| {
            let (a, b) = (s0.xy(), s1.xy());
            let ab = b - a;
            let t = ((p.xy() - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
            p.xy().distance(a + ab * t)
        })
        .fold(1.0, f32::min)
}

/// Runs a spot search to its end, `budget` expansions a step.
fn run_spot_search(wm: &WalkMesh, mut search: k2_walk::SpotSearch, budget: usize) -> (Vec<Vec3>, usize) {
    let mut pending = 0;
    loop {
        match wm.spot_step(&mut search, budget) {
            SpotStep::Pending => pending += 1,
            SpotStep::Found(route) => return (route, pending),
            SpotStep::Miss(why) => panic!("the spot search missed: {why:?}"),
        }
    }
}

/// Two reachable spots: with exit 0 for both, the search stops at one of them
/// (the least route cost). Charge that one an exit of 1000 and the search takes
/// the other: exit cost outweighs the route.
#[test]
fn an_exit_cost_moves_the_stand_to_the_other_spot() {
    let Some(mut res) = resources() else { return };
    let (wm, _) = area(&mut res, "101per");
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let pts = random_points(&wm, &mut rng, 400);
    let from = pts[0];
    let a = *pts.iter().find(|p| p.xy().distance(from.xy()) >= 4.0 && wm.same_region(from, **p)).expect("a walkable spot 4 m from the start");
    let b = *pts
        .iter()
        .find(|p| p.xy().distance(from.xy()) >= 4.0 && p.xy().distance(a.xy()) >= 8.0 && wm.same_region(from, **p))
        .expect("a walkable spot 8 m from the first");
    let at = |spot: Vec3, p: Vec3| p.xy().distance(spot.xy()) <= 3.0;
    let search = wm.spot_search(from, &[a, b], 0.3, FULL_BUDGET, |p| (at(a, p) || at(b, p)).then_some(0.0)).expect("both spots have goals");
    let (route, _) = run_spot_search(&wm, search, FULL_BUDGET);
    let end = *route.last().expect("a route has an end");
    assert!(at(a, end) || at(b, end), "the search ended at {end:?}, not at either spot");
    let (first, other) = if at(a, end) { (a, b) } else { (b, a) };
    let search = wm
        .spot_search(from, &[a, b], 0.3, FULL_BUDGET, |p| {
            if at(first, p) {
                Some(1000.0)
            } else if at(other, p) {
                Some(0.0)
            } else {
                None
            }
        })
        .expect("both spots have goals");
    let (route, _) = run_spot_search(&wm, search, FULL_BUDGET);
    let end = *route.last().expect("a route has an end");
    assert!(at(other, end), "the search ended at {end:?}, not at the uncharged spot {other:?}");
}

/// Stepping a spot search a few expansions at a time finds the route a single
/// full step finds: the pauses change the frames, not the route.
#[test]
fn a_spot_search_stepped_in_pieces_finds_the_one_shot_route() {
    let Some(mut res) = resources() else { return };
    let (wm, _) = area(&mut res, "101per");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let pts = random_points(&wm, &mut rng, 200);
    let from = pts[0];
    let spot = *pts.iter().find(|p| p.xy().distance(from.xy()) >= 10.0 && wm.same_region(from, **p)).expect("a walkable spot 10 m from the start");
    let goal = |p: Vec3| (p.xy().distance(spot.xy()) <= 2.0).then_some(0.0);
    let one_shot = wm.spot_search(from, &[spot], 0.3, FULL_BUDGET, goal).expect("the spot has goals");
    let stepped = wm.spot_search(from, &[spot], 0.3, FULL_BUDGET, goal).expect("the spot has goals");
    let (route, _) = run_spot_search(&wm, one_shot, FULL_BUDGET);
    let (piecewise, pending) = run_spot_search(&wm, stepped, 7);
    assert!(pending > 0, "a 10 m route needs more than 7 expansions: the stepped search never paused");
    assert_eq!(route, piecewise, "a stepped search found a different route");
}
