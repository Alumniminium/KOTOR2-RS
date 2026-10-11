use super::*;

#[test]
fn timed_save_fires_every_two_minutes_and_retries_when_blocked() {
    let mut t = TimedSave::default();
    assert!(!t.step(119.0, false));
    assert!(t.step(1.0, false), "due at 120 s");
    assert!(!t.step(119.0, false), "interval restarted");
    // Due but blocked (conversation): retried 10 s later, repeatedly.
    assert!(!t.step(1.0, true));
    assert!(!t.step(9.0, false));
    assert!(t.step(1.0, false), "10 s after the blocked attempt");
    assert!(!t.step(30.0, false));
    let _ = t.step(90.0, true);
    assert!(!t.step(5.0, true));
    assert!(!t.step(5.0, true), "still blocked at the retry");
    assert!(t.step(10.0, false));
}

#[test]
fn timed_slot_is_written_apart_from_the_other_slots() {
    let dir = std::env::var("K2_INSTALL")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = k2_res::Install::discover(dir) else {
        return;
    };
    let mut w = k2_world::World::new(install.clone()).unwrap();
    w.enter_module("101per", None).unwrap();
    // The module's opening conversation blocks saving: while blocked the
    // timed save is refused, as the system retries later.
    let mut guard = 0;
    while w.save_blocked().is_some() {
        assert!(write_timed(&w, &std::env::temp_dir()).is_err());
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
        w.tick(1.0 / 30.0);
        guard += 1;
        assert!(guard < 3000, "never saveable: {:?}", w.save_blocked());
    }
    let root = std::env::temp_dir().join(format!("k2-timed-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for other in [AUTO_SLOT, QUICK_SLOT, "restart"] {
        std::fs::create_dir_all(root.join(other)).unwrap();
        std::fs::write(root.join(other).join("marker"), other).unwrap();
    }
    let dir = write_timed(&w, &root).expect("timed save");
    assert_eq!(dir, root.join(TIMED_SLOT));
    let loaded =
        k2_world::World::load(install, &save::read_slot(&root, TIMED_SLOT).unwrap()).unwrap();
    assert_eq!(loaded.state_digest(), w.state_digest());
    for other in [AUTO_SLOT, QUICK_SLOT, "restart"] {
        assert_eq!(
            std::fs::read_to_string(root.join(other).join("marker")).unwrap(),
            other
        );
        assert!(
            !root.join(other).join("save.bin").exists(),
            "{other} untouched"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
