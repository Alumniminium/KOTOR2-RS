//! Two weapon sets and the X-key swap: the active pair lives in slots 4/5
//! and drives combat stats, the inactive pair in 18/19. Skips (passes) when
//! no install is found.

use k2_res::Install;
use k2_script::ObjectId;
use k2_world::{World, WorldEvent};

const DT: f32 = 1.0 / 30.0;
const RIGHT: i32 = 4;
const LEFT: i32 = 5;
const RIGHT2: i32 = 18;
const LEFT2: i32 = 19;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    let mut w = World::new(install).ok()?;
    w.enter_module("101per", None).unwrap();
    for _ in 0..30 {
        w.tick(DT);
    }
    Some(w)
}

fn in_slot(w: &World, c: ObjectId, slot: i32) -> Option<ObjectId> {
    w.equipment(c).into_iter().find(|&(s, _)| s == slot).map(|(_, i)| i)
}

fn item(w: &mut World, pc: ObjectId, template: &str) -> ObjectId {
    w.create_item(template, pc, 1).unwrap_or_else(|| panic!("no item {template}"))
}

#[test]
fn swap_trades_the_pairs_and_follows_combat_stats() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    let blade = item(&mut w, pc, "g_w_lghtsbr01");
    let short = item(&mut w, pc, "g_w_shortsbr01");
    let staff = item(&mut w, pc, "g_w_dblsbr001");
    let bare = w.combat_stats(pc).unwrap();

    // Set 1 (active) gets a saber in each hand, set 2 a double saber.
    w.equip(pc, blade, RIGHT).unwrap();
    w.equip(pc, short, LEFT).unwrap();
    let (r2, l2) = w.weapon_set_slots(pc, 1);
    assert_eq!((r2, l2), (RIGHT2, LEFT2));
    w.equip(pc, staff, r2).unwrap();
    assert_eq!(w.weapon_set(pc), 0);
    assert_eq!(in_slot(&w, pc, RIGHT), Some(blade));
    assert_eq!(in_slot(&w, pc, RIGHT2), Some(staff));
    let dual = w.combat_stats(pc).unwrap();
    assert!(dual.offhand.is_some(), "set 1 dual wields");
    assert_ne!(dual.main.num_dice * dual.main.die, bare.main.num_dice * bare.main.die);

    w.drain_events();
    assert!(w.swap_weapon_sets(pc));
    let evs = w.drain_events();
    assert!(evs.iter().any(|e| matches!(e, WorldEvent::EquipmentChanged { creature, slot: RIGHT } if *creature == pc)));
    assert!(evs.iter().any(|e| matches!(e, WorldEvent::WeaponSetSwapped { creature, set: 1 } if *creature == pc)));
    assert_eq!(w.weapon_set(pc), 1);
    assert_eq!(in_slot(&w, pc, RIGHT), Some(staff), "set 2 is live");
    assert_eq!(in_slot(&w, pc, LEFT), None);
    assert_eq!(in_slot(&w, pc, RIGHT2), Some(blade));
    assert_eq!(in_slot(&w, pc, LEFT2), Some(short));
    let staff_stats = w.combat_stats(pc).unwrap();
    assert!(staff_stats.offhand.is_none(), "set 2 holds no off-hand weapon");
    assert_ne!(staff_stats.main.wield, dual.main.wield, "the double saber wields differently");
    // Set labels are stable: set 1 now lives in 18/19, set 2 in 4/5.
    assert_eq!(w.weapon_set_slots(pc, 0), (RIGHT2, LEFT2));
    assert_eq!(w.weapon_set_slots(pc, 1), (RIGHT, LEFT));

    assert!(w.swap_weapon_sets(pc));
    assert_eq!(w.weapon_set(pc), 0);
    assert_eq!(in_slot(&w, pc, RIGHT), Some(blade));
    assert_eq!(in_slot(&w, pc, LEFT), Some(short));
    assert_eq!(in_slot(&w, pc, RIGHT2), Some(staff));
    let back = w.combat_stats(pc).unwrap();
    assert!(back.offhand.is_some());
    assert_eq!(back.defense, dual.defense);
}

#[test]
fn two_handed_rule_applies_per_set() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    let blade = item(&mut w, pc, "g_w_lghtsbr01");
    let short = item(&mut w, pc, "g_w_shortsbr01");
    let staff = item(&mut w, pc, "g_w_dblsbr001");
    w.equip(pc, blade, RIGHT).unwrap();
    w.equip(pc, short, LEFT).unwrap();
    let spare = item(&mut w, pc, "g_w_shortsbr02");
    let spare_main = item(&mut w, pc, "g_w_lghtsbr01");
    w.equip(pc, spare_main, RIGHT2).unwrap();
    w.equip(pc, spare, LEFT2).unwrap();
    // A two-hander in the other set's right hand frees only that set's left.
    w.equip(pc, staff, RIGHT2).unwrap();
    assert_eq!(in_slot(&w, pc, LEFT2), None, "two-hander frees its own off hand");
    assert_eq!(in_slot(&w, pc, LEFT), Some(short), "the live set is untouched");
    assert!(w.can_equip(pc, spare, LEFT2).is_err(), "off hand blocked by the two-hander");
    // Armour does not fit a weapon slot of either set.
    let armour = item(&mut w, pc, "g_a_clothes01");
    assert!(w.can_equip(pc, armour, RIGHT2).is_err());
}

#[test]
fn swap_needs_a_weapon_somewhere() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    for s in [RIGHT, LEFT, RIGHT2, LEFT2] {
        w.unequip(pc, s);
    }
    assert!(!w.swap_weapon_sets(pc));
    assert_eq!(w.weapon_set(pc), 0);
    let blade = item(&mut w, pc, "g_w_lghtsbr01");
    w.equip(pc, blade, RIGHT2).unwrap();
    assert!(w.swap_weapon_sets(pc));
    assert_eq!(in_slot(&w, pc, RIGHT), Some(blade));
}

#[test]
fn weapon_sets_survive_save_and_load() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    let blade = item(&mut w, pc, "g_w_lghtsbr01");
    let staff = item(&mut w, pc, "g_w_dblsbr001");
    w.equip(pc, blade, RIGHT).unwrap();
    w.equip(pc, staff, RIGHT2).unwrap();
    w.swap_weapon_sets(pc);
    let save = w.save();
    let install = Install::discover(concat!(env!("CARGO_MANIFEST_DIR"), "/../../..")).ok();
    let Some(install) = install else { return };
    let l = World::load(install, &save).unwrap();
    let pc = l.party_leader();
    assert_eq!(l.weapon_set(pc), 1);
    let tag = |s| in_slot(&l, pc, s).and_then(|i| l.get(i)).map(|o| o.template.clone());
    assert_eq!(tag(RIGHT).as_deref(), Some("g_w_dblsbr001"));
    assert_eq!(tag(RIGHT2).as_deref(), Some("g_w_lghtsbr01"));
}

fn strip_key(v: &mut ciborium::Value, key: &str) -> usize {
    match v {
        ciborium::Value::Map(m) => {
            let before = m.len();
            m.retain(|(k, _)| k.as_text() != Some(key));
            before - m.len() + m.iter_mut().map(|(_, x)| strip_key(x, key)).sum::<usize>()
        }
        ciborium::Value::Array(a) => a.iter_mut().map(|x| strip_key(x, key)).sum(),
        ciborium::Value::Tag(_, x) => strip_key(x, key),
        _ => 0,
    }
}

#[test]
fn saves_from_before_weapon_sets_still_load() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    let blade = item(&mut w, pc, "g_w_lghtsbr01");
    w.equip(pc, blade, RIGHT).unwrap();
    let save = w.save();
    let mut tree: ciborium::Value = ciborium::from_reader(save.state.as_slice()).unwrap();
    assert!(strip_key(&mut tree, "weapon_set") > 0, "creatures carry weapon_set");
    let mut state = Vec::new();
    ciborium::into_writer(&tree, &mut state).unwrap();
    let old = k2_world::save::SaveData { version: save.version, module: save.module.clone(), state };
    let Ok(install) = Install::discover(concat!(env!("CARGO_MANIFEST_DIR"), "/../../..")) else { return };
    let l = World::load(install, &old).expect("old save loads");
    let pc = l.party_leader();
    assert_eq!(l.weapon_set(pc), 0);
    assert!(in_slot(&l, pc, RIGHT).is_some());
}

#[test]
fn drawing_holds_the_hands_for_the_equip_clip() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    assert_eq!(w.weapon_busy(pc), 0.0);
    let blade = item(&mut w, pc, "g_w_lghtsbr01");
    let staff = item(&mut w, pc, "g_w_dblsbr001");
    w.equip(pc, blade, RIGHT).unwrap();
    let busy = w.weapon_busy(pc);
    assert!((busy - k2_world::inventory::EQUIP_SECONDS).abs() < 0.05, "busy {busy}");
    for _ in 0..40 {
        w.tick(DT);
    }
    assert_eq!(w.weapon_busy(pc), 0.0, "idle after the clip");
    // A swap draws again; an off-set change does not.
    w.equip(pc, staff, RIGHT2).unwrap();
    assert_eq!(w.weapon_busy(pc), 0.0, "the stored set is not in the hands");
    assert!(w.swap_weapon_sets(pc));
    assert!(w.weapon_busy(pc) > 0.9);
    for _ in 0..40 {
        w.tick(DT);
    }
    w.unequip(pc, RIGHT);
    assert!(w.weapon_busy(pc) > 0.9, "unequipping draws too");
}
