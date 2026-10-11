use super::*;

/// Writes `w` into `slot` with a fixed timestamp.
fn put(root: &std::path::Path, slot: &str, w: &k2_world::World, ts: u64) {
    let mut meta = w.save_meta(slot);
    meta.timestamp = ts;
    save::write_slot(root, slot, &w.save(), &meta).unwrap();
}

#[test]
fn dead_leader_blocks_saving_and_last_save_skips_dead_states() {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = k2_res::Install::discover(dir) else { return };
    let mut w = k2_world::World::new(install.clone()).unwrap();
    w.auto_dialog = true;
    w.enter_module("101per", None).unwrap();
    for _ in 0..120 {
        w.tick(1.0 / 30.0);
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
    }
    let root = std::env::temp_dir().join(format!("k2-gameover-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert!(w.save_blocked().is_none(), "{:?}", w.save_blocked());
    put(&root, "autosave", &w, 100);
    put(&root, "save-0001", &w, 50);
    // The only party member is knocked out (not dead, nobody left
    // standing): every rolling save is refused from now on.
    let leader = w.party_leader();
    w.hurt_creature(leader, 10_000, k2_script::OBJECT_INVALID);
    assert!(w.get(leader).and_then(|o| o.creature()).unwrap().down);
    assert!(w.party_defeated());
    assert_eq!(w.save_blocked(), Some("the party is dead"));
    assert_eq!(w.exit_save_blocked(), Some("the party is dead"));
    assert!(crate::saves::write_timed(&w, &root).is_err(), "timed autosave while dead");
    assert!(!root.join(crate::saves::TIMED_SLOT).exists());
    // Dead states that got on disk anyway (older builds) are skipped,
    // even newer ones, and the rolling slots come after player saves.
    put(&root, "quicksave", &w, 300);
    put(&root, "timed", &w, 400);
    assert_eq!(last_good_slot(&root, &install).as_deref(), Some("autosave"));
    // Loading it brings a living leader back.
    let loaded = k2_world::World::load(install.clone(), &save::read_slot(&root, "autosave").unwrap()).unwrap();
    let c = loaded.get(loaded.party_leader()).and_then(|o| o.creature()).unwrap();
    assert!(!c.dead && c.hp > 0);
    assert!(loaded.save_blocked().is_none() || loaded.save_blocked() != Some("the party is dead"));
    // Nothing but dead states: no slot.
    let _ = std::fs::remove_dir_all(root.join("autosave"));
    let _ = std::fs::remove_dir_all(root.join("save-0001"));
    assert_eq!(last_good_slot(&root, &install), None);
    let _ = std::fs::remove_dir_all(&root);
}
