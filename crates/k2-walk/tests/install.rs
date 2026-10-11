//! Tests against the real install. Each test skips (passes with a note) when
//! no install is found at K2_INSTALL or `../..` / `..` from the crate.

use std::time::Instant;

use glam::{Affine3A, Quat, Vec2, Vec3};
use k2_res::{Install, ResKey, Resources};
use k2_walk::{DoorState, Materials, WalkMesh, WalkMeshBuilder};
use rakata_core::ResourceType;
use rakata_formats::gff::GffValue;

fn resources() -> Option<Resources> {
    let cands = [std::env::var("K2_INSTALL").ok(), Some("../../..".into()), Some("../..".into()), Some("..".into())];
    for c in cands.into_iter().flatten() {
        if let Ok(i) = Install::discover(&c) {
            return Resources::open(i).ok();
        }
    }
    eprintln!("no KotOR2 install found; skipping");
    None
}

fn materials(res: &Resources) -> Materials {
    let b = res.get(&ResKey::new("surfacemat", ResourceType::TwoDa)).unwrap().unwrap();
    Materials::from_2da_bytes(&b).unwrap()
}

fn area(res: &mut Resources, module: &str) -> Option<(WalkMesh, Vec3)> {
    res.load_module(module).ok()?;
    let lyt = res.get(&ResKey::new(module, ResourceType::Lyt)).ok()??;
    let lyt = rakata_formats::read_lyt_from_bytes(&lyt).ok()?;
    let mut b = WalkMeshBuilder::new(materials(res));
    for room in &lyt.rooms {
        if let Some(w) = res.get(&ResKey::new(&room.model, ResourceType::Wok)).unwrap() {
            b.add_room(&rakata_formats::read_bwm_from_bytes(&w).unwrap());
        }
    }
    let ifo = res.get(&ResKey::new("module", ResourceType::Ifo)).ok()??;
    let ifo = rakata_formats::read_gff_from_bytes(&ifo).ok()?;
    let f = |n: &str| match ifo.root.field(n) {
        Some(GffValue::Single(v)) => *v,
        _ => 0.0,
    };
    Some((b.build(), Vec3::new(f("Mod_Entry_X"), f("Mod_Entry_Y"), f("Mod_Entry_Z"))))
}

fn bwm(res: &Resources, name: &str, ty: ResourceType) -> Option<rakata_formats::Bwm> {
    res.get(&ResKey::new(name, ty)).unwrap().map(|b| rakata_formats::read_bwm_from_bytes(&b).unwrap())
}

/// Tiny deterministic PRNG so failures reproduce.
struct Rng(u64);
impl Rng {
    fn f(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[test]
fn entry_point_has_ground() {
    let Some(mut res) = resources() else { return };
    let (wm, entry) = area(&mut res, "101per").unwrap();
    let h = wm.ground(entry.x, entry.y, entry.z).expect("ground under 101per entry");
    assert!((h.z - entry.z).abs() < 0.5, "ground z {} vs entry z {}", h.z, entry.z);
    assert!(wm.materials().walkable(h.material));
}

#[test]
fn entry_points_all_modules() {
    let Some(mut res) = resources() else { return };
    let mods = res.install.module_names();
    let (mut ok, mut total) = (0, 0);
    let mut missing = Vec::new();
    for m in &mods {
        let Some((wm, e)) = area(&mut res, m) else { continue };
        total += 1;
        if wm.ground(e.x, e.y, e.z).is_some_and(|h| (h.z - e.z).abs() < 1.0) {
            ok += 1;
        } else {
            missing.push(m.clone());
        }
    }
    eprintln!("entry ground {ok}/{total}; without: {missing:?}");
    assert!(ok * 100 >= total * 95, "entry ground {ok}/{total}");
}

#[test]
fn walking_into_walls_stays_on_walkmesh() {
    let Some(mut res) = resources() else { return };
    let (wm, entry) = area(&mut res, "101per").unwrap();
    let start = Vec3::new(entry.x, entry.y, wm.ground(entry.x, entry.y, entry.z).unwrap().z);
    let mut blocked = 0;
    for k in 0..16 {
        let a = k as f32 / 16.0 * std::f32::consts::TAU;
        let dir = Vec2::new(a.cos(), a.sin());
        let mut p = start;
        let mut travelled = 0.0;
        for _ in 0..200 {
            let n = wm.move_slide(p, dir * 0.25, 0.4);
            assert!(wm.is_walkable(n), "left walkmesh at {n:?} heading {k}");
            travelled += (n - p).truncate().dot(dir);
            p = n;
        }
        if travelled < 49.0 {
            blocked += 1;
        }
    }
    assert!(blocked >= 8, "only {blocked}/16 straight walks were stopped by walls");
}

#[test]
fn fuzz_random_moves_never_leave_walkable() {
    let Some(mut res) = resources() else { return };
    for module in ["101per", "201tel", "301nar", "701kor", "003ebo"] {
        let Some((wm, entry)) = area(&mut res, module) else { continue };
        let Some(g) = wm.ground(entry.x, entry.y, entry.z) else { continue };
        let mut p = Vec3::new(entry.x, entry.y, g.z);
        let mut rng = Rng(0x9e3779b97f4a7c15);
        let mut dist = 0.0;
        for i in 0..20_000 {
            let d = Vec2::new(rng.f() * 2.0 - 1.0, rng.f() * 2.0 - 1.0) * 1.5;
            let n = wm.move_slide(p, d, 0.35);
            let h = wm.ground(n.x, n.y, n.z).unwrap_or_else(|| panic!("{module} move {i}: no ground at {n:?} from {p:?} d {d:?}"));
            assert!(wm.materials().walkable(h.material));
            assert!((h.z - n.z).abs() < 1e-3, "{module} move {i}: z {} vs ground {}", n.z, h.z);
            dist += (n - p).length();
            p = n;
        }
        eprintln!("{module}: travelled {dist:.0} m over 20000 random moves");
        assert!(dist > 1000.0, "{module}: actor barely moved ({dist})");
    }
}

#[test]
fn door_blocks_when_closed() {
    let Some(mut res) = resources() else { return };
    let (mut wm, entry) = area(&mut res, "101per").unwrap();
    // Find a spot with 3 m of free walking in some direction (near entry first).
    let free = |wm: &WalkMesh, start: Vec3, d: Vec2| {
        let mut p = start;
        for _ in 0..12 {
            p = wm.move_slide(p, d * 0.25, 0.3);
        }
        (p.truncate() - start.truncate()).dot(d) > 2.95
    };
    let mut rng = Rng(3);
    let (start, dir) = (0..4000)
        .filter_map(|i| {
            let spread = 1.0 + i as f32 * 0.05;
            let (x, y) = (entry.x + (rng.f() - 0.5) * spread, entry.y + (rng.f() - 0.5) * spread);
            let g = wm.ground(x, y, entry.z + 1.0)?;
            let s = Vec3::new(x, y, g.z);
            (0..16).map(|k| {
                let a = k as f32 / 16.0 * std::f32::consts::TAU;
                Vec2::new(a.cos(), a.sin())
            }).find(|d| free(&wm, s, *d)).map(|d| (s, d))
        })
        .next()
        .expect("a spot with 3 m of free walking");
    // Door width runs along its local X: rotate so X is perpendicular to dir.
    let yaw = dir.y.atan2(dir.x) + std::f32::consts::FRAC_PI_2;
    let at = start + (dir * 1.5).extend(0.0);
    let xf = Affine3A::from_rotation_translation(Quat::from_rotation_z(yaw), at);
    let door = wm.add_door(
        bwm(&res, "dor_dan010", ResourceType::Dwk).as_ref(),
        bwm(&res, "dor_dan011", ResourceType::Dwk).as_ref(),
        bwm(&res, "dor_dan012", ResourceType::Dwk).as_ref(),
        xf,
    );
    let walk = |wm: &WalkMesh| {
        let mut p = start;
        for _ in 0..12 {
            p = wm.move_slide(p, dir * 0.25, 0.3);
        }
        (p.truncate() - start.truncate()).dot(dir)
    };
    let closed = walk(&wm);
    assert!(closed < 1.5, "walked {closed} m through a closed door");
    wm.set_door(door, DoorState::Open1);
    let open = walk(&wm);
    assert!(open > 2.9, "open door still blocks: {open} m");
    assert!(!wm.line_of_sight(start + Vec3::Z, start + (dir * 3.0).extend(1.0)) || open > 2.9);
    wm.set_door(door, DoorState::Closed);
    assert!(!wm.line_of_sight(start + Vec3::Z, start + (dir * 3.0).extend(1.0)), "closed door should block line of sight");
}

#[test]
fn ground_query_speed() {
    let Some(mut res) = resources() else { return };
    let (wm, _) = area(&mut res, "101per").unwrap();
    let (mn, mx) = wm.bounds();
    let mut rng = Rng(42);
    let pts: Vec<Vec3> = (0..200_000).map(|_| Vec3::new(mn.x + rng.f() * (mx.x - mn.x), mn.y + rng.f() * (mx.y - mn.y), mx.z)).collect();
    let t = Instant::now();
    let mut hits = 0;
    for p in &pts {
        hits += wm.ground(p.x, p.y, p.z).is_some() as usize;
    }
    let ns = t.elapsed().as_nanos() as f64 / pts.len() as f64;
    eprintln!("ground: {ns:.0} ns/query, {hits} hits of {}, faces {}, blocking segments {}", pts.len(), wm.face_count(), wm.segment_count());
    let mut rng = Rng(7);
    let e = wm.bounds();
    let _ = e;
    let t = Instant::now();
    let n = 20_000;
    let mut p = pts.iter().find(|p| wm.ground(p.x, p.y, p.z).is_some()).copied().unwrap();
    p.z = wm.ground(p.x, p.y, p.z).unwrap().z;
    for _ in 0..n {
        p = wm.move_slide(p, Vec2::new(rng.f() - 0.5, rng.f() - 0.5) * 0.2, 0.4);
    }
    eprintln!("move_slide (0.1 m step): {:.0} ns/call", t.elapsed().as_nanos() as f64 / n as f64);
    if !cfg!(debug_assertions) {
        assert!(ns < 5_000.0, "ground query {ns} ns");
    }
}
