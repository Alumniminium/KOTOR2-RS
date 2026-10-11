//! The use reach of an object (`World::in_use_reach`): REACH plus the radius
//! the client gives what the player can point at. Objects the client gives no
//! Interactable (static doors without a conversation, unusable placeables) keep
//! the 0.5 default; a static door with a conversation talks, so it has the door radius.
//! Skips when no install is found.

use std::collections::HashSet;

use glam::Vec3;
use k2_res::Install;
use k2_world::{ObjectKind, World};

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(&dir) else {
        eprintln!("SKIP use reach test: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    World::new(install).ok()
}

#[test]
fn objects_without_an_interactable_use_the_default_radius() {
    let Some(mut w) = world() else { return };
    let mut seen = HashSet::new();
    for module in ["101per", "201tel", "301nar", "001ebo", "503ond"] {
        w.enter_module(module, None).unwrap();
        let objects: Vec<(k2_script::ObjectId, Vec3, &'static str)> = w
            .iter()
            .filter_map(|o| match o.kind {
                ObjectKind::Door if o.door().is_some_and(|d| d.is_static) && o.conversation.as_deref().is_none_or(|c| c.is_empty()) => Some((o.id, o.position, "static door")),
                ObjectKind::Door => Some((o.id, o.position, "usable door")),
                ObjectKind::Placeable if o.placeable().is_some_and(|p| p.useable) => Some((o.id, o.position, "usable placeable")),
                ObjectKind::Placeable => Some((o.id, o.position, "plain placeable")),
                _ => None,
            })
            .collect();
        for (id, p, kind) in objects {
            // Reach is 2.3 m for the default radius, 2.6 for a usable placeable, 3.0 for a usable door.
            let near = w.in_use_reach(p + Vec3::new(2.2, 0.0, 0.0), id);
            let far = w.in_use_reach(p + Vec3::new(2.5, 0.0, 0.0), id);
            match kind {
                "static door" | "plain placeable" => assert!(near && !far, "{module}: {kind} {id} reach is not 2.3 m"),
                "usable placeable" => assert!(near && far && !w.in_use_reach(p + Vec3::new(2.9, 0.0, 0.0), id), "{module}: {kind} {id} reach is not 2.6 m"),
                _ => assert!(near && far && !w.in_use_reach(p + Vec3::new(3.2, 0.0, 0.0), id), "{module}: {kind} {id} reach is not 3.0 m"),
            }
            seen.insert(kind);
        }
    }
    assert!(seen.contains("static door"), "no static door in the sampled modules");
    assert!(seen.contains("usable door"), "no usable door in the sampled modules");
}

#[test]
fn creature_reach_follows_the_leader_default_and_the_creature_radius() {
    let Some(mut w) = world() else { return };
    let mut floor_seen = false;
    for module in ["101per", "201tel", "301nar"] {
        w.enter_module(module, None).unwrap();
        let leader = w.party_leader();
        let p = w.get(leader).unwrap().position;
        // The leader keeps the 0.5 default: reach 2.3 m.
        assert!(w.in_use_reach(p + Vec3::new(2.2, 0.0, 0.0), leader), "{module}: leader not in reach at 2.2 m");
        assert!(!w.in_use_reach(p + Vec3::new(2.5, 0.0, 0.0), leader), "{module}: leader in reach at 2.5 m");
        // A living creature reaches REACH (1.8 m) plus its radius, at least 0.4 m.
        let living: Vec<_> = w
            .iter()
            .filter(|o| o.kind == ObjectKind::Creature && o.id != leader)
            .filter_map(|o| o.creature().filter(|c| !c.dead && !c.down).map(|c| (o.id, o.position, c.radius)))
            .collect();
        for (id, q, radius) in living {
            let reach = 1.8 + radius.max(0.4);
            floor_seen |= radius <= 0.4;
            assert!(w.in_use_reach(q + Vec3::new(reach - 0.05, 0.0, 0.0), id), "{module}: creature {id} not in reach at {:.2} m", reach - 0.05);
            assert!(!w.in_use_reach(q + Vec3::new(reach + 0.05, 0.0, 0.0), id), "{module}: creature {id} in reach at {:.2} m", reach + 0.05);
        }
    }
    assert!(floor_seen, "no living creature with a radius of 0.4 m or less in the sampled modules");
}

#[test]
fn a_dead_creature_that_is_no_container_has_the_default_reach() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    let leader = w.party_leader();
    let candidates: Vec<_> = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && o.id != leader && o.creature().is_some_and(|c| !c.dead && !c.down))
        .map(|o| o.id)
        .collect();
    let mut victim = None;
    for id in candidates {
        w.get_mut(id).unwrap().creature_mut().unwrap().dead = true;
        if !w.is_container(id) {
            victim = Some(id);
            break;
        }
    }
    let id = victim.expect("101per has no creature that dies without a loot window");
    let q = w.get(id).unwrap().position;
    // Dead and no container: the 0.5 m default, so reach 2.3 m.
    assert!(w.in_use_reach(q + Vec3::new(2.25, 0.0, 0.0), id), "a dead creature is not in reach at 2.25 m");
    assert!(!w.in_use_reach(q + Vec3::new(2.35, 0.0, 0.0), id), "a dead creature is in reach at 2.35 m");
}
