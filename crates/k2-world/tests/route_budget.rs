//! Route searches over doorway links finish within a node cap. A route from beside the Kreia bed to a
//! stand for the Medical Computer crosses the closed door between them (a doorway link), and must be
//! found, not cut off by the cap. Skips (passes) when no install is found at K2_INSTALL or ../.. .

use glam::Vec3;
use k2_world::{ObjectKind, World};
mod support;

/// Nodes the route may expand. A route over a doorway link is found well inside this.
const CAP: usize = 20_000;

/// 101per entered, with the leader's start as the module places it.
fn peragus_world() -> Option<World> {
    let mut w = World::new(support::install()?).expect("the install loads into a world");
    w.enter_module("101per", None).expect("101per enters");
    Some(w)
}

#[test]
fn a_route_over_a_closed_doorway_link_finishes_within_the_node_cap() {
    let Some(mut w) = peragus_world() else { return };
    let pc = w.party_leader();
    let spot = Vec3::new(-1.29, 19.93, 9.02);
    w.place(pc, spot);
    let computer = w.iter().find(|o| o.kind == ObjectKind::Placeable && w.name_of(o.id) == "Medical Computer").map(|o| o.id).expect("101per has the Medical Computer");
    let stand = w.use_point(spot, computer).expect("a spot within use reach of the computer, from the bed");

    match w.walk.find_route_nodes_checked(spot, stand, w.leader_clearance(), CAP) {
        Ok(route) => assert!(!route.cut, "the route from the bed to {stand:?} ran out of the {CAP}-node cap"),
        Err(why) => panic!("no route over the doorway from the bed to {stand:?} within {CAP} nodes: {why:?}"),
    }
}
