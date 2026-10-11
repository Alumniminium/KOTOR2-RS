//! Sitters: each sit is queued once when the module spawns, the sitter stays
//! on its seat and faces the same way, and nothing re-queues the sit. Covers
//! the k_bar_spawn family (anim 37 and 38, including the k_bar_spawnp variants
//! whose conditional move to a waypoint is not taken in these modules) and the
//! 201tel traveller sitters (anim 36). Skips (passes) when no install
//! is found.
//!
//! The client plays sit-downs once and holds the seated pose (kotor2 game.rs,
//! scripted_playback); this test covers the simulation side of the loop.

use std::io::Write;
use k2_res::Install;
use k2_script::ObjectId;
use k2_world::{World, WorldEvent};

const DT: f32 = 1.0 / 30.0;

fn world(module: &str) -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(&dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP sitting test: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    let mut w = World::new(install).expect("World::new");
    w.enter_module(module, None).unwrap();
    Some(w)
}

/// Runs `seconds` of simulation, returning each sitter's sit events and its
/// (position, bearing) when the run started.
fn sit_run(w: &mut World, seats: &[ObjectId], seconds: f32) -> Vec<Vec<(i32, bool, f32)>> {
    let mut sits = vec![Vec::new(); seats.len()];
    let seen = |w: &mut World, sits: &mut Vec<Vec<(i32, bool, f32)>>| {
        for ev in w.drain_events() {
            if let WorldEvent::PlayAnimation { object, animation, looping, duration, .. } = ev
                && let Some(i) = seats.iter().position(|&s| s == object)
            {
                sits[i].push((animation, looping, duration));
            }
        }
    };
    seen(w, &mut sits);
    for _ in 0..(seconds / DT) as usize {
        w.tick(DT);
        seen(w, &mut sits);
    }
    sits
}

fn check_sitters(module: &str, tag_filter: &str) {
    let Some(mut w) = world(module) else { return };
    let sitters: Vec<ObjectId> = w.iter().filter(|o| o.kind == k2_world::ObjectKind::Creature && o.name.contains(tag_filter)).map(|o| o.id).collect();
    assert!(!sitters.is_empty(), "{module}: no {tag_filter} creatures");
    let start: Vec<_> = sitters.iter().map(|&id| w.get(id).map(|o| (o.position, o.bearing)).unwrap()).collect();
    let sits = sit_run(&mut w, &sitters, 30.0);
    for (i, &id) in sitters.iter().enumerate() {
        let o = w.get(id).unwrap();
        let (pos, bearing) = start[i];
        assert!(o.position.distance(pos) < 0.01, "{module}: {} moved from its seat {pos:?} to {:?}", o.name, o.position);
        assert!((o.bearing - bearing).abs() < 1e-4, "{module}: {} turned on its seat", o.name);
        assert!(o.actions.is_empty(), "{module}: {} still has {:?} queued", o.name, o.actions.front());
        assert_eq!(sits[i].len(), 1, "{module}: {} was sent {:?} in 30 s", o.name, sits[i]);
        let (anim, looping, duration) = sits[i][0];
        assert!(matches!(anim, 36..=38), "{module}: {} sits with animation {anim}", o.name);
        assert!(looping && duration <= 0.0, "{module}: {} sit is not the open-ended looping pose", o.name);
    }
}

#[test]
fn onderon_cantina_sitters_sit_once_and_stay_on_their_seats() {
    check_sitters("503ond", "Sitter");
}

#[test]
fn telos_cantina_sitters_sit_once_and_stay_on_their_seats() {
    check_sitters("207tel", "Sitt");
}

#[test]
fn taris_traveller_sitters_sit_once_and_stay_on_their_seats() {
    check_sitters("201tel", "Sit}");
}
