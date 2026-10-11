//! Diagnostic, not a check: the closed doors' walk links in 101per, and the walk
//! components of the spots the walks start from and aim at. Ignored by default; run with
//! --ignored --nocapture. Skips (passes) when no install is found at K2_INSTALL or ../.. .

use std::io::Write;

use glam::Vec3;
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

#[test]
#[ignore]
fn diagnose_the_doorway_links_of_101per() {
    let Some(w) = settled() else { return };
    let mut out = std::io::stderr();
    for line in w.walk.door_link_report() {
        let _ = writeln!(out, "{line}");
    }
    let bed = Vec3::new(-1.29, 19.93, 9.02);
    let spot = Vec3::new(1.64, 23.98, 9.05);
    let _ = writeln!(out, "component of the bed spot {bed:?}: {:?}", w.walk.component_at(bed));
    let _ = writeln!(out, "component of the spot {spot:?}: {:?}", w.walk.component_at(spot));
    let computer = w.iter().find(|o| o.kind == ObjectKind::Placeable && w.name_of(o.id) == "Medical Computer").map(|o| o.id);
    if let Some(computer) = computer {
        let c = w.get(computer).expect("the computer is in the world").position;
        let _ = writeln!(out, "Medical Computer at {c:?}, component {:?}", w.walk.component_at(c));
        for stand in w.use_candidates(bed, computer) {
            let _ = writeln!(out, "  stand {stand:?}: component {:?}", w.walk.component_at(stand));
        }
    }
    if let Some(closet) = w.by_tag("MedClosetDoor", 0) {
        let d = w.get(closet).expect("the door is in the world").position;
        let _ = writeln!(out, "MedClosetDoor at {d:?}");
    }
}
