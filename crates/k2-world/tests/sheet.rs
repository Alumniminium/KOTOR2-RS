//! Character sheet, journal and item use against the real install. Skips
//! (passes) when no install is found.

use k2_res::Install;
use k2_world::World;

const DT: f32 = 1.0 / 30.0;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

#[test]
fn medpac_heals_through_its_impact_script() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    for _ in 0..30 {
        w.tick(DT);
    }
    let pc = w.party_leader();
    let medpac = w.give_new_item("g_i_medeqpmnt01", pc, 2, true).expect("medpac");
    assert!(w.is_usable_item(medpac));
    let max = w.get(pc).and_then(|o| o.creature()).map(|c| c.max_hp).unwrap();
    w.get_mut(pc).and_then(|o| o.creature_mut()).unwrap().hp = 1;
    let spells = w.use_item(pc, medpac, pc).expect("used");
    assert_eq!(spells, vec![64]);
    let hp = w.get(pc).and_then(|o| o.creature()).map(|c| c.hp).unwrap();
    assert!(hp > 1 && hp <= max, "hp after medpac: {hp} of {max}");
    let left = w.get(medpac).and_then(|o| o.item()).map(|i| i.stack_size).unwrap();
    assert_eq!(left, 1, "one unit used");
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

#[test]
fn sheet_and_journal_have_content() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    for _ in 0..90 {
        w.tick(DT);
    }
    let pc = w.party_leader();
    let s = w.character_sheet(pc).expect("sheet");
    assert!(!s.classes.is_empty() && !s.classes[0].0.is_empty(), "{:?}", s.classes);
    assert!(s.next_level_xp > 0);
    assert_eq!(w.skill_views(pc).len(), 8);
    assert!(w.skill_views(pc).iter().all(|k| !k.name.is_empty()));
    w.journal.insert("tutorial_garage".into(), 10);
    let j = w.journal_view();
    let q = j.iter().find(|q| q.tag == "tutorial_garage").expect("quest");
    assert!(!q.name.is_empty() && !q.text.is_empty(), "{q:?}");
}
