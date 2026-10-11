//! Conversation facing against the real install. Skips (passes) when no
//! install is found at K2_INSTALL or ../.. (the repo inside the game folder).

use std::sync::Arc;

use k2_res::Install;
use k2_world::dialog::LOCK_ORIENTATION_LOCAL;
use k2_world::{ObjectKind, World};

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

/// Cosine between `id`'s facing and the direction to `to` (1 = facing it).
fn facing_cos(w: &World, id: u32, to: u32) -> f32 {
    let (a, b) = (w.get(id).unwrap(), w.get(to).unwrap());
    let d = (b.position - a.position).truncate().normalize_or_zero();
    let [x, y] = k2_data::module::dir_from_bearing(a.bearing);
    d.x * x + d.y * y
}

#[test]
fn participants_face_each_other_unless_locked() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    let pc = w.party_leader();
    let npcs: Vec<(u32, String)> = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && o.id != pc)
        .filter_map(|o| o.conversation.clone().map(|d| (o.id, d)))
        .collect();
    let mut checked = 0;
    for (npc, dlg) in npcs {
        // Point the PC directly away from the NPC.
        let away = k2_world::object::bearing_towards(w.get(npc).unwrap().position, w.get(pc).unwrap().position).unwrap();
        w.get_mut(pc).unwrap().bearing = away;
        if !w.start_conversation(npc, pc, &dlg) {
            continue;
        }
        let view = w.conversation_view();
        let staged = view.is_none();
        if !staged {
            assert!(facing_cos(&w, pc, npc) > 0.99, "pc faces {dlg} owner");
            assert!(facing_cos(&w, npc, pc) > 0.99, "{dlg} owner faces pc");
            checked += 1;
        }
        w.end_conversation(true);
        if checked >= 3 {
            break;
        }
    }
    assert!(checked > 0, "no conversation started in 101per");

    // A locked PC keeps its bearing.
    let (npc, dlg) = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && o.id != pc)
        .find_map(|o| o.conversation.clone().map(|d| (o.id, d)))
        .unwrap();
    let away = k2_world::object::bearing_towards(w.get(npc).unwrap().position, w.get(pc).unwrap().position).unwrap();
    let p = w.get_mut(pc).unwrap();
    p.bearing = away;
    p.local_bools.insert(LOCK_ORIENTATION_LOCAL, true);
    if w.start_conversation(npc, pc, &dlg) {
        assert!(facing_cos(&w, pc, npc) < -0.99, "locked pc must not turn");
        w.end_conversation(true);
    }
}

/// A CameraAngle 4 line in a dialog with no camera model is not a staged line
/// for facing (dialog.rs scripted_staging needs the camera model), so the
/// speakers still turn toward each other. 101atton has no camera model; its
/// entry 58 is made a CameraAngle 4 line in the test.
#[test]
fn camera_angle_line_without_a_camera_model_still_faces() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    let pc = w.party_leader();
    let owner = w.iter().find(|o| o.kind == ObjectKind::Creature && o.id != pc).map(|o| o.id).expect("an NPC");
    assert!(w.start_conversation(owner, pc, "101atton"), "the dialog starts");
    let conv = w.conversation.as_mut().expect("in conversation");
    Arc::get_mut(&mut conv.dialog).expect("sole owner of the dialog").entries[58].camera_angle = 4;

    let face = k2_world::object::bearing_towards(w.get(owner).unwrap().position, w.get(pc).unwrap().position).unwrap();
    w.get_mut(owner).unwrap().bearing = face + std::f32::consts::PI;
    assert!(facing_cos(&w, owner, pc) < -0.99, "the owner is turned away before the line");
    w.choose_reply(0);
    assert_eq!(w.conversation.as_ref().and_then(|c| c.entry), Some(58), "the conversation reached entry 58");
    assert!(facing_cos(&w, owner, pc) > 0.99, "the owner still turns toward the pc");
    assert!(facing_cos(&w, pc, owner) > 0.99, "the pc still turns toward the owner");
}
