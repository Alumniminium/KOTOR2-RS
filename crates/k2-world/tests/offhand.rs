//! Off-hand rules: the left hand only holds a weapon beside a compatible
//! one-handed main-hand weapon, in both weapon sets. Skips (passes) when no
//! install is found.

use k2_res::Install;
use k2_script::ObjectId;
use k2_world::inventory::ItemError;
use k2_world::World;

const DT: f32 = 1.0 / 30.0;
const RIGHT: i32 = 4;
const LEFT: i32 = 5;
const RIGHT2: i32 = 18;
const LEFT2: i32 = 19;

fn install() -> Option<Install> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    Install::discover(dir).ok()
}

fn world() -> Option<World> {
    let mut w = World::new(install()?).ok()?;
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

/// Writes straight into the equipment map, like a save from before the rules.
fn force_equip(w: &mut World, item: ObjectId, pc: ObjectId, slot: i32) {
    let o = w.get_mut(pc).unwrap();
    o.inventory.retain(|&i| i != item);
    o.creature_mut().unwrap().equipped.insert(slot, item);
}

#[test]
fn off_hand_needs_a_main_hand_weapon() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    let blade = item(&mut w, pc, "g_w_lghtsbr01");
    let short = item(&mut w, pc, "g_w_shortsbr01");
    for (r, l) in [(RIGHT, LEFT), (RIGHT2, LEFT2)] {
        assert_eq!(w.can_equip(pc, short, l), Err(ItemError::NeedsMainHand));
        assert_eq!(w.equip(pc, short, l), Err(ItemError::NeedsMainHand));
        assert!(w.equip_candidates(pc, l).is_empty(), "empty main hand lists nothing for slot {l}");
        assert_eq!(in_slot(&w, pc, l), None);
        w.equip(pc, blade, r).unwrap();
        assert!(w.equip_candidates(pc, l).iter().any(|v| v.id == short));
        w.equip(pc, short, l).unwrap();
        // The main-hand weapon cannot hop to the off hand and leave it empty.
        assert_eq!(w.equip(pc, blade, l), Err(ItemError::NeedsMainHand));
        assert_eq!(in_slot(&w, pc, r), Some(blade));
        w.unequip(pc, l);
        w.unequip(pc, r);
    }
}

#[test]
fn only_matching_one_handed_weapons_pair_up() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    let blade = item(&mut w, pc, "g_w_lghtsbr01");
    let pistol = item(&mut w, pc, "w_blaste_01");
    let pistol2 = item(&mut w, pc, "w_blaste_01");
    let staff = item(&mut w, pc, "g_w_dblsbr001");
    w.equip(pc, blade, RIGHT).unwrap();
    assert_eq!(w.can_equip(pc, pistol, LEFT), Err(ItemError::NoDualWield));
    assert_eq!(w.can_equip(pc, staff, LEFT), Err(ItemError::WrongSlot(LEFT)));
    // A new main hand that cannot pair with the off hand sends it back.
    let short = item(&mut w, pc, "g_w_shortsbr01");
    w.equip(pc, short, LEFT).unwrap();
    w.equip(pc, pistol, RIGHT).unwrap();
    assert_eq!(in_slot(&w, pc, LEFT), None, "a pistol cannot pair with the saber");
    assert!(w.party_inventory().contains(&short));
    w.equip(pc, pistol2, LEFT).unwrap();
    assert_eq!(in_slot(&w, pc, RIGHT), Some(pistol));
    // A two-hander empties the off hand.
    w.equip(pc, staff, RIGHT).unwrap();
    assert_eq!(in_slot(&w, pc, LEFT), None);
    let pack = w.party_inventory();
    assert!(pack.iter().filter(|&&i| w.get(i).unwrap().template == "w_blaste_01").count() >= 1, "{:?}", pack.iter().map(|&i| w.get(i).unwrap().template.clone()).collect::<Vec<_>>());
    let back = *pack.iter().find(|&&i| w.get(i).unwrap().template == "w_blaste_01").unwrap();
    assert_eq!(w.can_equip(pc, back, LEFT), Err(ItemError::TwoHanded));
}

#[test]
fn removing_the_main_hand_moves_the_off_hand_over() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    for (r, l) in [(RIGHT, LEFT), (RIGHT2, LEFT2)] {
        let blade = item(&mut w, pc, "g_w_lghtsbr01");
        let short = item(&mut w, pc, "g_w_shortsbr01");
        w.equip(pc, blade, r).unwrap();
        w.equip(pc, short, l).unwrap();
        assert_eq!(w.unequip(pc, r), Some(blade));
        assert_eq!(in_slot(&w, pc, r), Some(short), "off hand weapon became the main hand");
        assert_eq!(in_slot(&w, pc, l), None);
        w.unequip(pc, r);
    }
}

#[test]
fn swapping_sets_never_leaves_an_off_hand_alone() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    let blade = item(&mut w, pc, "g_w_lghtsbr01");
    let short = item(&mut w, pc, "g_w_shortsbr01");
    w.equip(pc, blade, RIGHT).unwrap();
    w.equip(pc, short, LEFT).unwrap();
    let blade2 = item(&mut w, pc, "g_w_lghtsbr01");
    w.equip(pc, blade2, RIGHT2).unwrap();
    for _ in 0..4 {
        assert!(w.swap_weapon_sets(pc));
        for (r, l) in [(RIGHT, LEFT), (RIGHT2, LEFT2)] {
            assert!(in_slot(&w, pc, l).is_none() || in_slot(&w, pc, r).is_some(), "off hand alone after swap");
        }
    }
    // Emptying a set's main hand through the screen's logical slots, then swapping.
    let (r, l) = w.weapon_set_slots(pc, 0);
    w.unequip(pc, r);
    assert_eq!(in_slot(&w, pc, l), None);
    assert_eq!(in_slot(&w, pc, r), Some(short));
    assert!(w.swap_weapon_sets(pc));
    assert!(in_slot(&w, pc, LEFT).is_none());
}

#[test]
fn old_saves_with_an_off_hand_only_setup_are_repaired_on_load() {
    let Some(mut w) = world() else { return };
    let Some(install) = install() else { return };
    let pc = w.party_leader();
    let short = item(&mut w, pc, "g_w_shortsbr01");
    let short2 = item(&mut w, pc, "g_w_shortsbr01");
    let staff = item(&mut w, pc, "g_w_dblsbr001");
    // The states the old rules allowed: off hand only, in both sets, and an
    // off hand beside a two-hander.
    force_equip(&mut w, short, pc, LEFT);
    force_equip(&mut w, short2, pc, LEFT2);
    force_equip(&mut w, staff, pc, RIGHT2);
    assert_eq!(in_slot(&w, pc, RIGHT), None);
    let save = w.save();
    let l = World::load(install, &save).unwrap();
    let pc = l.party_leader();
    assert_eq!(in_slot(&l, pc, LEFT), None);
    assert_eq!(in_slot(&l, pc, RIGHT).map(|i| l.get(i).unwrap().template.clone()).as_deref(), Some("g_w_shortsbr01"));
    assert_eq!(in_slot(&l, pc, LEFT2), None, "two-hander and off hand do not coexist");
    assert_eq!(in_slot(&l, pc, RIGHT2).map(|i| l.get(i).unwrap().template.clone()).as_deref(), Some("g_w_dblsbr001"));
}
