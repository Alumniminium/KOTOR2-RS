use std::io::Write;
use super::*;

fn install() -> Option<k2_res::Install> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = k2_res::Install::discover(&dir);
    if install.is_err() {
        let _ = writeln!(std::io::stderr(), "SKIP k2-world nav unit test: no install at {dir} (set K2_INSTALL)");
    }
    install.ok()
}

/// A walkable point `dist` m from `from` with a route back to it.
fn walkable_at(w: &World, from: Vec3, dist: f32) -> Option<Vec3> {
    (0..72).find_map(|k| {
        let a = k as f32 * std::f32::consts::TAU / 72.0;
        let q = from + Vec3::new(a.cos(), a.sin(), 0.0) * dist;
        let h = w.walk.ground(q.x, q.y, q.z + 1.0)?;
        let p = Vec3::new(q.x, q.y, h.z);
        w.walk.find_path(from, p, 0.3).is_some().then_some(p)
    })
}

/// A held follower stays put while the target is within FOLLOW_RESUME of
/// where it gave up, and walks again once the target is farther than that.
#[test]
fn a_held_follower_walks_again_only_once_the_target_is_a_resume_distance_farther() {
    let Some(install) = install() else { return };
    let mut w = World::new(install).unwrap();
    w.enter_module("201tel", None).unwrap();
    for _ in 0..30 {
        w.tick(1.0 / 30.0);
        let _ = w.drain_events();
    }
    let pc = w.party_leader();
    let start = w.get(pc).unwrap().position;
    let me = w.spawn_creature("p_kreia", start + Vec3::new(0.5, 0.0, 0.0), 0.0).unwrap();
    let me_at = w.get(me).unwrap().position;
    let target = w.spawn_creature("p_atton", start, 0.0).unwrap();
    let t0 = walkable_at(&w, me_at, 6.0).expect("a walkable point 6 m away");
    w.place(target, t0);
    let d0 = (t0 - me_at).truncate().length();
    w.paths.states.entry(me).or_default().follow_held = Some(d0);

    w.follow_towards(me, target, 2.0, 1.0 / 30.0);
    assert!(w.get(me).unwrap().position.distance(me_at) < 1e-4, "a held follower moved while the target stayed near");

    let t1 = walkable_at(&w, me_at, d0 + 1.5).expect("a walkable point 1.5 m farther");
    w.place(target, t1);
    w.follow_towards(me, target, 2.0, 1.0 / 30.0);
    assert!(w.get(me).unwrap().position.distance(me_at) > 1e-3, "the follower did not walk again after the target moved away");
}
