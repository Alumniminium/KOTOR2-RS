//! A sweep of the module waypoints: the leader placed on each WP_ waypoint of a module
//! can route a few metres away, by the route query and by the steering the leader's
//! walks use. Ignored by default (it loads whole modules); run with --ignored.
//! Skips (passes) when no install is found at K2_INSTALL or ../.. .

use std::io::Write;

use glam::{Vec2, Vec3};
use k2_world::World;
mod support;

/// The modules swept: 101per (the wake-up pocket and the medbay) and 001ebo (the prologue).
const MODULES: [&str; 2] = ["101per", "001ebo"];

/// A walkable point 3 or 4 m from `from` that a route from `from` ends at, or None.
fn point_a_few_metres_away(w: &World, from: Vec3) -> Option<Vec3> {
    let clear = w.leader_clearance();
    [3.0_f32, 4.0].into_iter().find_map(|r| {
        (0..16).find_map(|k| {
            let t = k as f32 * std::f32::consts::TAU / 16.0;
            let q = Vec2::new(from.x, from.y) + Vec2::new(t.cos(), t.sin()) * r;
            let to = Vec3::new(q.x, q.y, w.walk.ground(q.x, q.y, from.z + 1.0)?.z);
            let end = *w.walk.find_path(from, to, clear)?.last()?;
            (end.truncate().distance(to.truncate()) <= 0.6).then_some(to)
        })
    })
}

#[test]
#[ignore]
fn every_waypoint_routes_a_few_metres_away() {
    let mut failures: Vec<String> = Vec::new();
    let mut swept = 0;
    for module in MODULES {
        let Some(install) = support::install() else { return };
        let mut w = World::new(install).expect("the install loads into a world");
        w.enter_module(module, None).expect("the module enters");
        let _ = w.drain_events();
        let pc = w.party_leader();
        let waypoints: Vec<(String, Vec3)> = w.iter().filter(|o| o.tag.starts_with("WP_")).map(|o| (o.tag.clone(), o.position)).collect();
        for (tag, at) in waypoints {
            swept += 1;
            w.place(pc, at);
            let from = w.get(pc).expect("the leader is in the world").position;
            let Some(to) = point_a_few_metres_away(&w, from) else {
                failures.push(format!("{module} {tag} at {from:?}: no walkable point 3 to 4 m away has a route"));
                continue;
            };
            if w.steer_leader(from, to).is_none() {
                failures.push(format!("{module} {tag} at {from:?}: steering to {to:?} has no route"));
            }
        }
    }
    let mut out = std::io::stderr();
    for line in &failures {
        let _ = writeln!(out, "{line}");
    }
    assert!(failures.is_empty(), "{} of {swept} waypoints cannot route a few metres away:\n{}", failures.len(), failures.join("\n"));
}
