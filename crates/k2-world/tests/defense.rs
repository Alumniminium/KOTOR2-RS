//! Class, droid and deflection Defense against the real install, and blaster
//! deflection in a fight. Skips (passes) when no install is found.

use glam::Vec3;
use k2_res::Install;
use k2_script::ObjectId;
use k2_world::{World, WorldEvent};

const DT: f32 = 1.0 / 30.0;
const RIGHT: i32 = 4;

const SOLDIER: i32 = 0;
const JEDI_GUARDIAN: i32 = 3;
const COMBAT_DROID: i32 = 6;
const EXPERT_DROID: i32 = 7;

const FEAT_LOGIC_UPGRADE_COMBAT: u16 = 110;
const FEAT_LOGIC_UPGRADE_TACTICIAN: u16 = 111;
const FEAT_LOGIC_UPGRADE_BATTLE_DROID: u16 = 112;
const FEAT_JEDI_DEFENSE: u16 = 55;
const FEAT_ADVANCED_JEDI_DEFENSE: u16 = 1;
const FEAT_MASTER_JEDI_DEFENSE: u16 = 24;
const FEAT_DEFLECT: u16 = 168;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let mut w = World::new(Install::discover(dir).ok()?).ok()?;
    w.enter_module("101per", None).unwrap();
    for _ in 0..30 {
        w.tick(DT);
    }
    Some(w)
}

fn set_classes(w: &mut World, pc: ObjectId, classes: Vec<(i32, i32)>) {
    w.get_mut(pc).and_then(|o| o.creature_mut()).expect("pc creature").classes = classes;
}

fn defense(w: &mut World, pc: ObjectId) -> i32 {
    w.combat_stats(pc).expect("pc stats").defense
}

fn deflection(w: &mut World, pc: ObjectId) -> Option<i32> {
    w.combat_stats(pc).expect("pc stats").deflection
}

#[test]
fn class_defense_follows_the_acbonus_column_of_the_class() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    // acbonus.2da, sol column: rows 4, 5, 10, 11 (levels 5, 6, 11, 12) = 0, 2, 2, 4.
    set_classes(&mut w, pc, vec![(SOLDIER, 1)]);
    let base = defense(&mut w, pc);
    let at = |w: &mut World, level: i32| {
        set_classes(w, pc, vec![(SOLDIER, level)]);
        defense(w, pc)
    };
    assert_eq!(at(&mut w, 5) - base, 0);
    assert_eq!(at(&mut w, 6) - base, 2);
    assert_eq!(at(&mut w, 11) - base, 2);
    assert_eq!(at(&mut w, 12) - base, 4);
    // Jedi Guardian, jdg column row 0 = 2 at level 1.
    set_classes(&mut w, pc, vec![(JEDI_GUARDIAN, 1)]);
    assert_eq!(defense(&mut w, pc) - base, 2);
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

#[test]
fn non_droid_combat_logic_is_two_and_two_more_every_six_levels() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    set_classes(&mut w, pc, vec![(SOLDIER, 1)]);
    let without_1 = defense(&mut w, pc);
    set_classes(&mut w, pc, vec![(SOLDIER, 6)]);
    let without_6 = defense(&mut w, pc);
    w.grant_feat(pc, FEAT_LOGIC_UPGRADE_COMBAT);
    set_classes(&mut w, pc, vec![(SOLDIER, 1)]);
    assert_eq!(defense(&mut w, pc) - without_1, 2, "+2 at level 1");
    set_classes(&mut w, pc, vec![(SOLDIER, 6)]);
    assert_eq!(defense(&mut w, pc) - without_6, 2 + 2, "+4 at level 6");
}

#[test]
fn droid_classes_count_the_scd_ladder_once() {
    // CombatDroid and ExpertDroid take the SCD column of acbonus.2da, the
    // same ladder as feat 110, so the feat adds nothing on top of the class.
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    set_classes(&mut w, pc, vec![(COMBAT_DROID, 1)]);
    let plain_1 = defense(&mut w, pc);
    set_classes(&mut w, pc, vec![(COMBAT_DROID, 6)]);
    let plain_6 = defense(&mut w, pc);
    w.grant_feat(pc, FEAT_LOGIC_UPGRADE_COMBAT);
    assert_eq!(defense(&mut w, pc), plain_6, "feat 110 at level 6 equals the SCD term (+4)");
    set_classes(&mut w, pc, vec![(COMBAT_DROID, 1)]);
    assert_eq!(defense(&mut w, pc), plain_1, "feat 110 at level 1 equals the SCD term (+2)");
    set_classes(&mut w, pc, vec![(EXPERT_DROID, 1)]);
    assert_eq!(defense(&mut w, pc), plain_1, "ExpertDroid too");
    // The larger of the two applies: 112 (+6) over the class term (+2), not 8.
    w.grant_feat(pc, FEAT_LOGIC_UPGRADE_BATTLE_DROID);
    assert_eq!(defense(&mut w, pc) - plain_1, 6 - 2, "max(+2 class, +6 logic)");
}

#[test]
fn non_droid_combat_logic_feats_replace_the_one_below_at_level_twelve() {
    // feat.2da successors 110 -> 111 -> 112; TLK 38673-38675. Soldier, so the
    // droid class term is not in play. 110 is the ladder (+6 at 12).
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    set_classes(&mut w, pc, vec![(SOLDIER, 12)]);
    let base = defense(&mut w, pc);
    w.grant_feat(pc, FEAT_LOGIC_UPGRADE_COMBAT);
    assert_eq!(defense(&mut w, pc) - base, 6, "110 ladder at level 12");
    w.grant_feat(pc, FEAT_LOGIC_UPGRADE_TACTICIAN);
    assert_eq!(defense(&mut w, pc) - base, 4, "111 replaces 110: flat +4");
    w.grant_feat(pc, FEAT_LOGIC_UPGRADE_BATTLE_DROID);
    assert_eq!(defense(&mut w, pc) - base, 6, "112 replaces 111: flat +6");
}

#[test]
fn tactician_and_battle_droid_are_flat_from_level_one() {
    // Owning 111 or 112 at level 1 gives +4 / +6, not the 110 ladder's +2.
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    set_classes(&mut w, pc, vec![(SOLDIER, 1)]);
    let base = defense(&mut w, pc);
    w.grant_feat(pc, FEAT_LOGIC_UPGRADE_TACTICIAN);
    assert_eq!(defense(&mut w, pc) - base, 4, "111 at level 1");
    w.grant_feat(pc, FEAT_LOGIC_UPGRADE_BATTLE_DROID);
    assert_eq!(defense(&mut w, pc) - base, 6, "112 at level 1");
}

#[test]
fn jedi_defense_deflects_only_with_a_lightsaber_and_master_wins() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    set_classes(&mut w, pc, vec![(JEDI_GUARDIAN, 1)]);
    w.grant_feat(pc, FEAT_JEDI_DEFENSE);
    let blade = w.create_item("g_w_lghtsbr01", pc, 1).expect("lightsaber");
    w.unequip(pc, RIGHT);
    assert_eq!(deflection(&mut w, pc), None, "no lightsaber, no deflection");
    w.equip(pc, blade, RIGHT).expect("equip lightsaber");
    assert_eq!(deflection(&mut w, pc), Some(0), "Jedi Defense alone is +0");
    w.grant_feat(pc, FEAT_ADVANCED_JEDI_DEFENSE);
    assert_eq!(deflection(&mut w, pc), Some(3), "Advanced is +3");
    w.grant_feat(pc, FEAT_MASTER_JEDI_DEFENSE);
    assert_eq!(deflection(&mut w, pc), Some(6));
}

/// Forgets the creature's feats: the party leader starts with its class feats
/// (Jedi Defense among them).
fn no_feats(w: &mut World, id: ObjectId) {
    w.known_feats(id);
    w.progression.known.get_mut(&id).expect("known feats").feats.clear();
}

#[test]
fn jedi_chain_gates_on_any_of_its_feats() {
    // Each feat of the chain alone, with a lightsaber, opens the gate at its
    // own bonus: Master without 55 or 1 is +6, Advanced without 55 is +3.
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    let blade = w.create_item("g_w_lghtsbr01", pc, 1).expect("lightsaber");
    w.equip(pc, blade, RIGHT).expect("equip lightsaber");
    no_feats(&mut w, pc);
    assert_eq!(deflection(&mut w, pc), None, "no Jedi Defense feat");
    w.grant_feat(pc, FEAT_MASTER_JEDI_DEFENSE);
    assert_eq!(deflection(&mut w, pc), Some(6), "Master alone");
    no_feats(&mut w, pc);
    w.grant_feat(pc, FEAT_ADVANCED_JEDI_DEFENSE);
    assert_eq!(deflection(&mut w, pc), Some(3), "Advanced alone");
}

#[test]
fn deflect_is_one_plus_one_per_two_levels_after_first() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    w.grant_feat(pc, FEAT_DEFLECT);
    let at = |w: &mut World, level: i32| {
        set_classes(w, pc, vec![(SOLDIER, level)]);
        deflection(w, pc)
    };
    assert_eq!(at(&mut w, 1), Some(1));
    assert_eq!(at(&mut w, 2), Some(1));
    assert_eq!(at(&mut w, 3), Some(2));
    assert_eq!(at(&mut w, 5), Some(3));
}

#[test]
fn deflect_and_jedi_defense_take_the_better_bonus() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    set_classes(&mut w, pc, vec![(JEDI_GUARDIAN, 5)]);
    let blade = w.create_item("g_w_lghtsbr01", pc, 1).expect("lightsaber");
    w.equip(pc, blade, RIGHT).expect("equip lightsaber");
    w.grant_feat(pc, FEAT_JEDI_DEFENSE);
    w.grant_feat(pc, FEAT_DEFLECT);
    assert_eq!(deflection(&mut w, pc), Some(3), "Deflect at level 5 (+3) beats Jedi Defense (+0)");
    w.grant_feat(pc, FEAT_ADVANCED_JEDI_DEFENSE);
    assert_eq!(deflection(&mut w, pc), Some(3), "Advanced ties Deflect at +3");
    w.grant_feat(pc, FEAT_MASTER_JEDI_DEFENSE);
    assert_eq!(deflection(&mut w, pc), Some(6), "Master beats Deflect");
}

/// The leader with `weapon` in hand, and a mining droid `dist` m away that
/// cannot die. `deflect` gives the droid feat 168.
fn range_fight(weapon: &str, dist: f32, deflect: bool) -> Option<(World, ObjectId)> {
    let mut w = world()?;
    let pc = w.party_leader();
    let blade = w.create_item(weapon, pc, 1)?;
    w.equip(pc, blade, RIGHT).ok()?;
    {
        let c = w.get_mut(pc).and_then(|o| o.creature_mut())?;
        c.hp = 100_000;
        c.max_hp = 100_000;
    }
    let pos = w.get(pc)?.position + Vec3::X * dist;
    let target = w.spawn_creature("g_assassindrd002", pos, 0.0).ok()?;
    {
        let c = w.get_mut(target).and_then(|o| o.creature_mut())?;
        c.hp = 100_000;
        c.max_hp = 100_000;
    }
    if deflect {
        w.grant_feat(target, FEAT_DEFLECT);
    }
    Some((w, target))
}

/// The leader attacks `target` for `seconds` with the same seed. Returns the
/// (hits, misses) of its attacks on it.
fn shoot(w: &mut World, target: ObjectId, seconds: f32) -> (u32, u32) {
    let pc = w.party_leader();
    w.combat.seed(0x5eed);
    w.player_attack(target);
    let (mut hits, mut misses) = (0, 0);
    for _ in 0..(seconds / DT) as i32 {
        w.tick(DT);
        for e in w.drain_events() {
            match e {
                WorldEvent::AttackHit { attacker, target: t, .. } if attacker == pc && t == target => hits += 1,
                WorldEvent::AttackMissed { attacker, target: t } if attacker == pc && t == target => misses += 1,
                _ => {}
            }
        }
    }
    (hits, misses)
}

#[test]
fn deflect_turns_aside_some_blaster_bolts() {
    let Some((mut plain, t)) = range_fight("w_blaste_01", 5.0, false) else { return };
    let Some((mut deflect, u)) = range_fight("w_blaste_01", 5.0, true) else { return };
    let pc = plain.party_leader();
    assert!(plain.combat_stats(pc).expect("stats").main.ranged, "a blaster");
    let (hits_plain, _) = shoot(&mut plain, t, 200.0);
    let (hits_deflect, _) = shoot(&mut deflect, u, 200.0);
    assert!(hits_plain >= 20, "{hits_plain} hits: too few attacks to compare");
    assert!(hits_deflect < hits_plain, "{hits_deflect} hits with Deflect, {hits_plain} without");
    assert!(plain.vm_errors.is_empty() && deflect.vm_errors.is_empty(), "{:?}", deflect.vm_errors);
}

#[test]
fn deflect_does_not_turn_aside_melee_hits() {
    let Some((mut plain, t)) = range_fight("g_w_lghtsbr01", 1.0, false) else { return };
    let Some((mut deflect, u)) = range_fight("g_w_lghtsbr01", 1.0, true) else { return };
    let (hits_plain, _) = shoot(&mut plain, t, 60.0);
    let (hits_deflect, _) = shoot(&mut deflect, u, 60.0);
    assert!(hits_plain > 0, "no melee hits at all");
    assert_eq!(hits_deflect, hits_plain, "melee is never deflected");
}
