//! Party members knocked out in a fight get up once the battle is over. The
//! game's own hint (dialog.tlk 41883): "One of your party is down! Party
//! members who go down in combat will be revived at the end of the battle. If
//! your entire party goes down the game is over." Skips (passes) when no
//! install is found at K2_INSTALL or, by default, the game folder that holds
//! the repo (CARGO_MANIFEST_DIR/../../.., i.e. crates/k2-world up three levels).

use std::io::Write;
use glam::Vec3;
use k2_res::Install;
use k2_script::{ObjectId, OBJECT_INVALID};
use k2_world::{AvailableNpc, ObjectKind, World, WorldEvent};

const DT: f32 = 1.0 / 30.0;

fn install() -> Option<Install> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    Install::discover(dir).ok()
}

fn world(module: &str) -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(&dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP party_revival tests: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    let mut w = World::new(install).expect("a world on the install");
    w.auto_dialog = true;
    w.enter_module(module, None).unwrap_or_else(|e| panic!("entering {module}: {e:?}"));
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
        if let Some(p) = point_near(w, tp, dist) {
            w.set_player_position(p, 0.0);
            return Some(id);
        }
    }
    None
}

/// A walkable point `dist` m from `from` with a clear line of sight to it.
fn point_near(w: &mut World, from: Vec3, dist: f32) -> Option<Vec3> {
    for k in 0..36 {
        let ang = k as f32 * 10.0f32.to_radians();
        let (x, y) = (from.x + dist * ang.cos(), from.y + dist * ang.sin());
        let Some(h) = w.walk.ground(x, y, from.z) else { continue };
        let p = Vec3::new(x, y, h.z);
        if w.walk.line_of_sight(p + Vec3::Z * 1.5, from + Vec3::Z * 1.5) {
            return Some(p);
        }
    }
    None
}

/// Adds Kreia to the party (the second member in the field). Returns her id.
fn with_kreia(w: &mut World) -> ObjectId {
    w.available_npcs.insert(6, AvailableNpc { template: "p_kreia".into(), selectable: true, ..Default::default() });
    w.apply_party_selection(&[6]).unwrap();
    w.party.iter().copied().find(|&p| p != w.party_leader()).expect("Kreia joined the party")
}

fn toughen(w: &mut World, id: ObjectId) {
    let c = w.get_mut(id).and_then(|o| o.creature_mut()).unwrap();
    c.hp = 500;
    c.max_hp = 500;
}

fn hp(w: &World, id: ObjectId) -> i32 {
    w.get(id).and_then(|o| o.creature()).map_or(-1, |c| c.hp)
}

fn is_dead(w: &World, id: ObjectId) -> bool {
    w.get(id).and_then(|o| o.creature()).is_some_and(|c| c.dead)
}

/// One tick; the events it produced.
fn tick_once(w: &mut World) -> Vec<WorldEvent> {
    w.tick(DT);
    if w.conversation.is_some() {
        w.end_conversation(true);
    }
    w.drain_events()
}

/// Ticks `seconds`; the events seen.
fn run(w: &mut World, seconds: f32) -> Vec<WorldEvent> {
    let mut out = Vec::new();
    for _ in 0..(seconds / DT) as i32 {
        out.extend(tick_once(w));
    }
    out
}

/// Ticks until `id` is in combat (at most 20 s).
fn engage(w: &mut World, id: ObjectId) {
    let mut t = 0.0;
    while !w.in_combat(id) && t < 20.0 {
        tick_once(w);
        t += DT;
    }
    assert!(w.in_combat(id), "no fight started");
}

/// Every living hostile dies at once, killed by `by`.
fn kill_hostiles(w: &mut World, by: ObjectId) {
    let doomed: Vec<ObjectId> = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && o.id != by && o.creature().is_some_and(|c| !c.dead))
        .map(|o| o.id)
        .collect();
    for id in doomed {
        if w.is_enemy(id, by) {
            w.hurt_creature(id, 10_000, by);
        }
    }
    let _ = w.drain_events();
}

fn got_up(events: &[WorldEvent], who: ObjectId) -> bool {
    events.iter().any(|e| matches!(e, WorldEvent::GotUp { object } if *object == who))
}

fn planar_gap(w: &World, a: ObjectId, b: ObjectId) -> f32 {
    let pa = w.get(a).map(|o| o.position).unwrap_or(Vec3::ZERO);
    let pb = w.get(b).map(|o| o.position).unwrap_or(Vec3::ZERO);
    (pa - pb).truncate().length()
}

#[test]
fn a_companion_knocked_out_in_a_fight_gets_up_and_follows_again() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    let kreia = with_kreia(&mut w);
    let hostile = stand_near_hostile(&mut w, 6.0).expect("a hostile with open ground around it");
    toughen(&mut w, leader);
    engage(&mut w, leader);

    w.hurt_creature(kreia, 10_000, hostile);
    let ev = w.drain_events();
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::KnockedDown { object, .. } if *object == kreia)), "no KnockedDown: {ev:?}");
    assert!(!ev.iter().any(|e| matches!(e, WorldEvent::Died { object, .. } if *object == kreia)), "a companion died");
    assert!(w.is_down(kreia) && !is_dead(&w, kreia), "Kreia is not knocked out");
    assert_eq!(hp(&w, kreia), 0);
    assert_eq!(w.party_leader(), leader, "control moved although the leader stands");

    // The fight goes on with the leader while Kreia lies there. A knocked-out
    // companion takes no orders (a follow queued by its heartbeat or a script
    // must not walk the body).
    let lying = w.get(kreia).unwrap().position;
    w.push_action(kreia, k2_world::object::Action::Follow { of: k2_world::object::FollowTarget::Leader, range: 2.0 });
    let _ = run(&mut w, 2.0);
    assert!(w.is_down(kreia), "got up during the fight");
    assert_eq!(w.get(kreia).unwrap().position, lying, "a knocked-out companion walked");

    // The battle ends: every hostile dies.
    kill_hostiles(&mut w, leader);
    let mut got_up_at = None;
    let mut events = Vec::new();
    let mut t = 0.0;
    while t < 10.0 && got_up_at.is_none() {
        let ev = tick_once(&mut w);
        t += DT;
        if got_up(&ev, kreia) {
            got_up_at = Some(t);
        }
        events.extend(ev);
    }
    let got_up_at = got_up_at.expect("Kreia never got up after the battle");
    assert!(got_up_at <= 8.0, "got up only {got_up_at:.1} s after the battle ended");
    assert!(hp(&w, kreia) >= 1, "got up with {} hp", hp(&w, kreia));
    assert!(!is_dead(&w, kreia) && w.party.contains(&kreia));

    // Followers resume following: the leader walks off and Kreia comes along.
    // (Kreia may have fought on in another room; the leader steps to open
    // ground in sight of her, 8 m away.)
    let kp = w.get(kreia).unwrap().position;
    let spot = point_near(&mut w, kp, 8.0).expect("open ground 8 m from Kreia");
    w.set_player_position(spot, 0.0);
    let mut close = false;
    let mut t = 0.0;
    while t < 15.0 && !close {
        let _ = tick_once(&mut w);
        t += DT;
        close = planar_gap(&w, kreia, leader) <= 3.0;
    }
    assert!(close, "Kreia did not follow: {:.1} m away", planar_gap(&w, kreia, leader));
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

#[test]
fn a_fallen_leader_hands_control_to_a_companion_and_gets_up_later() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    let kreia = with_kreia(&mut w);
    let hostile = stand_near_hostile(&mut w, 6.0).expect("a hostile with open ground around it");
    engage(&mut w, leader);

    w.hurt_creature(leader, 10_000, hostile);
    let ev = w.drain_events();
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::LeaderChanged { old, new } if *old == leader && *new == kreia)), "no hand-over: {ev:?}");
    assert!(!ev.iter().any(|e| matches!(e, WorldEvent::PartyDefeated)), "the party is defeated with Kreia standing");
    assert_eq!(w.party_leader(), kreia);
    assert!(w.is_down(leader) && !is_dead(&w, leader));
    assert!(w.is_standing(kreia));

    // The battle ends (Kreia's side wins); the old leader gets up in the
    // party and stays in it.
    kill_hostiles(&mut w, kreia);
    let mut t = 0.0;
    let mut up = false;
    while t < 10.0 && !up {
        let ev = tick_once(&mut w);
        t += DT;
        up = got_up(&ev, leader);
    }
    assert!(up, "the fallen leader never got up");
    assert!(hp(&w, leader) >= 1 && !is_dead(&w, leader));
    assert!(w.party.contains(&leader));
    assert_eq!(w.party_leader(), kreia, "control came back without a choice");
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

#[test]
fn the_whole_party_down_is_game_over_and_nobody_gets_up() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    let kreia = with_kreia(&mut w);
    let hostile = stand_near_hostile(&mut w, 6.0).expect("a hostile with open ground around it");
    engage(&mut w, leader);

    w.hurt_creature(kreia, 10_000, hostile);
    let ev = w.drain_events();
    assert!(!ev.iter().any(|e| matches!(e, WorldEvent::PartyDefeated)), "defeated with the leader standing");
    w.hurt_creature(leader, 10_000, hostile);
    let ev = w.drain_events();
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::PartyDefeated)), "no PartyDefeated: {ev:?}");
    assert!(!ev.iter().any(|e| matches!(e, WorldEvent::LeaderChanged { .. })), "control moved to a fallen member");
    assert!(w.party_defeated());
    assert_eq!(w.save_blocked(), Some("the party is dead"));

    // The hostile is still alive and the battle goes on for a while: neither
    // member gets up, and neither is dead.
    let ev = run(&mut w, 15.0);
    assert!(!got_up(&ev, leader) && !got_up(&ev, kreia), "a member got up in a defeated party");
    for id in [leader, kreia] {
        assert!(w.is_down(id) && !is_dead(&w, id), "{id} is not knocked out");
    }
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

#[test]
fn a_save_with_a_fallen_companion_gets_her_up_after_load() {
    // Saves from before this rule stored a fallen party member as dead at
    // 0 HP (HK-47 in the player's saves). They load knocked out and get up
    // once no battle is on.
    let Some(mut w) = world("102per") else { return };
    let kreia = with_kreia(&mut w);
    {
        let c = w.get_mut(kreia).and_then(|o| o.creature_mut()).unwrap();
        c.hp = 0;
        c.dead = true;
    }
    let data = w.save();
    let mut loaded = World::load(install().unwrap(), &data).expect("load");
    assert!(loaded.is_down(kreia) && !is_dead(&loaded, kreia), "a stored 0 HP companion loaded as dead");
    let ev = run(&mut loaded, 8.0);
    assert!(got_up(&ev, kreia), "the fallen companion never got up after load");
    assert!(hp(&loaded, kreia) >= 1 && loaded.party.contains(&kreia));
    assert!(loaded.vm_errors.is_empty(), "{:?}", loaded.vm_errors);
}

/// Pins the push_action guard: a knocked-out creature takes no orders (its
/// queue was cleared at the knock-down), while a standing one does. Dropping
/// the guard from push_action fails this.
#[test]
fn a_knocked_out_creature_takes_no_orders() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    let kreia = with_kreia(&mut w);
    let order = || k2_world::object::Action::Wait { remaining: 1.0 };
    w.get_mut(kreia).unwrap().actions.clear();
    w.push_action(kreia, order());
    assert_eq!(w.get(kreia).unwrap().actions.len(), 1, "a standing companion refused an order");
    w.hurt_creature(kreia, 10_000, leader);
    assert!(w.is_down(kreia));
    assert!(w.get(kreia).unwrap().actions.is_empty(), "the knock-down kept the queue");
    w.push_action(kreia, order());
    assert!(w.get(kreia).unwrap().actions.is_empty(), "a knocked-out companion took an order");
}

#[test]
fn a_companion_slain_outright_stays_dead_across_a_save() {
    let Some(mut w) = world("102per") else { return };
    let kreia = with_kreia(&mut w);
    {
        let c = w.get_mut(kreia).and_then(|o| o.creature_mut()).unwrap();
        c.hp = 0;
        c.dead = true;
        c.slain = true;
    }
    let data = w.save();
    let mut loaded = World::load(install().unwrap(), &data).expect("load");
    let ev = run(&mut loaded, 8.0);
    assert!(is_dead(&loaded, kreia) && !loaded.is_down(kreia), "a slain companion came back");
    assert!(!got_up(&ev, kreia));
}

#[test]
fn effect_death_kills_a_knocked_out_companion_for_good() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    let kreia = with_kreia(&mut w);
    w.hurt_creature(kreia, 10_000, OBJECT_INVALID);
    assert!(w.is_down(kreia));
    w.slay_creature(kreia, leader);
    let ev = w.drain_events();
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::Died { object, .. } if *object == kreia)), "no Died: {ev:?}");
    let c = w.get(kreia).and_then(|o| o.creature()).unwrap();
    assert!(c.dead && c.slain && !c.down, "dead {} slain {} down {}", c.dead, c.slain, c.down);
    let ev = run(&mut w, 10.0);
    assert!(!got_up(&ev, kreia), "a slain companion got up");
    assert!(is_dead(&w, kreia));
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

/// Pins the regeneration guard (regen.rs run_regen): a downed companion stays at 0 HP
/// while it is held down. The get-up timer is pushed out so only regen could
/// change the HP.
#[test]
fn a_downed_companion_does_not_regenerate() {
    let Some(mut w) = world("102per") else { return };
    let kreia = with_kreia(&mut w);
    w.hurt_creature(kreia, 10_000, OBJECT_INVALID);
    assert!(w.is_down(kreia));
    w.get_mut(kreia).and_then(|o| o.creature_mut()).unwrap().get_up_in = 1.0e6;
    let _ = run(&mut w, 12.0);
    assert!(w.is_down(kreia), "a downed companion got up");
    assert_eq!(hp(&w, kreia), 0, "a downed companion regenerated");
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

/// EffectResurrection(percent) and EffectHeal as the scripts use them: the
/// Revitalize family resurrects with no HP and heals a fixed amount; 0 HP
/// does not stand a companion up, a heal above 0 does.
#[test]
fn resurrection_and_healing_of_a_companion_follow_the_script_amounts() {
    let Some(mut w) = world("102per") else { return };
    let kreia = with_kreia(&mut w);
    let max = w.get(kreia).and_then(|o| o.creature()).unwrap().max_hp;
    assert!(max >= 10, "Kreia has {max} max hp");
    // Revitalize: EffectResurrection() then EffectHeal(10).
    w.hurt_creature(kreia, 10_000, OBJECT_INVALID);
    w.resurrect_creature(kreia, 0);
    assert!(w.is_down(kreia) && hp(&w, kreia) == 0, "resurrect(0) stood her up");
    let ev = w.drain_events();
    assert!(!got_up(&ev, kreia), "GotUp before any heal");
    w.heal_creature(kreia, 10);
    let ev = w.drain_events();
    assert!(got_up(&ev, kreia), "no GotUp after the heal");
    assert!(!w.is_down(kreia) && hp(&w, kreia) == 10, "hp {} after resurrect(0) and heal 10", hp(&w, kreia));
    // Resurrect then heal 1 (UT_RestorePartyToOneHealth shape): 1 HP.
    w.hurt_creature(kreia, 10_000, OBJECT_INVALID);
    w.resurrect_creature(kreia, 0);
    w.heal_creature(kreia, 1);
    assert!(!w.is_down(kreia) && hp(&w, kreia) == 1, "hp {} after resurrect(0) and heal 1", hp(&w, kreia));
    // A percentage stands her up with that share of max HP (at least 1).
    w.hurt_creature(kreia, 10_000, OBJECT_INVALID);
    w.resurrect_creature(kreia, 10);
    let want = (max * 10 / 100).max(1);
    assert!(!w.is_down(kreia) && hp(&w, kreia) == want, "10% gave {} hp, want {want}", hp(&w, kreia));
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

/// Atton joins as the blueprint copy (p_atton.utc: a_attonspirit as ScriptDeath,
/// k_hen_* scripts) with the Spirit feat, and the leader fights a hostile. The
/// module's placed Atton carries default k_def_* scripts, so the fixture
/// removes him. Returns (leader, Atton, hostile).
fn atton_in_a_fight(w: &mut World) -> (ObjectId, ObjectId, ObjectId) {
    let leader = w.party_leader();
    let placed: Vec<ObjectId> = w.iter().filter(|o| o.tag.eq_ignore_ascii_case("atton")).map(|o| o.id).collect();
    for id in placed {
        w.destroy(id);
    }
    w.available_npcs.insert(0, AvailableNpc { template: "p_atton".into(), selectable: true, ..Default::default() });
    w.apply_party_selection(&[0]).unwrap();
    let atton = w.party.iter().copied().find(|&p| p != leader).expect("Atton joined");
    assert_eq!(w.iter().filter(|o| o.tag.eq_ignore_ascii_case("atton")).count(), 1, "two Attons in the area");
    assert_eq!(w.get(atton).unwrap().scripts.get("ScriptDeath").map(String::as_str), Some("a_attonspirit"));
    let spirit = w.constant("FEAT_SPIRIT").expect("FEAT_SPIRIT") as u16;
    w.grant_feat(atton, spirit);
    let hostile = stand_near_hostile(w, 6.0).expect("a hostile with open ground around it");
    toughen(w, leader);
    engage(w, leader);
    (leader, atton, hostile)
}

/// Atton's Spirit feat (a_attonspirit.nss, ScriptDeath): with the feat, a
/// knocked-out Atton is revived 8 s later with 10% of his max HP while another
/// standing member is still in combat.
#[test]
fn atton_spirit_gets_him_up_about_eight_seconds_later_at_ten_percent() {
    let Some(mut w) = world("102per") else { return };
    let (leader, atton, hostile) = atton_in_a_fight(&mut w);
    let max = w.get(atton).and_then(|o| o.creature()).unwrap().max_hp;
    w.hurt_creature(atton, 10_000, hostile);
    assert!(w.is_down(atton));
    // The leader keeps fighting, so the battle-end revive cannot be the one.
    let mut t = 0.0;
    let mut up_at = None;
    let mut hp_up = 0;
    let mut leader_fighting = true;
    while t < 14.0 && up_at.is_none() {
        let ev = tick_once(&mut w);
        t += DT;
        if got_up(&ev, atton) {
            up_at = Some(t);
            hp_up = hp(&w, atton);
            leader_fighting = w.in_combat(leader);
        }
    }
    let up_at = up_at.expect("Atton never got up with the Spirit feat");
    assert!((7.0..=9.5).contains(&up_at), "Atton got up {up_at:.1} s after the knock-down");
    assert!(leader_fighting, "the leader was out of combat when Atton got up");
    let want = (max * 10 / 100).max(1);
    assert_eq!(hp_up, want, "Spirit gives 10% of max HP ({max} max)");
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

/// EffectDeath is final: a standing Atton killed outright gets Died, and his
/// Spirit revive (8 s later, a_attonspirit DoRevive -> EffectResurrection) does
/// not bring him back.
#[test]
fn effect_death_on_a_standing_atton_is_final() {
    let Some(mut w) = world("102per") else { return };
    let (leader, atton, _hostile) = atton_in_a_fight(&mut w);
    w.slay_creature(atton, leader);
    let ev = w.drain_events();
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::Died { object, .. } if *object == atton)), "no Died: {ev:?}");
    let ev = run(&mut w, 14.0);
    assert!(!got_up(&ev, atton), "a slain Atton got up");
    let c = w.get(atton).and_then(|o| o.creature()).unwrap();
    assert!(c.dead && c.slain && !c.down && c.hp == 0, "dead {} slain {} down {} hp {}", c.dead, c.slain, c.down, c.hp);
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

/// EffectDeath on a knocked-out Atton: the Spirit revive that was queued at the
/// knock-down (8 s) must not bring him back either.
#[test]
fn effect_death_on_a_downed_atton_is_final() {
    let Some(mut w) = world("102per") else { return };
    let (leader, atton, hostile) = atton_in_a_fight(&mut w);
    w.hurt_creature(atton, 10_000, hostile);
    assert!(w.is_down(atton));
    let _ = run(&mut w, 2.0);
    w.slay_creature(atton, leader);
    let ev = w.drain_events();
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::Died { object, .. } if *object == atton)), "no Died: {ev:?}");
    assert!(!ev.iter().any(|e| matches!(e, WorldEvent::LeaderChanged { .. } | WorldEvent::PartyDefeated)), "the standing leader was handed over again: {ev:?}");
    let ev = run(&mut w, 12.0);
    assert!(!got_up(&ev, atton), "a slain Atton got up");
    let c = w.get(atton).and_then(|o| o.creature()).unwrap();
    assert!(c.dead && c.slain && !c.down, "dead {} slain {} down {}", c.dead, c.slain, c.down);
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

/// The last member going down defeats the party once; killing that downed
/// member afterwards does not report the defeat again.
#[test]
fn killing_a_downed_member_of_a_defeated_party_does_not_defeat_twice() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    let kreia = with_kreia(&mut w);
    let hostile = stand_near_hostile(&mut w, 6.0).expect("a hostile with open ground around it");
    engage(&mut w, leader);
    w.hurt_creature(kreia, 10_000, hostile);
    w.hurt_creature(leader, 10_000, hostile);
    let ev = w.drain_events();
    assert_eq!(ev.iter().filter(|e| matches!(e, WorldEvent::PartyDefeated)).count(), 1, "defeat not reported once: {ev:?}");
    w.slay_creature(kreia, hostile);
    let ev = w.drain_events();
    assert!(!ev.iter().any(|e| matches!(e, WorldEvent::PartyDefeated)), "defeat reported again: {ev:?}");
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::Died { object, .. } if *object == kreia)), "no Died: {ev:?}");
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

/// EffectDeath on a knocked-out min1hp creature does not stand it up: min1hp
/// holds a standing creature at 1 HP and does not revive a fallen one.
#[test]
fn effect_death_leaves_a_downed_min1hp_companion_down() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    let kreia = with_kreia(&mut w);
    w.hurt_creature(kreia, 10_000, OBJECT_INVALID);
    assert!(w.is_down(kreia));
    w.get_mut(kreia).and_then(|o| o.creature_mut()).unwrap().min1hp = true;
    w.slay_creature(kreia, leader);
    let ev = w.drain_events();
    assert!(!got_up(&ev, kreia), "EffectDeath stood a downed min1hp companion up");
    let c = w.get(kreia).and_then(|o| o.creature()).unwrap();
    assert!(c.down && !c.dead && c.hp == 0, "down {} dead {} hp {}", c.down, c.dead, c.hp);
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

/// A raise with the leader still down hands control to the raised member
/// (the same hand-over as settle_loaded_party and party_member_down).
#[test]
fn a_member_raised_while_the_leader_is_down_takes_control() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    let kreia = with_kreia(&mut w);
    w.hurt_creature(kreia, 10_000, OBJECT_INVALID);
    assert!(w.is_down(kreia));
    w.hurt_creature(leader, 10_000, OBJECT_INVALID);
    let ev = w.drain_events();
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::PartyDefeated)), "no defeat: {ev:?}");
    assert!(w.party_defeated());
    assert_eq!(w.party_leader(), leader, "control moved with nobody standing");
    // A heal raises Kreia while the leader still lies there: she takes control.
    w.heal_creature(kreia, 10);
    let ev = w.drain_events();
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::LeaderChanged { old, new } if *old == leader && *new == kreia)), "no hand-over: {ev:?}");
    assert_eq!(w.party_leader(), kreia);
    assert!(w.is_standing(kreia) && w.is_down(leader));
    assert!(!w.party_defeated());
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

/// min1hp and plot only hold a standing creature at 1 HP: a companion that
/// fell with the flags set up after the knock-down gets up at the battle end
/// like everyone else, at 1 HP.
#[test]
fn a_downed_min1hp_or_plot_companion_gets_up_at_the_battle_end() {
    let Some(mut w) = world("102per") else { return };
    let kreia = with_kreia(&mut w);
    w.hurt_creature(kreia, 10_000, OBJECT_INVALID);
    assert!(w.is_down(kreia));
    {
        let o = w.get_mut(kreia).unwrap();
        o.plot = true;
        o.creature_mut().unwrap().min1hp = true;
    }
    // Read the HP on the tick of the get-up: regeneration adds HP afterwards.
    let mut hp_up = None;
    let mut t = 0.0;
    while t < 8.0 && hp_up.is_none() {
        let ev = tick_once(&mut w);
        t += DT;
        if got_up(&ev, kreia) {
            hp_up = Some(hp(&w, kreia));
        }
    }
    assert_eq!(hp_up, Some(1), "a downed min1hp or plot companion did not get up at 1 HP at the battle end");
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

/// A standing min1hp or plot companion stays at 1 HP under a non-lethal hit and
/// under EffectDeath: neither flag lets it die or fall.
#[test]
fn a_standing_min1hp_or_plot_companion_stays_at_one_hp() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    let kreia = with_kreia(&mut w);
    w.get_mut(kreia).unwrap().creature_mut().unwrap().min1hp = true;
    w.hurt_creature(kreia, 10_000, OBJECT_INVALID);
    assert!(!w.is_down(kreia) && !is_dead(&w, kreia) && hp(&w, kreia) == 1, "min1hp: hp {}", hp(&w, kreia));
    w.slay_creature(kreia, leader);
    assert!(!w.is_down(kreia) && !is_dead(&w, kreia) && hp(&w, kreia) == 1, "min1hp under EffectDeath: hp {}", hp(&w, kreia));
    {
        let o = w.get_mut(kreia).unwrap();
        o.plot = true;
        o.creature_mut().unwrap().min1hp = false;
    }
    w.hurt_creature(kreia, 10_000, OBJECT_INVALID);
    assert!(!w.is_down(kreia) && !is_dead(&w, kreia) && hp(&w, kreia) == 1, "plot: hp {}", hp(&w, kreia));
    w.slay_creature(kreia, leader);
    assert!(!w.is_down(kreia) && !is_dead(&w, kreia) && hp(&w, kreia) == 1, "plot under EffectDeath: hp {}", hp(&w, kreia));
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

/// A heal does not raise a dead creature: EffectHeal on a slain companion
/// leaves it dead at 0 HP.
#[test]
fn heal_does_not_raise_a_dead_companion() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    let kreia = with_kreia(&mut w);
    w.slay_creature(kreia, leader);
    w.heal_creature(kreia, 10);
    let c = w.get(kreia).and_then(|o| o.creature()).unwrap();
    assert!(c.dead && c.hp == 0, "dead {} hp {}", c.dead, c.hp);
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}
