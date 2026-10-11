//! The 101per intro (New Game, after the Ebon Hawk prologue) against the
//! real install. Skips (passes) when no install is found.

use k2_res::Install;
use k2_world::World;

const DT: f32 = 1.0 / 30.0;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

#[test]
fn peragus_intro_runs_in_order_and_returns_control() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    let pc = w.party_leader();
    let tank = w.get(pc).unwrap().position;
    let wake = w.by_tag("WP_wakeup_donnie", 0).expect("wakeup waypoint");
    let wake_pos = w.get(wake).unwrap().position;

    let mut entries: Vec<usize> = Vec::new();
    let mut moved_at = None;
    let mut controlled_during = false;
    for _ in 0..(120.0 / DT) as usize {
        w.tick(DT);
        if let Some(e) = w.conversation.as_ref().and_then(|c| c.entry) {
            if entries.last() != Some(&e) {
                entries.push(e);
                if moved_at.is_none() && (w.get(pc).unwrap().position - wake_pos).truncate().length() < 0.5 {
                    moved_at = Some(e);
                }
            }
            controlled_during |= w.player_controlled();
            // Stand in for the UI timer: continue past each line.
            if w.conversation.as_ref().is_some_and(|c| c.shown_for > 0.1) {
                w.advance_conversation();
            }
        } else if !entries.is_empty() {
            break;
        }
    }
    assert_eq!(entries, (0..=18).collect::<Vec<_>>(), "101awake entries in order");
    assert!(!controlled_during, "the player has no control during the sequence");
    // The PC starts in the tank and is moved to the floor at entry 10.
    assert!((w.get(pc).unwrap().position - wake_pos).truncate().length() < 0.5, "pc ends on the floor spot");
    assert!((tank - wake_pos).truncate().length() > 3.0, "tank and floor spot are apart");
    assert_eq!(moved_at, Some(10), "moved out of the tank at entry 10");
    assert!(w.conversation.is_none(), "conversation over");
    assert!(w.player_controlled(), "control returned");
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}
