//! Hostile creatures start fights on their own. Skips (passes) when no
//! install is found at K2_INSTALL or ../.. (the repo inside the game folder).

use std::io::Write;
use glam::Vec3;
use k2_res::Install;
use k2_script::ObjectId;
use k2_world::{ObjectKind, World, WorldEvent};

const DT: f32 = 1.0 / 30.0;

fn world(module: &str) -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(&dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP hostiles test: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    let mut w = World::new(install).unwrap_or_else(|e| panic!("{dir}: the install does not load into a world: {e:?}"));
    w.auto_dialog = true;
    w.enter_module(module, None).unwrap_or_else(|e| panic!("{module}: enter_module: {e:?}"));
    // Let the spawn scripts and the first heartbeats settle.
    for _ in 0..90 {
        w.tick(DT);
        let _ = w.drain_events();
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
    }
    Some(w)
}

/// Puts the idle leader `dist` m from the nearest hostile creature that has
/// perception, on open ground with a clear line of sight. Returns the hostile.
fn stand_near_hostile(w: &mut World, dist: f32) -> Option<ObjectId> {
    let leader = w.party_leader();
    let from = w.get(leader)?.position;
    let mut hostiles: Vec<(f32, ObjectId, Vec3)> = Vec::new();
    let ids: Vec<ObjectId> = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && o.scripts.contains_key("ScriptOnNotice") && o.id != leader)
        .map(|o| o.id)
        .collect();
    for id in ids {
        if w.is_enemy(id, leader) && w.get(id).and_then(|o| o.creature()).is_some_and(|c| !c.dead) {
            let p = w.get(id)?.position;
            hostiles.push((p.distance(from), id, p));
        }
    }
    hostiles.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, id, tp) in hostiles {
        for k in 0..36 {
            let ang = k as f32 * 10.0f32.to_radians();
            let (x, y) = (tp.x + dist * ang.cos(), tp.y + dist * ang.sin());
            let Some(h) = w.walk.ground(x, y, tp.z) else { continue };
            let p = Vec3::new(x, y, h.z);
            if w.walk.line_of_sight(p + Vec3::Z * 1.5, tp + Vec3::Z * 1.5) {
                w.set_player_position(p, 0.0);
                return Some(id);
            }
        }
    }
    None
}

/// Ticks `seconds`; returns when the leader was first damaged and by whom.
fn first_damage_on_leader(w: &mut World, seconds: f32) -> Option<(f32, ObjectId)> {
    let leader = w.party_leader();
    let mut t = 0.0;
    while t < seconds {
        w.tick(DT);
        t += DT;
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
        for e in w.drain_events() {
            if let WorldEvent::Damaged { object, by, .. } = e
                && object == leader
            {
                return Some((t, by));
            }
        }
    }
    None
}

#[test]
fn awake_hostile_in_sight_attacks_the_party() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    let hostile = stand_near_hostile(&mut w, 10.0).expect("a hostile with open ground around it");
    let hp = w.get(leader).and_then(|o| o.creature()).map(|c| c.hp).unwrap();
    let (t, by) = first_damage_on_leader(&mut w, 20.0).expect("the party was never attacked");
    assert!(t < 15.0, "first hit only after {t:.1} s");
    assert!(w.is_enemy(by, leader), "damage came from a non-hostile");
    assert!(w.get(leader).and_then(|o| o.creature()).unwrap().hp < hp);
    let _ = hostile;
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

#[test]
fn hostile_that_already_noticed_the_party_still_attacks() {
    // A save made mid-reaction (or a notice whose combat round found no
    // target) leaves the leader in the hostile's seen set while it idles.
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    let hostile = stand_near_hostile(&mut w, 10.0).expect("a hostile with open ground around it");
    w.get_mut(hostile).and_then(|o| o.creature_mut()).unwrap().seen.insert(leader);
    let (t, _) = first_damage_on_leader(&mut w, 25.0).expect("an idle hostile never re-engaged");
    assert!(t < 20.0, "first hit only after {t:.1} s");
}

#[test]
fn the_only_party_member_can_fall_and_the_party_is_defeated() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    stand_near_hostile(&mut w, 6.0).expect("a hostile with open ground around it");
    {
        let c = w.get_mut(leader).and_then(|o| o.creature_mut()).unwrap();
        c.hp = 3;
        c.min1hp = false;
    }
    let mut down = false;
    let mut defeated = false;
    for _ in 0..(40.0 / DT) as i32 {
        w.tick(DT);
        for e in w.drain_events() {
            match e {
                WorldEvent::KnockedDown { object, .. } if object == leader => down = true,
                WorldEvent::PartyDefeated => defeated = true,
                _ => {}
            }
        }
        if defeated {
            break;
        }
    }
    assert!(down, "the leader survived 40 s next to hostiles");
    assert!(defeated, "no PartyDefeated after the only party member fell");
    // Knocked out, not dead, and nobody is left to get up before the battle
    // ends: the world keeps ticking over the body without errors.
    for _ in 0..(5.0 / DT) as i32 {
        w.tick(DT);
    }
    let _ = w.drain_events();
    let c = w.get(leader).and_then(|o| o.creature()).unwrap();
    assert!(c.down && !c.dead, "down {} dead {}", c.down, c.dead);
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

#[test]
fn combat_ends_a_few_seconds_after_the_last_hostile_dies() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    w.available_npcs.insert(6, k2_world::AvailableNpc { template: "p_kreia".into(), selectable: true, ..Default::default() });
    w.apply_party_selection(&[6]).unwrap();
    stand_near_hostile(&mut w, 6.0).expect("a hostile with open ground around it");
    {
        let c = w.get_mut(leader).and_then(|o| o.creature_mut()).unwrap();
        c.hp = 500;
        c.max_hp = 500;
    }
    // The leader fights until a hostile has engaged it.
    let mut t = 0.0;
    while !w.in_combat(leader) && t < 20.0 {
        w.tick(DT);
        t += DT;
    }
    assert!(w.in_combat(leader), "no fight started");
    // Everything hostile dies at once.
    let doomed: Vec<ObjectId> = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && o.id != leader && o.creature().is_some_and(|c| !c.dead))
        .map(|o| o.id)
        .collect();
    for id in doomed {
        if w.is_enemy(id, leader) {
            w.hurt_creature(id, 10_000, leader);
        }
    }
    let _ = w.drain_events();
    let mut left_at = None;
    for _ in 0..(10.0 / DT) as i32 {
        w.tick(DT);
        t += DT;
        if left_at.is_none() && w.party.iter().all(|&p| !w.in_combat(p)) {
            left_at = Some(t);
        }
    }
    let killed_at = t - 10.0;
    let left_at = left_at.expect("party still in combat 10 s after the last hostile died");
    assert!(left_at - killed_at <= 4.0, "combat ended only {:.1} s after the last kill", left_at - killed_at);
    assert!(w.party.iter().all(|&p| !w.in_combat(p)), "combat came back");
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

#[test]
fn mining_droid_hide_penalties_apply() {
    // The creature hide (g_i_crhide015) takes 5 off Dex and 5 off attacks.
    let Some(mut w) = world("101per") else { return };
    let pos = w.get(w.party_leader()).unwrap().position;
    let id = w.spawn_creature("g_assassindrd002", pos, 0.0).unwrap();
    let s = w.combat_stats(id).unwrap();
    assert_eq!(s.abilities[1], 3, "Dex 8 - 5");
    assert!(s.attack_bonus(false) <= -5, "droid attack bonus {}", s.attack_bonus(false));
    // 10 + Dex -4 + ExpertDroid class Defense 2 (SCD at level 1) = 8.
    assert_eq!(s.dex_mod(), -4);
    assert_eq!(s.defense, 10 + s.dex_mod() + 2, "droid defense {}", s.defense);
    assert_eq!(s.defense, 8);
    assert!(!s.main.lightsaber, "a blaster is not a lightsaber");
}
