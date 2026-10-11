//! Influence (K2): ModifyInfluence / SetInfluence / GetInfluence move the
//! value, clamp it to 0..100, announce a real change through the engine's
//! feedback line and survive a save. Calls go through the routine table and
//! ScriptHost like a script does. Skips (passes) when no install is found.

use k2_res::Install;
use k2_script::{Call, ObjectId, ScriptHost, Value};
use k2_world::{AvailableNpc, World, WorldEvent};

const ATTON: i32 = 0;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    let mut w = World::new(install).ok()?;
    w.available_npcs.insert(ATTON, AvailableNpc { template: "p_atton".into(), selectable: true, ..Default::default() });
    w.enter_module("101per", None).unwrap();
    w.drain_events();
    Some(w)
}

/// Calls an engine routine the way a script does (routine table + ScriptHost).
fn call(w: &mut World, name: &str, caller: ObjectId, args: &[Value]) -> Value {
    let routines = w.routines.clone();
    let routine = routines.by_name(name).unwrap_or_else(|| panic!("no routine {name}"));
    let script = k2_script::load_ncs(&w.res, "k_trp_generic").expect("a script to host the call");
    w.call(Call { caller, script: &script, routine, args })
}

fn int(v: Value) -> i32 {
    match v {
        Value::Int(i) => i,
        other => panic!("expected an int, got {other:?}"),
    }
}

/// Feedback lines emitted since the last call.
fn feedback(w: &mut World) -> Vec<String> {
    w.drain_events()
        .into_iter()
        .filter_map(|e| match e {
            WorldEvent::Feedback { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn modify_and_set_influence_move_clamp_and_announce_changes() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    assert_eq!(int(call(&mut w, "GetInfluence", pc, &[Value::Int(ATTON)])), 50, "default is 50");

    call(&mut w, "ModifyInfluence", pc, &[Value::Int(ATTON), Value::Int(10)]);
    assert_eq!(int(call(&mut w, "GetInfluence", pc, &[Value::Int(ATTON)])), 60);
    let gained = feedback(&mut w);
    assert_eq!(gained.len(), 1, "one feedback line for a gain: {gained:?}");
    assert!(gained[0].contains(&w.npc_name(ATTON)), "the line names the companion: {gained:?}");

    call(&mut w, "ModifyInfluence", pc, &[Value::Int(ATTON), Value::Int(-70)]);
    assert_eq!(int(call(&mut w, "GetInfluence", pc, &[Value::Int(ATTON)])), 0, "clamped at 0");
    let lost = feedback(&mut w);
    assert_eq!(lost.len(), 1, "one feedback line for a loss: {lost:?}");
    assert_ne!(lost[0], gained[0], "loss uses its own string");

    // A change that clamps to the same value announces nothing.
    call(&mut w, "SetInfluence", pc, &[Value::Int(ATTON), Value::Int(150)]);
    assert_eq!(int(call(&mut w, "GetInfluence", pc, &[Value::Int(ATTON)])), 100, "clamped at 100");
    feedback(&mut w);
    call(&mut w, "ModifyInfluence", pc, &[Value::Int(ATTON), Value::Int(5)]);
    assert_eq!(int(call(&mut w, "GetInfluence", pc, &[Value::Int(ATTON)])), 100);
    assert!(feedback(&mut w).is_empty(), "no change, no line");
    call(&mut w, "SetInfluence", pc, &[Value::Int(ATTON), Value::Int(100)]);
    assert!(feedback(&mut w).is_empty(), "setting the same value is silent");
}

#[test]
fn influence_survives_a_save_and_load() {
    let Some(mut w) = world() else { return };
    let pc = w.party_leader();
    call(&mut w, "SetInfluence", pc, &[Value::Int(ATTON), Value::Int(77)]);
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).unwrap();
    let data = w.save();
    let loaded = World::load(install, &data).unwrap();
    assert_eq!(loaded.influence_of(ATTON), 77);
}
