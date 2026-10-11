//! Slot candidate query (equipment screen) against the real install. Skips
//! (passes) when no install is found at K2_INSTALL or ../..

use k2_res::Install;
use k2_world::World;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

fn names(w: &mut World, slot: i32) -> Vec<String> {
    let me = w.player;
    w.equip_candidates(me, slot).into_iter().map(|v| v.template).collect()
}

#[test]
fn candidates_follow_slot_masks() {
    let Some(mut w) = world() else { return };
    w.enter_module("103per", None).unwrap();
    let pc = w.player;
    for t in ["w_blaste_01", "g_w_lghtsbr01", "g_w_dblsbr001", "a_heavy_01", "a_gloves_01", "a_helmet_01", "a_belt_01"] {
        w.give_new_item(t, pc, 1, true);
    }
    let right = names(&mut w, 4);
    let left = names(&mut w, 5);
    let body = names(&mut w, 1);
    let head = names(&mut w, 0);
    assert!(right.contains(&"w_blaste_01".to_string()), "{right:?}");
    assert!(right.contains(&"g_w_dblsbr001".to_string()), "{right:?}");
    // The off hand lists nothing while the main hand is empty.
    assert!(left.is_empty(), "{left:?}");
    // With a pistol in the main hand: two-handed weapons and other kinds of
    // weapon never fit the left hand, a second pistol does.
    let second = w.give_new_item("w_blaste_01", pc, 1, true).unwrap();
    let main = w.equip_candidates(pc, 4).into_iter().find(|v| v.template == "w_blaste_01").unwrap().id;
    w.equip(pc, main, 4).unwrap();
    let left = names(&mut w, 5);
    assert!(!left.contains(&"g_w_dblsbr001".to_string()), "{left:?}");
    assert!(!left.contains(&"g_w_lghtsbr01".to_string()), "{left:?}");
    assert!(left.contains(&"w_blaste_01".to_string()), "{left:?}");
    let _ = second;
    w.unequip(pc, 4);
    // Weapons never fit body or head; armour fits the body only.
    assert!(body.contains(&"a_heavy_01".to_string()), "{body:?}");
    assert!(!body.iter().any(|t| t.contains("_w_") || t.starts_with("w_")), "{body:?}");
    assert!(head.contains(&"a_helmet_01".to_string()), "{head:?}");
    assert!(!head.iter().any(|t| t.contains("_w_") || t.starts_with("w_") || t == "a_heavy_01"), "{head:?}");
    // Out-of-range slots are empty, not a panic.
    assert!(names(&mut w, 99).is_empty());
    assert!(names(&mut w, -1).is_empty());
}

#[test]
fn two_handed_blocks_left_hand_and_equip_returns_old_item() {
    let Some(mut w) = world() else { return };
    w.enter_module("103per", None).unwrap();
    let pc = w.player;
    let sabre = w.give_new_item("g_w_dblsbr001", pc, 1, true).unwrap();
    w.give_new_item("w_blaste_01", pc, 1, true);
    w.equip(pc, sabre, 4).unwrap();
    // Right hand holds a two-handed weapon: the left slot lists nothing.
    assert!(names(&mut w, 5).is_empty());
    // The equipped item left the candidate list; it is no longer in the pack.
    assert!(!names(&mut w, 4).contains(&"g_w_dblsbr001".to_string()));
    // Swapping in the pistol puts the sabre back as a candidate again.
    let pistol = w.equip_candidates(pc, 4)[0].id;
    w.equip(pc, pistol, 4).unwrap();
    assert!(names(&mut w, 4).contains(&"g_w_dblsbr001".to_string()));
}

#[test]
fn droid_and_organic_restrictions_filter_candidates() {
    let Some(mut w) = world() else { return };
    w.enter_module("103per", None).unwrap();
    let pc = w.player;
    // Find a droid-only and an organic-only template among the head/body
    // items the PC could hold; at least verify the filter agrees with
    // can_equip for every candidate and that nothing rejected is listed.
    for t in ["a_heavy_01", "a_gloves_01", "a_helmet_01", "a_belt_01"] {
        w.give_new_item(t, pc, 1, true);
    }
    for slot in [0, 1, 3, 7, 8, 9, 10] {
        let listed = w.equip_candidates(pc, slot);
        for v in w.party_inventory_views() {
            let ok = w.can_equip(pc, v.id, slot).is_ok();
            assert_eq!(listed.iter().any(|l| l.id == v.id), ok, "slot {slot} item {}", v.template);
        }
    }
}
