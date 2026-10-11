//! Engine routines that act on the party, applied effects, weapon sets,
//! placeable heartbeats, follow actions and module transit: GetFirstPC /
//! GetNextPC, ClearAllEffects, RemoveEffectByID, RemoveEffectByExactMatch,
//! ActionSwitchWeapons, ActionEquipItem, RemoveHeartbeat, ActionFollowLeader,
//! ActionFollowOwner, ActionForceFollowObject, SetAreaUnescapable and
//! SetDisableTransit. Calls go
//! through the routine table and ScriptHost like a script does. Skips
//! (passes) when no install is found.

use glam::Vec3;
use k2_res::Install;
use k2_script::{Call, ObjectId, ScriptHost, Value, OBJECT_INVALID};
use k2_world::object::{Action, FollowTarget, Target};
use k2_world::{AvailableNpc, ObjectKind, World, WorldEvent};

const DT: f32 = 1.0 / 30.0;

fn install() -> Option<Install> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let found = Install::discover(&dir).ok();
    if found.is_none() {
        // Every test passes vacuously without an install; say so once, on stderr
        // (not captured by the test harness).
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "SKIP: no KotOR 2 install at {dir}; script_routines tests did not run");
        });
    }
    found
}

fn world(module: &str) -> Option<World> {
    let mut w = World::new(install()?).ok()?;
    w.enter_module(module, None).unwrap();
    for _ in 0..30 {
        w.tick(DT);
    }
    Some(w)
}

/// Calls an engine routine the way a script does (routine table + ScriptHost).
fn call(w: &mut World, name: &str, caller: ObjectId, args: &[Value]) -> Value {
    let routines = w.routines.clone();
    let routine = routines.by_name(name).unwrap_or_else(|| panic!("no routine {name}"));
    let script = k2_script::load_ncs(&w.res, "k_trp_generic").expect("a script to host the call");
    w.call(Call { caller, script: &script, routine, args })
}

fn object(v: Value) -> ObjectId {
    match v {
        Value::Object(o) => o,
        other => panic!("expected an object, got {other:?}"),
    }
}

fn int(v: Value) -> i32 {
    match v {
        Value::Int(i) => i,
        other => panic!("expected an int, got {other:?}"),
    }
}

fn effect_count(w: &World, id: ObjectId) -> usize {
    w.get(id).unwrap().effects.len()
}

#[test]
fn get_next_pc_walks_the_party_then_invalid() {
    let Some(mut w) = world("103per") else { return };
    let pc = w.player;
    w.available_npcs.insert(6, AvailableNpc { template: "p_kreia".into(), selectable: true, ..Default::default() });
    w.available_npcs.insert(0, AvailableNpc { template: "p_atton".into(), selectable: true, ..Default::default() });
    w.apply_party_selection(&[6, 0]).unwrap();
    let party = w.party.clone();
    assert_eq!(party.len(), 3);

    let mut seen = vec![object(call(&mut w, "GetFirstPC", pc, &[]))];
    for _ in 0..3 {
        seen.push(object(call(&mut w, "GetNextPC", pc, &[])));
    }
    assert_eq!(seen, vec![party[0], party[1], party[2], OBJECT_INVALID]);

    // GetFirstPC restarts the walk.
    assert_eq!(object(call(&mut w, "GetFirstPC", pc, &[])), party[0]);
    assert_eq!(object(call(&mut w, "GetNextPC", pc, &[])), party[1]);
}

#[test]
fn effect_removal_routines_match_by_caller_type_and_content() {
    let Some(mut w) = world("101per") else { return };
    let pc = w.player;
    let temp = Value::Int(w.constant("DURATION_TYPE_TEMPORARY").unwrap());
    let sleep = call(&mut w, "EffectSleep", pc, &[]);
    let paralyze = call(&mut w, "EffectParalyze", pc, &[]);
    for eff in [&sleep, &paralyze] {
        call(&mut w, "ApplyEffectToObject", pc, &[temp.clone(), eff.clone(), Value::Object(pc), Value::Float(30.0)]);
    }
    assert_eq!(effect_count(&w, pc), 2);

    // RemoveEffectByID drops every effect of that type and nothing else.
    let sleep_id = w.constant("EFFECT_TYPE_SLEEP").unwrap();
    call(&mut w, "RemoveEffectByID", pc, &[Value::Object(pc), Value::Int(sleep_id)]);
    assert_eq!(effect_count(&w, pc), 1);

    // RemoveEffectByExactMatch takes a fresh value of the same type.
    let fresh = call(&mut w, "EffectParalyze", pc, &[]);
    call(&mut w, "RemoveEffectByExactMatch", pc, &[Value::Object(pc), fresh]);
    assert_eq!(effect_count(&w, pc), 0);

    // ClearAllEffects clears the caller's effects.
    for eff in [&sleep, &paralyze] {
        call(&mut w, "ApplyEffectToObject", pc, &[temp.clone(), eff.clone(), Value::Object(pc), Value::Float(30.0)]);
    }
    assert_eq!(effect_count(&w, pc), 2);
    call(&mut w, "ClearAllEffects", pc, &[]);
    assert_eq!(effect_count(&w, pc), 0);
}

#[test]
fn removed_effects_take_their_spell_and_absorb_entries_with_them() {
    let Some(mut w) = world("101per") else { return };
    let pc = w.player;
    let temp = Value::Int(w.constant("DURATION_TYPE_TEMPORARY").unwrap());
    for name in ["EffectSleep", "EffectParalyze"] {
        let eff = call(&mut w, name, pc, &[]);
        call(&mut w, "ApplyEffectToObject", pc, &[temp.clone(), eff, Value::Object(pc), Value::Float(30.0)]);
    }
    let handles: Vec<u32> = w.get(pc).unwrap().effects.iter().map(|e| e.handle).collect();
    assert_eq!(handles.len(), 2);
    for &h in &handles {
        w.powers.effect_spell.insert(h, 7);
        w.powers.absorb_left.insert(h, 5);
    }

    // RemoveEffectByID takes sleep (applied first) and its entries, not paralyze's.
    let sleep_id = w.constant("EFFECT_TYPE_SLEEP").unwrap();
    call(&mut w, "RemoveEffectByID", pc, &[Value::Object(pc), Value::Int(sleep_id)]);
    assert!(!w.powers.effect_spell.contains_key(&handles[0]), "sleep kept its spell id");
    assert!(!w.powers.absorb_left.contains_key(&handles[0]), "sleep kept its absorb pool");
    assert!(w.powers.effect_spell.contains_key(&handles[1]), "paralyze lost its spell id");
    assert!(w.powers.absorb_left.contains_key(&handles[1]), "paralyze lost its absorb pool");

    call(&mut w, "ClearAllEffects", pc, &[]);
    assert!(!w.powers.effect_spell.contains_key(&handles[1]), "ClearAllEffects kept a spell id");
    assert!(!w.powers.absorb_left.contains_key(&handles[1]), "ClearAllEffects kept an absorb pool");
}

#[test]
fn remove_effect_drops_its_side_table_entries() {
    let Some(mut w) = world("101per") else { return };
    let pc = w.player;
    let temp = Value::Int(w.constant("DURATION_TYPE_TEMPORARY").unwrap());
    let sleep = call(&mut w, "EffectSleep", pc, &[]);
    let paralyze = call(&mut w, "EffectParalyze", pc, &[]);
    for eff in [&sleep, &paralyze] {
        call(&mut w, "ApplyEffectToObject", pc, &[temp.clone(), eff.clone(), Value::Object(pc), Value::Float(30.0)]);
    }
    let handles: Vec<u32> = w.get(pc).unwrap().effects.iter().map(|e| e.handle).collect();
    for &h in &handles {
        w.powers.effect_spell.insert(h, 7);
        w.powers.absorb_left.insert(h, 5);
    }

    call(&mut w, "RemoveEffect", pc, &[Value::Object(pc), sleep]);
    assert_eq!(effect_count(&w, pc), 1);
    assert!(!w.powers.effect_spell.contains_key(&handles[0]), "RemoveEffect kept the spell id");
    assert!(!w.powers.absorb_left.contains_key(&handles[0]), "RemoveEffect kept the absorb pool");
    assert!(w.powers.effect_spell.contains_key(&handles[1]) && w.powers.absorb_left.contains_key(&handles[1]));
}

#[test]
fn absorbed_temporary_hitpoints_drop_their_side_table_entries() {
    let Some(mut w) = world("101per") else { return };
    let pc = w.player;
    let temp = Value::Int(w.constant("DURATION_TYPE_TEMPORARY").unwrap());
    let instant = Value::Int(w.constant("DURATION_TYPE_INSTANT").unwrap());
    let thp = call(&mut w, "EffectTemporaryHitpoints", pc, &[Value::Int(5)]);
    let handle = match &thp {
        Value::Engine(e) => e.handle,
        other => panic!("expected an engine effect, got {other:?}"),
    };
    call(&mut w, "ApplyEffectToObject", pc, &[temp, thp, Value::Object(pc), Value::Float(30.0)]);
    let hp = w.get(pc).unwrap().creature().unwrap().hp;

    // 10 damage: the 5 temporary points absorb half, the rest comes off hit points.
    let hit = call(&mut w, "EffectDamage", pc, &[Value::Int(10), Value::Int(0)]);
    call(&mut w, "ApplyEffectToObject", pc, &[instant, hit, Value::Object(pc), Value::Float(0.0)]);
    assert_eq!(w.get(pc).unwrap().creature().unwrap().hp, hp - 5, "the pool did not absorb 5");
    assert_eq!(effect_count(&w, pc), 0, "the spent effect stayed applied");
    assert!(!w.powers.absorb_left.contains_key(&handle), "a spent absorb pool stayed in absorb_left");
}

#[test]
fn turbolift_in_unescapable_101per_leads_to_105per() {
    // 101per's ARE sets Unescapable, and no script clears it. Transit is
    // refused only by SetDisableTransit, so the turbolift still works.
    let Some(mut w) = world("101per") else { return };
    let pc = w.player;
    assert_eq!(int(call(&mut w, "GetAreaUnescapable", pc, &[])), 1);
    let lift = w
        .iter()
        .find(|o| o.door().is_some_and(|d| d.linked_to_module.as_deref().is_some_and(|m| m.eq_ignore_ascii_case("105per"))))
        .map(|o| o.id)
        .expect("the turbolift door linked to 105per");
    call(&mut w, "SetLocked", pc, &[Value::Object(lift), Value::Int(0)]);
    w.use_object(pc, lift);
    let t = w.pending_transition.as_ref().expect("the turbolift did not transit");
    assert_eq!(t.module, "105per");
}

#[test]
fn exact_match_removal_needs_every_integer_to_match() {
    let Some(mut w) = world("101per") else { return };
    let pc = w.player;
    let temp = Value::Int(w.constant("DURATION_TYPE_TEMPORARY").unwrap());
    let strength = Value::Int(w.constant("ABILITY_STRENGTH").unwrap());
    let plus2 = call(&mut w, "EffectAbilityIncrease", pc, &[strength.clone(), Value::Int(2)]);
    call(&mut w, "ApplyEffectToObject", pc, &[temp, plus2, Value::Object(pc), Value::Float(30.0)]);
    assert_eq!(effect_count(&w, pc), 1);

    // Same type and ability, other amount: Integer(1) differs, nothing goes.
    let plus3 = call(&mut w, "EffectAbilityIncrease", pc, &[strength.clone(), Value::Int(3)]);
    call(&mut w, "RemoveEffectByExactMatch", pc, &[Value::Object(pc), plus3]);
    assert_eq!(effect_count(&w, pc), 1, "a mismatched Integer(1) removed the effect");

    // The same values do remove it.
    let plus2_again = call(&mut w, "EffectAbilityIncrease", pc, &[strength, Value::Int(2)]);
    call(&mut w, "RemoveEffectByExactMatch", pc, &[Value::Object(pc), plus2_again]);
    assert_eq!(effect_count(&w, pc), 0);
}

#[test]
fn switch_weapons_action_swaps_the_sets_when_reached() {
    // The swap is skipped during dialog, so end the entry conversation first.
    let Some(mut w) = world("103per") else { return };
    let pc = w.party_leader();
    if w.conversation_view().is_some() {
        w.end_conversation(true);
    }
    assert!(w.conversation_view().is_none(), "a conversation is still running");
    let blade = w.create_item("g_w_lghtsbr01", pc, 1).expect("a lightsaber");
    w.equip(pc, blade, 4).unwrap();
    let staff = w.create_item("g_w_dblsbr001", pc, 1).expect("a double-bladed lightsaber");
    w.equip(pc, staff, 18).unwrap();
    assert_eq!(w.weapon_set(pc), 0);
    assert!(w.other_weapon_set_armed(pc), "the other set must be armed before the switch");

    // Queued like any action: the swap happens when it reaches the front.
    call(&mut w, "ClearAllActions", pc, &[]);
    call(&mut w, "ActionSwitchWeapons", pc, &[]);
    assert_eq!(w.weapon_set(pc), 0);
    w.tick(DT);
    assert_eq!(w.weapon_set(pc), 1);
    assert_eq!(w.weapon_set_slots(pc, 1), (4, 5), "the staff now sits in the active pair");

    call(&mut w, "ActionSwitchWeapons", pc, &[]);
    w.tick(DT);
    assert_eq!(w.weapon_set(pc), 0);
}

#[test]
fn switch_weapons_with_an_empty_other_set_does_nothing() {
    // The AI's switch never leaves the creature unarmed: with only set 1 armed
    // there is no set to swap to.
    let Some(mut w) = world("103per") else { return };
    let pc = w.party_leader();
    if w.conversation_view().is_some() {
        w.end_conversation(true);
    }
    let blade = w.create_item("g_w_lghtsbr01", pc, 1).expect("a lightsaber");
    w.equip(pc, blade, 4).unwrap();

    call(&mut w, "ClearAllActions", pc, &[]);
    call(&mut w, "ActionSwitchWeapons", pc, &[]);
    w.tick(DT);
    assert_eq!(w.weapon_set(pc), 0, "swapped to an empty set");
    assert_eq!(w.weapon_set_slots(pc, 0), (4, 5));
    assert!(w.equipment(pc).iter().any(|&(s, i)| s == 4 && i == blade), "the blade stays in hand");
}

#[test]
fn switch_weapons_during_dialog_leaves_the_sets_alone() {
    let Some(mut w) = world("103per") else { return };
    let pc = w.party_leader();
    assert!(w.conversation_view().is_some(), "103per should open in a conversation");
    let blade = w.create_item("g_w_lghtsbr01", pc, 1).expect("a lightsaber");
    w.equip(pc, blade, 4).unwrap();
    let slots = w.weapon_set_slots(pc, 1);

    call(&mut w, "ClearAllActions", pc, &[]);
    call(&mut w, "ActionSwitchWeapons", pc, &[]);
    w.tick(DT);
    assert!(w.conversation_view().is_some(), "the conversation ended, so the test proves nothing");
    assert_eq!(w.weapon_set(pc), 0, "the set changed during dialog");
    assert_eq!(w.weapon_set_slots(pc, 1), slots);
    assert!(w.get(pc).unwrap().actions.is_empty(), "the action stayed queued");
}

#[test]
fn scripted_equip_and_unequip_go_through_equip() {
    // inventory.rs handles ActionEquipItem / ActionUnequipItem through equip()
    // and unequip(): the slot change is announced and the draw starts.
    let Some(mut w) = world("101per") else { return };
    let pc = w.player;
    let blade = w.create_item("g_w_lghtsbr01", pc, 1).expect("a lightsaber");
    let _ = w.drain_events();

    call(&mut w, "ActionEquipItem", pc, &[Value::Object(blade), Value::Int(4)]);
    assert!(w.equipment(pc).contains(&(4, blade)), "the blade is not in slot 4");
    let announced = w.drain_events().iter().any(|e| matches!(e, WorldEvent::EquipmentChanged { creature, slot: 4 } if *creature == pc));
    assert!(announced, "no EquipmentChanged for slot 4");
    assert!(w.weapon_busy(pc) > 0.0, "the draw did not start");

    call(&mut w, "ActionUnequipItem", pc, &[Value::Object(blade)]);
    assert!(!w.equipment(pc).iter().any(|&(_, i)| i == blade), "the blade is still equipped");
    let announced = w.drain_events().iter().any(|e| matches!(e, WorldEvent::EquipmentChanged { creature, slot: 4 } if *creature == pc));
    assert!(announced, "no EquipmentChanged for the unequip");
}

/// Ticks `secs` of game time. Followers hold still during dialog, so any
/// conversation the module opens is ended first.
fn settle(w: &mut World, secs: f32) {
    for _ in 0..(secs / DT) as usize {
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
        w.tick(DT);
    }
}

/// A walkable point `dist` m from `from` that has a route back to it.
fn walkable_near(w: &World, from: Vec3, dist: f32) -> Vec3 {
    for k in 0..72 {
        for r in [dist, dist * 0.9, dist * 1.1] {
            let a = k as f32 * std::f32::consts::TAU / 72.0;
            let q = from + Vec3::new(a.cos(), a.sin(), 0.0) * r;
            let Some(h) = w.walk.ground(q.x, q.y, q.z + 1.0) else { continue };
            let p = Vec3::new(q.x, q.y, h.z);
            if w.walk.find_path(from, p, 0.3).is_some() {
                return p;
            }
        }
    }
    panic!("no walkable point {dist} m from {from:?}");
}

fn gap(w: &World, follower: ObjectId, leader: ObjectId) -> f32 {
    (w.get(follower).unwrap().position - w.get(leader).unwrap().position).truncate().length()
}

#[test]
fn follow_leader_walks_after_the_leader_and_yields_to_the_next_action() {
    let Some(mut w) = world("201tel") else { return };
    let pc = w.party_leader();
    let start = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", start + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    w.party.push(kreia);
    call(&mut w, "ActionFollowLeader", kreia, &[]);

    let goal = walkable_near(&w, start, 10.0);
    w.place(pc, goal);
    settle(&mut w, 12.0);
    assert!(gap(&w, kreia, pc) <= 2.5, "follower {:.2} m behind", gap(&w, kreia, pc));
    assert!(w.get(kreia).unwrap().position.distance(start) > 5.0, "follower did not move");
    assert_eq!(int(call(&mut w, "GetCurrentAction", kreia, &[])), w.constant("ACTION_FOLLOWLEADER").unwrap());

    // Working interpretation: an action queued behind the leader follow makes
    // it yield, and the queued action runs.
    w.push_action(kreia, Action::Wait { remaining: 0.2 });
    w.tick(DT);
    let queue = &w.get(kreia).unwrap().actions;
    assert_eq!(queue.len(), 1, "the follow did not yield to the queued wait");
    assert!(matches!(queue.front(), Some(Action::Wait { .. })), "the wait is not running");
}

#[test]
fn follow_owner_holds_the_queue_and_keeps_its_range() {
    let Some(mut w) = world("201tel") else { return };
    let pc = w.party_leader();
    let start = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", start + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");

    // No owner: nothing to follow, but the action still holds the queue.
    call(&mut w, "ActionFollowOwner", kreia, &[Value::Float(2.5)]);
    w.push_action(kreia, Action::Wait { remaining: 0.1 });
    settle(&mut w, 1.0);
    assert_eq!(w.get(kreia).unwrap().actions.len(), 2, "an ownerless follow let the queue move");
    assert!(matches!(w.get(kreia).unwrap().actions.front(), Some(Action::Follow { .. })));
    call(&mut w, "ClearAllActions", kreia, &[]);
    assert!(w.get(kreia).unwrap().actions.is_empty());

    // The owner link is what GetPUPOwner reports, and what the follow walks after.
    w.get_mut(kreia).unwrap().owner = Some(pc);
    assert_eq!(object(call(&mut w, "GetPUPOwner", kreia, &[Value::Object(kreia)])), pc);
    call(&mut w, "ActionFollowOwner", kreia, &[Value::Float(3.0)]);
    let goal = walkable_near(&w, start, 9.0);
    w.place(pc, goal);
    settle(&mut w, 12.0);
    assert!(gap(&w, kreia, pc) <= 3.5, "follower {:.2} m from its owner", gap(&w, kreia, pc));
    assert!(w.get(kreia).unwrap().position.distance(start) > 5.0, "follower did not move");
    assert_eq!(int(call(&mut w, "GetCurrentAction", kreia, &[])), w.constant("ACTION_FOLLOWOWNER").unwrap());

    // Still holding after the walk: a queued wait does not run.
    w.push_action(kreia, Action::Wait { remaining: 0.1 });
    settle(&mut w, 1.0);
    assert_eq!(w.get(kreia).unwrap().actions.len(), 2, "the owner follow yielded to the queue");
    call(&mut w, "ClearAllActions", kreia, &[]);
}

#[test]
fn follow_force_walks_to_the_object_and_holds_the_queue() {
    let Some(mut w) = world("201tel") else { return };
    let pc = w.party_leader();
    let start = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", start + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    call(&mut w, "ActionForceFollowObject", kreia, &[Value::Object(pc), Value::Float(3.0)]);

    let goal = walkable_near(&w, start, 10.0);
    w.place(pc, goal);
    settle(&mut w, 12.0);
    assert!(gap(&w, kreia, pc) <= 3.5, "follower {:.2} m from the object", gap(&w, kreia, pc));
    assert!(w.get(kreia).unwrap().position.distance(start) > 5.0, "follower did not move");
    assert_eq!(int(call(&mut w, "GetCurrentAction", kreia, &[])), w.constant("ACTION_FOLLOW").unwrap());

    // Like ActionFollowOwner, a queued action waits until ClearAllActions.
    w.push_action(kreia, Action::Wait { remaining: 0.1 });
    settle(&mut w, 1.0);
    assert_eq!(w.get(kreia).unwrap().actions.len(), 2, "the forced follow yielded to the queue");
    call(&mut w, "ClearAllActions", kreia, &[]);
}

/// Heartbeat scripts repeat their follow call every few seconds without
/// ClearAllActions. Each repeat must be a no-op, not a new queue entry.
#[test]
fn repeated_follow_calls_do_not_stack() {
    let Some(mut w) = world("201tel") else { return };
    let pc = w.party_leader();
    let at = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", at, 0.0).expect("p_kreia");
    // Ten calls with no tick between them: a leader follow yields as soon as
    // another entry sits behind it, so stacking shows before any tick runs.
    for _ in 0..10 {
        call(&mut w, "ActionFollowLeader", kreia, &[]);
    }
    assert_eq!(w.get(kreia).unwrap().actions.len(), 1, "leader follows stacked");
    settle(&mut w, 1.0);
    assert_eq!(w.get(kreia).unwrap().actions.len(), 1, "a leader follow left the queue after a tick");

    // The same holds for the blocking follows, which keep their first range.
    call(&mut w, "ClearAllActions", kreia, &[]);
    for _ in 0..10 {
        call(&mut w, "ActionFollowOwner", kreia, &[Value::Float(2.5)]);
        call(&mut w, "ActionFollowOwner", kreia, &[Value::Float(9.0)]);
        call(&mut w, "ActionForceFollowObject", kreia, &[Value::Object(pc), Value::Float(3.0)]);
        settle(&mut w, 0.5);
    }
    let queue = &w.get(kreia).unwrap().actions;
    assert_eq!(queue.len(), 2, "owner and forced follows stacked: {queue:?}");
    assert!(matches!(queue.front(), Some(Action::Follow { range, .. }) if *range == 2.5), "the first owner range was replaced");
    call(&mut w, "ClearAllActions", kreia, &[]);
}

#[test]
fn remove_heartbeat_drops_the_placeable_script() {
    let Some(mut w) = world("101per") else { return };
    let plc = w
        .iter()
        .find(|o| o.kind == ObjectKind::Placeable)
        .map(|o| o.id)
        .expect("a placeable in 101per");
    w.get_mut(plc).unwrap().scripts.insert("OnHeartbeat".into(), "k_plc_treas_disp".into());
    assert!(w.get(plc).unwrap().script("OnHeartbeat").is_some());
    let pc = w.player;
    call(&mut w, "RemoveHeartbeat", pc, &[Value::Object(plc)]);
    assert!(w.get(plc).unwrap().script("OnHeartbeat").is_none());
}

#[test]
fn disable_transit_refuses_linked_doors_until_cleared() {
    let Some(mut w) = world("101per") else { return };
    let pc = w.player;
    let door = w
        .iter()
        .find(|o| o.door().is_some_and(|d| d.linked_to_module.as_deref().is_some_and(|m| !m.is_empty())))
        .map(|o| o.id)
        .expect("a door linked to another module in 101per");
    call(&mut w, "SetLocked", pc, &[Value::Object(door), Value::Int(0)]);

    // 101per's ARE sets Unescapable, yet its turbolift works: only
    // SetDisableTransit refuses transit.
    w.use_object(pc, door);
    assert!(w.pending_transition.is_some(), "the ARE Unescapable flag refused the turbolift");
    w.pending_transition = None;

    call(&mut w, "SetAreaUnescapable", pc, &[Value::Int(1)]);
    w.use_object(pc, door);
    assert!(w.pending_transition.is_some(), "SetAreaUnescapable refused transit");
    w.pending_transition = None;

    // SetDisableTransit refuses, and clearing it allows again.
    call(&mut w, "SetDisableTransit", pc, &[Value::Int(1)]);
    w.use_object(pc, door);
    assert!(w.pending_transition.is_none(), "transit disabled but the party left");
    call(&mut w, "SetDisableTransit", pc, &[Value::Int(0)]);
    w.use_object(pc, door);
    assert!(w.pending_transition.is_some());
    w.pending_transition = None;

    // Entering a module resets both flags. 201tel's ARE is not Unescapable,
    // and nothing is disabled there afterwards.
    call(&mut w, "SetAreaUnescapable", pc, &[Value::Int(1)]);
    call(&mut w, "SetDisableTransit", pc, &[Value::Int(1)]);
    w.enter_module("201tel", None).unwrap();
    assert_eq!(w.area_unescapable_override, None);
    assert!(!w.transit_disabled);
    assert!(!w.transit_blocked());
}

#[test]
fn get_area_unescapable_is_the_override_then_the_are_flag() {
    // 101per's ARE sets Unescapable. A script's FALSE overrides that flag.
    let Some(mut w) = world("101per") else { return };
    let pc = w.player;
    assert!(w.area_unescapable_override.is_none());
    assert_eq!(int(call(&mut w, "GetAreaUnescapable", pc, &[])), 1, "101per's ARE flag not seen");

    call(&mut w, "SetAreaUnescapable", pc, &[Value::Int(0)]);
    assert_eq!(int(call(&mut w, "GetAreaUnescapable", pc, &[])), 0, "FALSE did not override the ARE flag");

    // The override is saved and loaded with the module.
    let save = w.save();
    let loaded = World::load(install().unwrap(), &save).expect("save loads");
    assert_eq!(loaded.area_unescapable_override, Some(false));
    assert!(!loaded.area_unescapable());
}

#[test]
fn start_new_module_works_while_transit_is_disabled() {
    let Some(mut w) = world("101per") else { return };
    let pc = w.player;
    call(&mut w, "SetAreaUnescapable", pc, &[Value::Int(1)]);
    call(&mut w, "SetDisableTransit", pc, &[Value::Int(1)]);
    assert!(w.transit_blocked());
    call(&mut w, "StartNewModule", pc, &[Value::String("201tel".into()), Value::String("".into())]);
    assert_eq!(w.pending_transition.as_ref().map(|t| t.module.as_str()), Some("201tel"), "StartNewModule was refused");
}

#[test]
fn a_companion_heartbeat_follow_keeps_the_party_settled() {
    // k_ai_master queues ActionFollowLeader on every companion, so a companion
    // is idle while its queue holds only follows.
    let Some(mut w) = world("201tel") else { return };
    settle(&mut w, 0.5);
    let pc = w.party_leader();
    let at = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", at + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    w.party.push(kreia);
    assert!(w.settled(), "the party is not settled before the companion follows");
    call(&mut w, "ActionFollowLeader", kreia, &[]);
    settle(&mut w, 0.5);
    assert!(w.settled(), "a companion's heartbeat follow unsettled the party");

    // A real action on a companion still counts.
    w.push_action(kreia, Action::Wait { remaining: 1.0 });
    assert!(!w.settled(), "a queued wait did not unsettle the party");
}

#[test]
fn leader_follow_on_the_party_leader_is_done_at_once() {
    let Some(mut w) = world("201tel") else { return };
    settle(&mut w, 0.5);
    let pc = w.party_leader();
    call(&mut w, "ActionFollowLeader", pc, &[]);
    w.tick(DT);
    assert!(w.get(pc).unwrap().actions.is_empty(), "the leader's own follow stayed queued");
    assert!(w.player_controlled(), "the leader's own follow locked player control");
}

#[test]
fn leader_follow_queued_before_a_wait_resumes_after_it() {
    // A follow queued behind a wait is kept even though an earlier follow sits
    // in the queue: the earlier one yields to the wait, and this one resumes.
    let Some(mut w) = world("201tel") else { return };
    settle(&mut w, 0.5);
    let pc = w.party_leader();
    let at = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", at + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    call(&mut w, "ActionFollowLeader", kreia, &[]);
    w.push_action(kreia, Action::Wait { remaining: 0.2 });
    call(&mut w, "ActionFollowLeader", kreia, &[]);
    assert_eq!(w.get(kreia).unwrap().actions.len(), 3, "the repeat was not queued behind the wait");
    settle(&mut w, 1.0);
    let queue = &w.get(kreia).unwrap().actions;
    assert_eq!(queue.len(), 1, "the follow did not resume after the wait");
    assert!(matches!(queue.front(), Some(Action::Follow { of: FollowTarget::Leader, .. })));
}

#[test]
fn pup_owner_must_be_in_the_party() {
    // nwscript 841: the owner must exist and be in the party.
    let Some(mut w) = world("201tel") else { return };
    let pc = w.party_leader();
    let at = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", at + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    let atton = w.spawn_creature("p_atton", at + Vec3::new(0.0, 0.5, 0.0), 0.0).expect("p_atton");

    w.get_mut(kreia).unwrap().owner = Some(pc);
    assert_eq!(object(call(&mut w, "GetPUPOwner", kreia, &[Value::Object(kreia)])), pc, "a party owner was not found");
    w.get_mut(kreia).unwrap().owner = Some(atton);
    assert_eq!(object(call(&mut w, "GetPUPOwner", kreia, &[Value::Object(kreia)])), OBJECT_INVALID, "a non-party owner was found");
}

#[test]
fn explicit_follow_range_is_kept_including_zero() {
    // The VM fills nwscript's default (2.5) for a call that omits the argument,
    // so a 0.0 here is an explicit zero and must stay zero.
    let Some(mut w) = world("201tel") else { return };
    let pc = w.party_leader();
    let at = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", at + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    call(&mut w, "ActionFollowOwner", kreia, &[Value::Float(0.0)]);
    assert!(matches!(w.get(kreia).unwrap().actions.front(), Some(Action::Follow { range, .. }) if *range == 0.0), "an explicit 0.0 range was replaced");
    call(&mut w, "ClearAllActions", kreia, &[]);
    call(&mut w, "ActionFollowOwner", kreia, &[Value::Float(2.5)]);
    assert!(matches!(w.get(kreia).unwrap().actions.front(), Some(Action::Follow { range, .. }) if *range == 2.5));
}

#[test]
fn an_expired_shared_effect_keeps_side_tables_while_another_copy_lasts() {
    // One engine effect applied to two objects shares its handle. Expiry of one
    // copy must not drop the spell id or absorb pool the other copy still uses.
    let Some(mut w) = world("201tel") else { return };
    settle(&mut w, 0.5);
    let pc = w.party_leader();
    let at = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", at + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    let temp = Value::Int(w.constant("DURATION_TYPE_TEMPORARY").unwrap());
    let sleep = call(&mut w, "EffectSleep", pc, &[]);
    call(&mut w, "ApplyEffectToObject", pc, &[temp.clone(), sleep.clone(), Value::Object(pc), Value::Float(1.0)]);
    call(&mut w, "ApplyEffectToObject", pc, &[temp, sleep, Value::Object(kreia), Value::Float(30.0)]);
    let handle = w.get(pc).unwrap().effects[0].handle;
    assert_eq!(w.get(kreia).unwrap().effects[0].handle, handle, "the copies do not share a handle");
    w.powers.effect_spell.insert(handle, 7);
    w.powers.absorb_left.insert(handle, 5);

    settle(&mut w, 2.0);
    assert!(w.get(pc).unwrap().effects.is_empty(), "the short copy did not expire");
    assert_eq!(w.get(kreia).unwrap().effects.len(), 1, "the long copy was removed");
    assert!(w.powers.effect_spell.contains_key(&handle), "expiry dropped a live copy's spell id");
    assert!(w.powers.absorb_left.contains_key(&handle), "expiry dropped a live copy's absorb pool");

    settle(&mut w, 30.0);
    assert!(w.get(kreia).unwrap().effects.is_empty(), "the long copy did not expire");
    assert!(!w.powers.effect_spell.contains_key(&handle), "the last copy's spell id stayed");
    assert!(!w.powers.absorb_left.contains_key(&handle), "the last copy's absorb pool stayed");
}

#[test]
fn leader_switch_leaves_each_new_leader_controllable() {
    // Companions hold heartbeat follows (k_ai_master). After a switch the new
    // leader must still take control and accept a move, every time.
    let Some(mut w) = world("201tel") else { return };
    settle(&mut w, 0.5);
    let pc = w.party_leader();
    let at = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", at + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    let atton = w.spawn_creature("p_atton", at + Vec3::new(0.0, 0.5, 0.0), 0.0).expect("p_atton");
    w.party.push(kreia);
    w.party.push(atton);
    settle(&mut w, 10.0);

    for new in [kreia, atton] {
        assert!(w.set_party_leader(new), "the switch to {new} was refused");
        // Heartbeats keep running with the new leader in place.
        settle(&mut w, 10.0);
        assert_eq!(w.party_leader(), new);
        assert!(!w.get(new).unwrap().actions.iter().any(|a| matches!(a, Action::Follow { of: FollowTarget::Leader, .. })), "the leader holds a follow of itself");
        assert!(w.player_controlled(), "the new leader {new} is not controllable");
        assert!(w.settled(), "the new leader {new} does not settle");

        let before = w.get(new).unwrap().position;
        let goal = walkable_near(&w, before, 4.0);
        w.push_action(new, Action::MoveTo { target: Target::Point(goal), run: false, range: 0.5, timeout: None, elapsed: 0.0, stuck: 0.0 });
        settle(&mut w, 3.0);
        assert!(w.get(new).unwrap().position.distance(before) > 1.0, "the new leader {new} did not move");
    }
}

#[test]
fn every_leader_change_leaves_the_new_leader_with_an_empty_queue() {
    // A follow on the leader is refused, and switching (portrait, Tab) clears
    // the new leader's queue, including heartbeat follows.
    let Some(mut w) = world("201tel") else { return };
    settle(&mut w, 0.5);
    let pc = w.party_leader();
    let at = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", at + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    let atton = w.spawn_creature("p_atton", at + Vec3::new(0.0, 0.5, 0.0), 0.0).expect("p_atton");
    w.party.push(kreia);
    w.party.push(atton);

    call(&mut w, "ActionFollowLeader", pc, &[]);
    assert!(w.get(pc).unwrap().actions.is_empty(), "the leader queued a follow on itself");
    call(&mut w, "ActionFollowLeader", kreia, &[]);
    call(&mut w, "ActionFollowLeader", atton, &[]);

    assert!(w.set_party_leader(kreia));
    assert!(w.get(kreia).unwrap().actions.is_empty(), "the portrait switch kept the queue");
    assert!(w.player_controlled());

    // The old leader is now a companion and may queue its follow again.
    call(&mut w, "ActionFollowLeader", pc, &[]);
    assert!(w.cycle_party_leader().is_some(), "Tab did not rotate the party");
    let leader = w.party_leader();
    assert!(w.get(leader).unwrap().actions.is_empty(), "Tab kept the new leader's queue");
    assert!(w.player_controlled(), "Tab left the new leader uncontrollable");
}

/// Kreia joins through the party selection (NPC 6), as the revival tests do.
fn join_kreia(w: &mut World) -> ObjectId {
    w.available_npcs.insert(6, AvailableNpc { template: "p_kreia".into(), selectable: true, ..Default::default() });
    w.apply_party_selection(&[6]).unwrap();
    w.party.iter().copied().find(|&p| p != w.party_leader()).expect("Kreia joined the party")
}

/// A companion queue of a heartbeat follow of the leader, then a wait.
fn follow_then_wait(w: &mut World, id: ObjectId) {
    call(w, "ActionFollowLeader", id, &[]);
    w.push_action(id, Action::Wait { remaining: 5.0 });
}

fn has_leader_follow(w: &World, id: ObjectId) -> bool {
    w.get(id).unwrap().actions.iter().any(|a| matches!(a, Action::Follow { of: FollowTarget::Leader, .. }))
}

fn has_wait(w: &World, id: ObjectId) -> bool {
    w.get(id).unwrap().actions.iter().any(|a| matches!(a, Action::Wait { .. }))
}

#[test]
fn party_member_down_drops_only_the_new_leaders_leader_follow() {
    let Some(mut w) = world("102per") else { return };
    let pc = w.party_leader();
    let kreia = join_kreia(&mut w);
    follow_then_wait(&mut w, kreia);
    assert!(has_leader_follow(&w, kreia) && has_wait(&w, kreia));
    w.hurt_creature(pc, 10_000, kreia);
    assert_eq!(w.party_leader(), kreia, "the fallen leader did not hand over");
    assert!(!has_leader_follow(&w, kreia), "the new leader kept its leader follow");
    assert!(has_wait(&w, kreia), "the hand-over dropped the new leader's wait");
}

#[test]
fn loaded_party_hand_over_drops_only_the_leader_follow() {
    let Some(mut w) = world("102per") else { return };
    let pc = w.party_leader();
    let kreia = join_kreia(&mut w);
    follow_then_wait(&mut w, kreia);
    {
        let c = w.get_mut(pc).and_then(|o| o.creature_mut()).unwrap();
        c.hp = 0;
        c.dead = true;
    }
    let data = w.save();
    let loaded = World::load(install().unwrap(), &data).expect("load");
    assert_eq!(loaded.party_leader(), kreia, "the fallen leader was not replaced on load");
    assert!(!has_leader_follow(&loaded, kreia), "the load hand-over kept the leader follow");
    assert!(has_wait(&loaded, kreia), "the load hand-over dropped the wait");
}

#[test]
fn switch_to_npc_drops_only_the_leader_follow() {
    let Some(mut w) = world("102per") else { return };
    let kreia = join_kreia(&mut w);
    follow_then_wait(&mut w, kreia);
    call(&mut w, "SwitchPlayerCharacter", kreia, &[Value::Int(6)]);
    assert_eq!(w.party_leader(), kreia, "SwitchPlayerCharacter(NPC) did not take control");
    assert!(!has_leader_follow(&w, kreia), "the switch to an NPC kept the leader follow");
    assert!(has_wait(&w, kreia), "the switch to an NPC dropped the wait");
}

#[test]
fn switch_back_to_the_pc_drops_only_the_leader_follow() {
    let Some(mut w) = world("102per") else { return };
    let pc = w.party_leader();
    join_kreia(&mut w);
    // The PC's queue is stashed while it is away; give it both entries first.
    w.get_mut(pc).unwrap().actions.push_back(Action::Follow { of: FollowTarget::Leader, range: 2.0 });
    w.push_action(pc, Action::Wait { remaining: 5.0 });
    call(&mut w, "SwitchPlayerCharacter", pc, &[Value::Int(6)]);
    call(&mut w, "SwitchPlayerCharacter", pc, &[Value::Int(-1)]);
    assert_eq!(w.party_leader(), pc, "the PC did not get control back");
    assert!(!has_leader_follow(&w, pc), "the switch back kept the leader follow");
    assert!(has_wait(&w, pc), "the switch back dropped the wait");
}

#[test]
fn set_party_leader_routine_drops_only_the_leader_follow() {
    let Some(mut w) = world("102per") else { return };
    let kreia = join_kreia(&mut w);
    follow_then_wait(&mut w, kreia);
    call(&mut w, "SetPartyLeader", kreia, &[Value::Int(6)]);
    assert_eq!(w.party_leader(), kreia, "SetPartyLeader did not take control");
    assert!(!has_leader_follow(&w, kreia), "SetPartyLeader kept the leader follow");
    assert!(has_wait(&w, kreia), "SetPartyLeader dropped the wait");
}

#[test]
fn a_portrait_switch_clears_the_whole_queue() {
    let Some(mut w) = world("102per") else { return };
    let kreia = join_kreia(&mut w);
    follow_then_wait(&mut w, kreia);
    assert!(w.set_party_leader(kreia));
    assert!(w.get(kreia).unwrap().actions.is_empty(), "the player's switch kept queued actions");
}

/// Creature collision holds a walker about 0.7 m off its target. A tight range
/// (0.0 or 0.5) must still settle: the follower walks in from 1.5 m, stops
/// within the arrival distance, and stays stopped.
#[test]
fn a_tight_follow_range_settles_instead_of_walking_in_place() {
    for range in [0.0f32, 0.5] {
        let Some(mut w) = world("201tel") else { return };
        settle(&mut w, 0.5);
        let pc = w.party_leader();
        let start = w.get(pc).unwrap().position;
        let kreia = w.spawn_creature("p_kreia", start + Vec3::new(1.5, 0.0, 0.0), 0.0).expect("p_kreia");
        call(&mut w, "ActionForceFollowObject", kreia, &[Value::Object(pc), Value::Float(range)]);
        settle(&mut w, 4.0);
        assert!(gap(&w, kreia, pc) <= 1.0, "range {range}: follower {:.2} m away", gap(&w, kreia, pc));
        assert!(w.get(kreia).unwrap().position.distance(start + Vec3::new(1.5, 0.0, 0.0)) > 0.4, "range {range}: follower did not walk in");
        assert!(!w.motion.contains_key(&kreia), "range {range}: the follower still walks in place");

        // Once the target moves well away, the follower walks again.
        let goal = walkable_near(&w, start, 8.0);
        w.place(pc, goal);
        settle(&mut w, 12.0);
        assert!(gap(&w, kreia, pc) <= 1.5, "range {range}: no walk after the target moved away ({:.2} m)", gap(&w, kreia, pc));
    }
}

/// An unreachable target (far off the walkmesh, so no route) makes the follower
/// give up: no progress for FOLLOW_STUCK seconds, after the grace period, holds
/// it still. The walk-again rule has its own unit test (nav.rs).
#[test]
fn a_follower_with_no_route_gives_up_and_stands_still() {
    let Some(mut w) = world("201tel") else { return };
    settle(&mut w, 0.5);
    let pc = w.party_leader();
    let start = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", start + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    let target = w.spawn_creature("p_atton", start, 0.0).expect("p_atton");
    let (mn, _) = w.walk.walkable_bounds();
    w.place(target, Vec3::new(mn.x - 40.0, mn.y, start.z));
    call(&mut w, "ActionForceFollowObject", kreia, &[Value::Object(target), Value::Float(2.0)]);

    settle(&mut w, 6.0);
    let held_at = w.get(kreia).unwrap().position;
    settle(&mut w, 1.0);
    assert!(w.get(kreia).unwrap().position.distance(held_at) < 0.05, "the follower kept moving with no route");
    assert!(!w.motion.contains_key(&kreia), "the given-up follower still steps");
}

/// A new follow must not inherit the hold of an earlier one that ClearAllActions
/// removed. Held far from an unreachable target, then a reachable target 2.5 m
/// away: the new follow walks to its range.
#[test]
fn a_new_follow_after_a_clear_does_not_inherit_a_hold() {
    let Some(mut w) = world("201tel") else { return };
    settle(&mut w, 0.5);
    let pc = w.party_leader();
    let start = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", start + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    let target = w.spawn_creature("p_atton", start, 0.0).expect("p_atton");
    let (mn, _) = w.walk.walkable_bounds();
    w.place(target, Vec3::new(mn.x - 40.0, mn.y, start.z));
    call(&mut w, "ActionForceFollowObject", kreia, &[Value::Object(target), Value::Float(2.0)]);
    settle(&mut w, 6.0);
    call(&mut w, "ClearAllActions", kreia, &[]);

    let near = walkable_near(&w, w.get(kreia).unwrap().position, 2.5);
    w.place(target, near);
    call(&mut w, "ActionForceFollowObject", kreia, &[Value::Object(target), Value::Float(2.0)]);
    settle(&mut w, 4.0);
    assert!(gap(&w, kreia, target) <= 2.3, "the new follow stood at {:.2} m: a hold was inherited", gap(&w, kreia, target));
}

/// Hold-type follows (ActionForceFollowObject, ActionFollowOwner) and any
/// follow of the follower itself are refused, so no queue can lock control
/// until ClearAllActions. ActionFollowLeader on the leader stays refused too.
#[test]
fn hold_follows_on_the_leader_and_self_follows_are_refused() {
    let Some(mut w) = world("201tel") else { return };
    settle(&mut w, 0.5);
    let pc = w.party_leader();
    let at = w.get(pc).unwrap().position;
    let kreia = w.spawn_creature("p_kreia", at + Vec3::new(0.5, 0.0, 0.0), 0.0).expect("p_kreia");
    w.party.push(kreia);

    call(&mut w, "ActionForceFollowObject", pc, &[Value::Object(kreia), Value::Float(2.0)]);
    call(&mut w, "ActionForceFollowObject", pc, &[Value::Object(pc), Value::Float(0.0)]);
    w.get_mut(pc).unwrap().owner = Some(kreia);
    call(&mut w, "ActionFollowOwner", pc, &[Value::Float(2.5)]);
    call(&mut w, "ActionFollowLeader", pc, &[]);
    assert!(w.get(pc).unwrap().actions.is_empty(), "a follow was queued on the leader");
    assert!(w.player_controlled(), "a refused follow locked the leader");

    call(&mut w, "ActionForceFollowObject", kreia, &[Value::Object(kreia), Value::Float(2.0)]);
    w.get_mut(kreia).unwrap().owner = Some(kreia);
    call(&mut w, "ActionFollowOwner", kreia, &[Value::Float(2.5)]);
    assert!(w.get(kreia).unwrap().actions.is_empty(), "a follow of the follower itself was queued");

    // A hold pushed onto the leader directly is done at once, not held.
    w.push_action(pc, Action::Follow { of: FollowTarget::Object(kreia), range: 2.0 });
    w.tick(DT);
    assert!(w.get(pc).unwrap().actions.is_empty(), "a hold on the leader was not released");
    assert!(w.player_controlled());
}

/// A switch that is not the player's removes every follow from the new leader's
/// queue, including a hold, and keeps the rest of the queue.
#[test]
fn a_switch_drops_a_hold_type_follow_too() {
    let Some(mut w) = world("102per") else { return };
    let pc = w.party_leader();
    let kreia = join_kreia(&mut w);
    call(&mut w, "ActionForceFollowObject", kreia, &[Value::Object(pc), Value::Float(2.0)]);
    w.push_action(kreia, Action::Wait { remaining: 5.0 });
    call(&mut w, "SetPartyLeader", kreia, &[Value::Int(6)]);
    assert_eq!(w.party_leader(), kreia);
    assert!(!w.get(kreia).unwrap().actions.iter().any(|a| matches!(a, Action::Follow { .. })), "the new leader kept a hold");
    assert!(has_wait(&w, kreia), "the switch dropped the wait");
}
