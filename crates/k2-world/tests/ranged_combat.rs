//! Ranged combat in a real module: a party leader armed with a rifle and a
//! blaster-armed hostile both attack at range (ranged AttackStarted with a
//! blaster clip), the rifle never turns into a melee attack, and no weapon
//! set swap happens during the fight. Skips (passes) when no install is found.

use std::io::Write;
use glam::Vec3;
use k2_res::Install;
use k2_script::ObjectId;
use k2_world::object::{Action, FollowTarget};
use k2_world::{ObjectKind, World, WorldEvent};

const DT: f32 = 1.0 / 30.0;
const RIFLE: &str = "w_brifle_04";
const RIGHT: i32 = 4;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(&dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP ranged_combat test: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    let mut w = World::new(install).unwrap_or_else(|e| panic!("{dir}: the install does not load into a world: {e:?}"));
    w.auto_dialog = true;
    w.enter_module("101per", None).unwrap();
    for _ in 0..60 {
        w.tick(DT);
        let _ = w.drain_events();
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
    }
    Some(w)
}

/// The nearest living, ranged-armed creature that is not in the party.
fn ranged_hostile(w: &mut World, from: Vec3) -> Option<ObjectId> {
    let ids: Vec<(f32, ObjectId)> = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && !w.party.contains(&o.id) && o.scripts.contains_key("ScriptOnNotice"))
        .filter(|o| o.creature().is_some_and(|c| !c.dead))
        .map(|o| (o.position.distance(from), o.id))
        .collect();
    let mut ids = ids;
    ids.sort_by(|a, b| a.0.total_cmp(&b.0));
    ids.into_iter().map(|(_, id)| id).find(|&id| w.combat_stats(id).is_some_and(|s| s.main.ranged))
}

#[test]
fn ranged_leader_and_ranged_hostile_fire_without_swapping_sets() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    for (slot, _) in w.equipment(leader) {
        if slot == RIGHT {
            w.unequip(leader, slot);
        }
    }
    let rifle = w.give_new_item(RIFLE, leader, 1, true).expect("rifle template");
    w.equip(leader, rifle, RIGHT).expect("equip rifle");
    assert!(w.combat_stats(leader).is_some_and(|s| s.main.ranged), "leader holds a ranged weapon");

    let from = w.get(leader).unwrap().position;
    let opponent = ranged_hostile(&mut w, from).expect("a ranged creature in 101per");
    w.get_mut(opponent).unwrap().creature_mut().unwrap().faction = 1;
    let (opos, obearing) = w.get(opponent).map(|o| (o.position, o.bearing)).unwrap();
    let spot = opos + Vec3::new(-obearing.sin(), obearing.cos(), 0.0) * 4.0;
    let ground = w.walk.ground(spot.x, spot.y, spot.z).map_or(spot, |h| Vec3::new(spot.x, spot.y, h.z));
    w.set_player_position(ground, 0.0);

    let (mut leader_ranged, mut leader_melee, mut opponent_ranged, mut swaps) = (0, 0, 0, 0);
    let mut anims = Vec::new();
    let mut t = 0.0;
    while t < 30.0 {
        if w.get(leader).unwrap().actions.is_empty() && w.get(opponent).and_then(|o| o.creature()).is_some_and(|c| !c.dead) {
            w.player_attack(opponent);
        }
        w.tick(DT);
        t += DT;
        for e in w.drain_events() {
            match e {
                WorldEvent::AttackStarted { attacker, anim, ranged, .. } if attacker == leader => {
                    if ranged {
                        leader_ranged += 1;
                    } else {
                        leader_melee += 1;
                    }
                    anims.push(anim);
                }
                WorldEvent::AttackStarted { attacker, ranged: true, .. } if attacker == opponent => opponent_ranged += 1,
                WorldEvent::WeaponSetSwapped { .. } => swaps += 1,
                _ => {}
            }
        }
    }
    assert!(leader_ranged >= 1, "the leader fires its rifle ({anims:?})");
    assert_eq!(leader_melee, 0, "the rifle never becomes a melee attack");
    assert!(anims.iter().all(|a| a.starts_with('b')), "ranged attacks use blaster clips: {anims:?}");
    assert!(opponent_ranged >= 1, "the hostile fires back at range");
    assert_eq!(swaps, 0, "no weapon set swap during the fight");
    assert_eq!(w.weapon_set(leader), 0);
}

/// Leader armed with the rifle, a ranged hostile placed 4 m away, and HK-47
/// joining beside the leader. `follow` queues the heartbeat ActionFollowLeader
/// the companion scripts put on idle companions.
fn companion_fight(follow: bool) -> Option<(World, ObjectId, ObjectId, ObjectId)> {
    let mut w = world()?;
    let leader = w.party_leader();
    for (slot, _) in w.equipment(leader) {
        if slot == RIGHT {
            w.unequip(leader, slot);
        }
    }
    let rifle = w.give_new_item(RIFLE, leader, 1, true).expect("rifle template");
    w.equip(leader, rifle, RIGHT).expect("equip rifle");

    let from = w.get(leader).unwrap().position;
    let opponent = ranged_hostile(&mut w, from).expect("a ranged creature in 101per");
    w.get_mut(opponent).unwrap().creature_mut().unwrap().faction = 1;
    let (opos, obearing) = w.get(opponent).map(|o| (o.position, o.bearing)).unwrap();
    let spot = opos + Vec3::new(-obearing.sin(), obearing.cos(), 0.0) * 4.0;
    let ground = w.walk.ground(spot.x, spot.y, spot.z).map_or(spot, |h| Vec3::new(spot.x, spot.y, h.z));
    w.set_player_position(ground, 0.0);

    let lp = w.get(leader).unwrap().position;
    let companion = w.spawn_creature("p_hk47", lp + Vec3::new(-2.0, 1.0, 0.0), 0.0).expect("HK-47");
    w.party.push(companion);
    if follow {
        w.push_action(companion, Action::Follow { of: FollowTarget::Leader, range: 2.5 });
    }
    Some((w, leader, opponent, companion))
}

#[test]
fn companion_engages_the_enemy_shooting_the_leader() {
    let Some((mut w, leader, opponent, companion)) = companion_fight(false) else { return };
    let mut companion_attacks = 0;
    let mut t = 0.0;
    while t < 30.0 {
        if w.get(leader).unwrap().actions.is_empty() && w.get(opponent).and_then(|o| o.creature()).is_some_and(|c| !c.dead) {
            w.player_attack(opponent);
        }
        w.tick(DT);
        t += DT;
        for e in w.drain_events() {
            if let WorldEvent::AttackStarted { attacker, target, .. } = e
                && attacker == companion
                && target == opponent
            {
                companion_attacks += 1;
            }
        }
    }
    assert!(companion_attacks >= 1, "the companion attacks the enemy shooting the leader");
}

#[test]
fn companion_holding_a_heartbeat_follow_still_engages() {
    // The companion scripts queue ActionFollowLeader on idle companions; a
    // standing follow must not keep the companion out of the fight.
    let Some((mut w, leader, opponent, companion)) = companion_fight(true) else { return };
    assert!(matches!(w.get(companion).unwrap().actions.front(), Some(Action::Follow { .. })), "the follow is queued");
    let mut companion_attacks = 0;
    let mut t = 0.0;
    while t < 30.0 {
        if w.get(leader).unwrap().actions.is_empty() && w.get(opponent).and_then(|o| o.creature()).is_some_and(|c| !c.dead) {
            w.player_attack(opponent);
        }
        w.tick(DT);
        t += DT;
        for e in w.drain_events() {
            if let WorldEvent::AttackStarted { attacker, target, .. } = e
                && attacker == companion
                && target == opponent
            {
                companion_attacks += 1;
            }
        }
    }
    assert!(companion_attacks >= 1, "a companion holding a follow attacks the enemy shooting the leader");
}

#[test]
fn party_member_notices_a_neutral_creature_attacking_the_leader() {
    // Perception: a party member notices whatever is attacking a party member,
    // even a creature that is not hostile to the party (the script then decides
    // with GetIsEnemy).
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    let lp = w.get(leader).unwrap().position;
    let companion = w.spawn_creature("p_hk47", lp + Vec3::new(-2.0, 1.0, 0.0), 0.0).expect("HK-47");
    w.party.push(companion);
    let mut near: Vec<(f32, ObjectId)> = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && o.id != leader && !w.party.contains(&o.id) && o.scripts.contains_key("ScriptOnNotice"))
        .filter(|o| o.creature().is_some_and(|c| !c.dead))
        .map(|o| (o.position.distance(lp), o.id))
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    let attacker = near.first().expect("a creature in 101per").1;
    w.get_mut(attacker).unwrap().creature_mut().unwrap().faction = 5;
    let ap = w.get(attacker).unwrap().position;
    w.place(companion, ap + Vec3::new(1.5, 0.0, 0.0));
    w.push_action(attacker, Action::Attack(leader));

    let mut noticed = false;
    for _ in 0..90 {
        w.tick(DT);
        let _ = w.drain_events();
        if w.get(companion).and_then(|o| o.creature()).is_some_and(|c| c.seen.contains(&attacker)) {
            noticed = true;
            break;
        }
    }
    assert!(noticed, "the companion notices the creature attacking the leader");
}
