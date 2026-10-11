//! Companion conversations on the Ebon Hawk (003EBO), headless. Atton and
//! Bao-Dur are available as the prologue leaves them; the run ticks at 30 Hz
//! for 600 s, answering every dialog with reply 0. The companion join itself
//! goes through the party-selection GUI, which this run does not drive, so
//! the party stays at the player. Skips (passes) when no install is found.

use k2_res::Install;
use k2_world::{AvailableNpc, World, WorldEvent};

const DT: f32 = 1.0 / 30.0;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    let mut w = World::new(install).ok()?;
    w.available_npcs.insert(0, AvailableNpc { template: "p_atton".into(), selectable: true, ..Default::default() });
    w.available_npcs.insert(1, AvailableNpc { template: "p_baodur".into(), selectable: true, ..Default::default() });
    w.enter_module("003ebo", None).unwrap();
    Some(w)
}

/// What a run observed: dialogs started (dialog, owner) and the cutscene
/// markers the Ebon Hawk scripts print (CS: ...), in order.
#[derive(Default)]
struct Seen {
    started: Vec<(String, String)>,
    cutscenes: Vec<String>,
}

fn run(w: &mut World, secs: f32) -> Seen {
    let mut seen = Seen::default();
    for _ in 0..(secs / DT) as usize {
        w.tick(DT);
        for e in w.drain_events() {
            match e {
                WorldEvent::ConversationStarted { owner, dialog } => seen.started.push((dialog, w.describe(owner))),
                WorldEvent::DebugText(m) if m.starts_with("CS:") => seen.cutscenes.push(m),
                _ => {}
            }
        }
        if let Some(v) = w.conversation_view() {
            if v.replies.is_empty() {
                w.advance_conversation();
            } else {
                w.choose_reply(0);
            }
        }
    }
    seen
}

#[test]
fn ebon_hawk_companion_conversations_fire() {
    let Some(mut w) = world() else { return };
    let seen = run(&mut w, 600.0);
    assert!(seen.cutscenes.iter().any(|m| m.contains("FIRST ATTON")), "first Atton scene: {:?}", seen.cutscenes);
    assert!(seen.cutscenes.iter().any(|m| m.contains("SECOND ATTON")), "second Atton scene: {:?}", seen.cutscenes);
    assert!(seen.cutscenes.iter().any(|m| m.contains("SPAWNING PARTY")), "party spawn reached: {:?}", seen.cutscenes);
    assert!(seen.started.iter().any(|(_, owner)| owner.contains("Atton")), "no Atton dialog: {:?}", seen.started);
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
    assert!(w.unimplemented.is_empty(), "unimplemented routines: {:?}", w.unimplemented);
}
