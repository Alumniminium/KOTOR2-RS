//! Attack and stance clip choice against the real models. A ranged attack
//! never resolves to a melee swing clip (swingshort/swinglong/hit events); the
//! weapon's wield class maps to its animation family (pistol 4 -> 5, rifle 5 ->
//! 7, heavy 6 -> 9, dual pistols -> 6); the in-combat stance is g<family>r1,
//! then the creature ready clips (creadyr) that droid and creature bodies carry.
//! Skips (passes) when no install is found.

use std::collections::HashMap;
use std::io::Write;

use k2_res::{Install, ResKey};
use k2_world::combat::{attack_clip_list, blaster_family, ready_clips};
use k2_world::World;
use rakata_core::ResourceType;

const DT: f32 = 1.0 / 30.0;

/// Clips of `body` and its supermodel chain (the child's clip wins), with the
/// names of the events each clip fires, lowercased.
fn clips(w: &World, body: &str) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    let mut cur = Some(body.to_ascii_lowercase());
    let mut depth = 0;
    while let Some(name) = cur.take() {
        depth += 1;
        if depth > 8 {
            break;
        }
        let bytes = w.res.get(&ResKey::new(&name, ResourceType::Mdl)).unwrap().unwrap_or_else(|| panic!("{name}: no MDL in the install"));
        let mdx = w.res.get(&ResKey::new(&name, ResourceType::Mdx)).unwrap();
        let m = k2_mdl::read_mdl_from_bytes(&bytes, mdx.as_deref()).unwrap_or_else(|e| panic!("{name}: the model does not parse: {e:?}"));
        for a in &m.animations {
            out.entry(a.name.to_ascii_lowercase()).or_insert_with(|| a.events.iter().map(|e| e.name.to_ascii_lowercase()).collect());
        }
        cur = (!m.supermodel_name.is_empty() && m.supermodel_name != "NULL").then(|| m.supermodel_name.to_ascii_lowercase());
    }
    out
}

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(&dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP anim_clips test: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    let mut w = World::new(install).unwrap_or_else(|e| panic!("{dir}: the install does not load into a world: {e:?}"));
    w.enter_module("101per", None).unwrap();
    for _ in 0..30 {
        w.tick(DT);
    }
    Some(w)
}

fn is_melee(events: &[String]) -> bool {
    events.iter().any(|e| e.starts_with("swing") || e == "hit" || e == "clash")
}

/// The clip the client plays: the first name in the list that the model's
/// chain has.
fn first_present<'a>(c: &HashMap<String, Vec<String>>, names: &'a [String]) -> Option<&'a String> {
    names.iter().find(|n| c.contains_key(n.as_str()))
}

#[test]
fn weapon_wield_maps_to_the_animation_family() {
    // animations.2da: b5/g5 single blaster, b6/g6 dual blasters, b7/g7 rifle,
    // b9/g9 heavy carbine. Melee wields are their own families.
    assert_eq!(blaster_family(4, false), 5, "pistol plays the single-blaster family");
    assert_eq!(blaster_family(5, false), 7, "rifle plays the rifle family");
    assert_eq!(blaster_family(6, false), 9, "heavy plays the heavy-carbine family");
    assert_eq!(blaster_family(4, true), 6, "two pistols play the dual-blaster family");
    for melee in [1, 2, 3] {
        assert_eq!(blaster_family(melee, false), melee);
    }
}

#[test]
fn ranged_attacks_pick_a_blaster_clip_on_the_area_models() {
    let Some(w) = world() else { return };
    // Iziz cantina (503ond) and Peragus bodies: humanoid supermodel chains.
    let bodies = [
        "p_mandalorebb", "p_hk47", "pmbam", "pmbhm", "n_commf", "n_commm", "n_malethug", "n_sithsoldier", "n_bith", "n_duros",
        "n_ondcomm1", "n_twilekf", "n_rodian", "n_quarren", "n_swoopgang", "n_sullustan", "n_walrusman",
    ];
    for body in bodies {
        let c = clips(&w, body);
        assert!(!c.is_empty(), "{body} has clips");
        // Pistol (4), rifle (5), heavy (6), both attack numbers.
        for wield in [4, 5, 6] {
            for k in [1, 2] {
                let names = attack_clip_list(wield, true, false, k, false);
                // A blaster's list has no melee, unarmed or throw family at all.
                assert!(
                    names.iter().all(|n| !(n.starts_with('g') || n.starts_with('c') || n.starts_with('m') || n.starts_with('t'))),
                    "{body} wield {wield}: non-blaster clip in {names:?}"
                );
                let chosen = first_present(&c, &names).unwrap_or_else(|| panic!("{body} has no blaster clip for wield {wield}: {names:?}"));
                assert!(!is_melee(&c[chosen]), "{body} wield {wield} attack {k} picks {chosen}, a melee clip with events {:?}", c[chosen]);
            }
        }
    }
}

#[test]
fn each_blaster_plays_its_own_family_shot() {
    let Some(w) = world() else { return };
    let c = clips(&w, "p_mandalorebb");
    // Pistol -> single (b5), rifle -> rifle (b7), heavy -> heavy carbine (b9),
    // dual pistols -> dual (b6).
    let cases = [((4, false), "b5a1"), ((5, false), "b7a1"), ((6, false), "b9a1"), ((4, true), "b6a1")];
    for ((wield, dual), want) in cases {
        let names = attack_clip_list(wield, true, dual, 1, false);
        assert_eq!(first_present(&c, &names).map(String::as_str), Some(want), "wield {wield} dual {dual}");
    }
}

#[test]
fn melee_attacks_still_pick_a_melee_clip() {
    let Some(w) = world() else { return };
    // The melee list is unchanged: a sword swings, and it has swing events.
    let c = clips(&w, "p_mandalorebb");
    let names = attack_clip_list(2, false, false, 1, false);
    let chosen = first_present(&c, &names).expect("a sword clip");
    assert!(is_melee(&c[chosen]), "{chosen} is a melee clip: {:?}", c[chosen]);
}

#[test]
fn ranged_attacks_on_droids_use_the_droid_blaster_clips() {
    let Some(w) = world() else { return };
    // A droid's attack list starts with b0a (attack_clip_list); the b0a clips
    // are the droid's own blaster attacks and carry no melee events.
    for body in ["c_drdmkone", "c_drdprobe", "c_drdwar"] {
        let c = clips(&w, body);
        assert!(c.contains_key("b0a1") && c.contains_key("b0a2"), "{body} blaster clips: {:?}", c.keys().collect::<Vec<_>>());
        let names = attack_clip_list(4, true, false, 1, true);
        assert_eq!(first_present(&c, &names).map(String::as_str), Some("b0a1"), "{body} plays its blaster attack");
        assert!(!is_melee(&c["b0a1"]));
    }
}

#[test]
fn ready_stance_follows_the_weapon_family_on_humanoid_models() {
    let Some(w) = world() else { return };
    let c = clips(&w, "p_mandalorebb");
    // The stance is g<family>r1 for each family the wield maps to.
    let cases = [((8, false), "g10r1"), ((2, false), "g2r1"), ((3, false), "g3r1"), ((4, false), "g5r1"), ((5, false), "g7r1"), ((6, false), "g9r1"), ((4, true), "g6r1")];
    for ((wield, dual), want) in cases {
        let family = if wield == 8 { 8 } else { blaster_family(wield, dual) };
        let ready = ready_clips(family);
        assert_eq!(first_present(&c, &ready).map(String::as_str), Some(want), "wield {wield} dual {dual}: {ready:?}");
    }
}

#[test]
fn droids_stand_ready_with_creadyr() {
    let Some(w) = world() else { return };
    // Droid bodies carry no g<family>r1 stance: they take creadyr, the creature
    // ready clip, before the standing idle.
    let c = clips(&w, "c_drdmkone");
    assert!(c.contains_key("creadyr"), "a droid has creadyr");
    assert!(!c.keys().any(|k| k.starts_with('g') && k.ends_with("r1")), "a droid has no g<family>r1 stance: {:?}", c.keys().filter(|k| k.starts_with('g') && k.ends_with("r1")).collect::<Vec<_>>());
    let ready = ready_clips(blaster_family(4, false));
    assert_eq!(first_present(&c, &ready).map(String::as_str), Some("creadyr"), "a pistol droid stands ready with creadyr: {ready:?}");
}

#[test]
fn main_hand_wield_follows_the_held_weapon() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    for (slot, _) in w.equipment(pc) {
        if slot == 4 {
            w.unequip(pc, slot);
        }
    }
    assert_eq!(w.main_hand_wield(pc), 8, "unarmed");
    let rifle = w.give_new_item("w_brifle_04", pc, 1, true).expect("a rifle");
    w.equip(pc, rifle, 4).unwrap();
    assert_eq!(w.main_hand_wield(pc), 5, "rifle");
    assert_eq!(w.main_hand_family(pc), 7, "the rifle stance family");
    let pistol = w.give_new_item("propbp01", pc, 1, true).expect("a blaster pistol");
    w.unequip(pc, 4);
    w.equip(pc, pistol, 4).unwrap();
    assert_eq!(w.main_hand_wield(pc), 4, "pistol");
    assert_eq!(w.main_hand_family(pc), 5, "the pistol stance family");
    // Two pistols: the dual-blaster family.
    let second = w.give_new_item("propbp01", pc, 1, true).expect("a second blaster pistol");
    w.equip(pc, second, 5).unwrap();
    assert_eq!(w.main_hand_family(pc), 6, "two pistols: the dual-blaster family");
    // Dual wield with blades: the main hand decides the stance.
    w.unequip(pc, 4);
    w.unequip(pc, 5);
    let blade = w.give_new_item("g_w_lghtsbr01", pc, 1, true).expect("a lightsaber");
    let short = w.give_new_item("g_w_shortsbr01", pc, 1, true).expect("a short lightsaber");
    w.equip(pc, blade, 4).unwrap();
    w.equip(pc, short, 5).unwrap();
    assert_eq!(w.main_hand_wield(pc), 2, "dual wield: the one-hand blade in the main hand");
    assert_eq!(w.main_hand_family(pc), 2, "dual blades keep the one-hand blade family");
}
