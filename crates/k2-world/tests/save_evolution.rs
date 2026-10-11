//! Saves survive schema changes: fields added since the save (missing keys
//! take defaults) and fields removed since the save (unknown keys are
//! ignored). Simulated by editing the decoded CBOR value tree of a real
//! save. Skips (passes) when no install is found.

use ciborium::Value;
use k2_res::Install;
use k2_world::save::SaveData;
use k2_world::{World, WorldError};

fn install() -> Option<Install> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let found = Install::discover(&dir).ok();
    if found.is_none() {
        // Every test passes vacuously without an install; say so once, on stderr
        // (not captured by the test harness).
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "SKIP: no KotOR 2 install at {dir}; save_evolution tests did not run");
        });
    }
    found
}

fn map_mut(v: &mut Value) -> &mut Vec<(Value, Value)> {
    match v {
        Value::Map(m) => m,
        other => panic!("expected a CBOR map, got {other:?}"),
    }
}

fn remove_key(m: &mut Vec<(Value, Value)>, key: &str) -> bool {
    let before = m.len();
    m.retain(|(k, _)| k.as_text() != Some(key));
    m.len() != before
}

#[test]
fn save_loads_with_added_and_removed_fields() {
    let Some(inst) = install() else { return };
    let mut a = World::new(inst.clone()).unwrap();
    a.enter_module("101per", None).unwrap();
    for _ in 0..60 {
        a.tick(1.0 / 30.0);
        let _ = a.drain_events();
    }
    let save = a.save();
    let mut tree: Value = ciborium::from_reader(save.state.as_slice()).unwrap();
    let top = map_mut(&mut tree);

    // Fields "added after the save was made": the save lacks them.
    for key in ["solo_mode", "progression", "craft", "selected_planet"] {
        assert!(remove_key(top, key), "state has no field {key}");
    }
    if let Some((_, Value::Map(combat))) = top.iter_mut().find(|(k, _)| k.as_text() == Some("combat"))
        && !combat.is_empty()
    {
        combat.remove(0);
    }

    // Fields "removed since the save": the save has keys the build lacks.
    top.push((Value::Text("field_from_an_older_build".into()), Value::Integer(42.into())));
    let mut objects = 0;
    if let Some((_, Value::Map(objs))) = top.iter_mut().find(|(k, _)| k.as_text() == Some("objects")) {
        for (_, o) in objs.iter_mut() {
            if let Value::Map(fields) = o {
                fields.push((Value::Text("obsolete_field".into()), Value::Text("x".into())));
                objects += 1;
            }
        }
    }
    assert!(objects > 0, "objects not encoded as maps");

    let mut state = Vec::new();
    ciborium::into_writer(&tree, &mut state).unwrap();
    let edited = SaveData { version: save.version, module: save.module.clone(), state };
    let bytes = edited.to_bytes();
    let b = World::load(inst, &SaveData::from_bytes(&bytes).unwrap()).expect("edited save must load");
    assert_eq!(b.objects.len(), a.objects.len());
    let (pa, pb) = (a.get(a.party_leader()).unwrap().position, b.get(b.party_leader()).unwrap().position);
    assert!(pa.distance(pb) < 1e-4, "leader moved: {pa} vs {pb}");
}

#[test]
fn pre_cbor_saves_are_refused_clearly() {
    // A format-2 save is positional postcard: no magic.
    let old = [2u8, 6, b'1', b'0', b'1', b'p', b'e', b'r', 0, 1, 2, 3, 4, 5, 6, 7, 8];
    match SaveData::from_bytes(&old) {
        Err(WorldError::IncompatibleSave(msg)) => assert!(msg.contains("older incompatible build"), "{msg}"),
        other => panic!("expected IncompatibleSave, got {other:?}"),
    }
}

#[test]
fn legacy_unescapable_bool_loads_as_an_override() {
    // Saves from before the SetAreaUnescapable override stored a bool: TRUE
    // was a script-set flag (kept as an override), FALSE meant none was set.
    let Some(inst) = install() else { return };
    let mut a = World::new(inst.clone()).unwrap();
    a.enter_module("101per", None).unwrap();
    for legacy in [true, false] {
        let save = a.save();
        let mut tree: Value = ciborium::from_reader(save.state.as_slice()).unwrap();
        let top = map_mut(&mut tree);
        assert!(remove_key(top, "area_unescapable_override"), "state has no area_unescapable_override");
        top.push((Value::Text("area_unescapable".into()), Value::Bool(legacy)));
        let mut state = Vec::new();
        ciborium::into_writer(&tree, &mut state).unwrap();
        let edited = SaveData { version: save.version, module: save.module.clone(), state };
        let b = World::load(inst.clone(), &SaveData::from_bytes(&edited.to_bytes()).unwrap()).expect("legacy save must load");
        assert_eq!(b.area_unescapable_override, legacy.then_some(true));
    }
}

/// Every map in the tree that has a key with this text, visited recursively.
fn maps_with_key<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Map(m) => {
            if m.iter().any(|(k, _)| k.as_text() == Some(key)) {
                out.push(v);
            }
            for (_, child) in m {
                maps_with_key(child, key, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|c| maps_with_key(c, key, out)),
        _ => {}
    }
}

fn mentions_key(v: &Value, key: &str) -> bool {
    let mut out = Vec::new();
    maps_with_key(v, key, &mut out);
    !out.is_empty()
}

/// The follow latches (follow_stuck, follow_held) are serde(skip): a save never
/// writes them, and a save that carries them (an edited or older one) still loads
/// with them ignored.
#[test]
fn follow_latches_are_not_saved_and_are_ignored_when_present() {
    let Some(inst) = install() else { return };
    let mut a = World::new(inst.clone()).unwrap();
    a.enter_module("201tel", None).unwrap();
    for _ in 0..30 {
        a.tick(1.0 / 30.0);
        let _ = a.drain_events();
    }
    let pc = a.party_leader();
    let at = a.get(pc).unwrap().position;
    let kreia = a.spawn_creature("p_kreia", at + glam::Vec3::new(0.5, 0.0, 0.0), 0.0).unwrap();
    a.party.push(kreia);
    // Walk the companion so its path cache holds an entry to save.
    let goal = a.walk.ground(at.x + 4.0, at.y, at.z + 1.0).map(|h| glam::Vec3::new(at.x + 4.0, at.y, h.z));
    if let Some(goal) = goal {
        a.push_action(kreia, k2_world::object::Action::MoveTo { target: k2_world::object::Target::Point(goal), run: false, range: 0.5, timeout: None, elapsed: 0.0, stuck: 0.0 });
    }
    for _ in 0..30 {
        a.tick(1.0 / 30.0);
        let _ = a.drain_events();
    }
    let save = a.save();
    let mut tree: Value = ciborium::from_reader(save.state.as_slice()).unwrap();
    assert!(mentions_key(&tree, "points"), "no path cache in the save: the case proves nothing");
    assert!(!mentions_key(&tree, "follow_stuck") && !mentions_key(&tree, "follow_held"), "a follow latch was written to the save");

    // An edited save that carries both latches still loads.
    let mut caches = Vec::new();
    maps_with_key(&tree, "points", &mut caches);
    let count = caches.len();
    fn add_latches(v: &mut Value) {
        match v {
            Value::Map(m) => {
                if m.iter().any(|(k, _)| k.as_text() == Some("points")) {
                    m.push((Value::Text("follow_stuck".into()), Value::Float(3.0)));
                    m.push((Value::Text("follow_held".into()), Value::Float(9.0)));
                }
                for (_, child) in m.iter_mut() {
                    add_latches(child);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(add_latches),
            _ => {}
        }
    }
    add_latches(&mut tree);
    assert!(count >= 1);
    let mut state = Vec::new();
    ciborium::into_writer(&tree, &mut state).unwrap();
    let edited = SaveData { version: save.version, module: save.module.clone(), state };
    let mut loaded = World::load(inst, &SaveData::from_bytes(&edited.to_bytes()).unwrap()).expect("a save with latches must load");
    assert_eq!(loaded.party.len(), a.party.len());
    assert!(loaded.vm_errors.is_empty(), "{:?}", loaded.vm_errors);

    // The carried latches must be ignored. Had they been read, follow_held = 9.0 m
    // would hold the companion while the leader is within 10 m (9.0 m plus the
    // 1 m resume distance). The leader is placed about 8 m from where it stood,
    // so the companion starts 6 to 8 m away and would stand still.
    let leader = loaded.party_leader();
    let kreia_loaded = loaded.party.iter().copied().find(|&p| p != leader).expect("a companion after load");
    loaded.push_action(kreia_loaded, k2_world::object::Action::Follow { of: k2_world::object::FollowTarget::Leader, range: 2.0 });
    let at = loaded.get(leader).unwrap().position;
    let goal = (0..72)
        .find_map(|k| {
            let ang = k as f32 * std::f32::consts::TAU / 72.0;
            let q = at + glam::Vec3::new(ang.cos(), ang.sin(), 0.0) * 8.0;
            let h = loaded.walk.ground(q.x, q.y, q.z + 1.0)?;
            let p = glam::Vec3::new(q.x, q.y, h.z);
            loaded.walk.find_path(at, p, 0.3).is_some().then_some(p)
        })
        .expect("a walkable point 8 m from the leader");
    loaded.place(leader, goal);
    let before = loaded.get(kreia_loaded).unwrap().position;
    // It has to cover the gap down to its follow range (2.0 m, stopping near 2.5 m).
    let needed = (before - goal).truncate().length() - 2.5;
    for _ in 0..(12 * 30) {
        if loaded.conversation.is_some() {
            loaded.end_conversation(true);
        }
        loaded.tick(1.0 / 30.0);
        let _ = loaded.drain_events();
    }
    let gap = (loaded.get(kreia_loaded).unwrap().position - loaded.get(leader).unwrap().position).truncate().length();
    assert!(gap <= 2.6, "the follow after load stood {gap:.2} m from the leader: a carried latch held it");
    let after = loaded.get(kreia_loaded).unwrap().position;
    assert!(after.truncate().distance(before.truncate()) >= needed - 0.3, "the companion did not walk after load: moved {:.2} m of {needed:.2} m needed", after.truncate().distance(before.truncate()));
}
