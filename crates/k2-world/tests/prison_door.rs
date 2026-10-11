//! The 101per prison door (PrisonRoomDr) against the real install, in the order a
//! playthrough meets it. Adm_Console's switch unlocks the door (a_unlock_prison)
//! once the door's heartbeat (k_door_heart) finds no hostile mining droid. Walking
//! into KreiaPrisonWarn starts the door's own talk (prsndoor), where Kreia sets
//! 101PER_Kreia_Telepath. Using the unlocked door then opens it, and its OnOpen
//! (k_attontlk) starts Atton's talk. Skips (passes, printing why) when no install is
//! found at K2_INSTALL or ../.. . With K2_INSTALL set, an install that does not load
//! is a failure.

use ciborium::Value;
use glam::{Vec2, Vec3};
use k2_script::ObjectId;
use k2_world::save::SaveData;
use k2_world::{ObjectKind, ScriptCtx, World, WorldEvent};

mod support;

use support::install;

/// 101per entered, with nothing run: the state at load.
fn fresh_101per() -> Option<World> {
    let mut w = World::new(install()?).expect("the install loads into a world");
    w.enter_module("101per", None).unwrap();
    w.drain_events();
    Some(w)
}

fn door_open(w: &World, door: ObjectId) -> bool {
    w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open)
}

fn door_locked(w: &World, door: ObjectId) -> bool {
    w.get(door).and_then(|o| o.door()).is_some_and(|d| d.locked)
}

/// Ticks the world for `secs` game seconds at 30 Hz.
fn tick_for(w: &mut World, secs: f32) {
    for _ in 0..(secs * 30.0) as usize {
        w.tick(1.0 / 30.0);
        let _ = w.drain_events();
    }
}

/// Plays the running conversation to its end, as a player would: the first reply each
/// time, and the bare lines continued. Bounded at 60 s of game time.
fn play_out(w: &mut World) {
    for _ in 0..(60 * 30) {
        if w.conversation.is_none() {
            return;
        }
        match w.conversation_view() {
            Some(v) if !v.replies.is_empty() => w.choose_reply(0),
            _ => w.advance_conversation(),
        }
        w.tick(1.0 / 30.0);
        let _ = w.drain_events();
    }
    assert!(w.conversation.is_none(), "the conversation did not end in 60 s: {:?}", w.conversation_view().map(|v| v.dialog));
}

#[test]
fn the_prison_door_opens_by_use_after_kreias_talk_and_starts_attons() {
    let Some(mut w) = fresh_101per() else { return };
    let pc = w.party_leader();
    let door = w.by_tag("PrisonRoomDr", 0).expect("101per has PrisonRoomDr");
    assert!(door_locked(&w, door), "PrisonRoomDr starts locked");

    // The playtester killed the mining droids before the switch. A dead droid stays
    // hostile to the PC until its corpse is destroyed (combat.rs CORPSE_SECONDS).
    kill_mining_droids(&mut w);
    // Adm_Console's switch (admlog E3) runs a_unlock_prison. The door's heartbeat (6 s)
    // then runs k_door_heart, which clears the lock once no mining droid is hostile, and
    // sets local 30 on the door.
    assert!(w.run_script("a_unlock_prison", pc, ScriptCtx::default()).is_some(), "a_unlock_prison runs");
    tick_for(&mut w, 15.0);
    assert!(!door_locked(&w, door), "the switch clears PrisonRoomDr's lock once the mining droids are dead and gone");
    assert!(!door_open(&w, door), "the switch does not open the door");
    assert_eq!(w.get(door).and_then(|o| o.local_bools.get(&30).copied()), Some(true), "k_door_heart sets local 30 on the door");

    // The intro (101awake) can still be running: play it out first, as a player would.
    play_out(&mut w);
    // Walking into KreiaPrisonWarn starts the door's own talk (a_kreiaspch2).
    let trigger = w.by_tag("KreiaPrisonWarn", 0).expect("101per has the KreiaPrisonWarn trigger");
    let poly = w.get(trigger).and_then(|o| o.trigger()).expect("KreiaPrisonWarn is a trigger").polygon.clone();
    let inside = walkable_point_in(&w, &poly).expect("KreiaPrisonWarn has a walkable point inside its polygon");
    w.set_player_position(inside, 0.0);
    let mut talk = false;
    for _ in 0..(6 * 30) {
        w.tick(1.0 / 30.0);
        let _ = w.drain_events();
        if w.conversation.is_some() {
            talk = true;
            break;
        }
    }
    assert!(talk, "walking into KreiaPrisonWarn did not start the door's talk");
    assert_eq!(w.conversation_view().map(|v| v.dialog), Some("prsndoor".to_owned()), "the talk is not prsndoor");

    play_out(&mut w);
    assert_eq!(w.globals.number("101PER_Kreia_Telepath"), 1, "Kreia's talk sets 101PER_Kreia_Telepath");
    assert!(!door_open(&w, door), "the talk leaves the door shut");

    // Locked at load and unlocked since: the use opens the door, and its talk is not started
    // (lib.rs use_object, the locked_at_load rule).
    w.use_object(pc, door);
    let _ = w.drain_events();
    assert!(door_open(&w, door), "the unlocked prison door opens on use");

    // OnOpen (k_attontlk) walks Atton to WP_ATTONCAGE and starts his talk.
    let mut atton = false;
    for _ in 0..(10 * 30) {
        w.tick(1.0 / 30.0);
        let started = w.drain_events().into_iter().any(|e| matches!(e, WorldEvent::ConversationStarted { dialog, .. } if dialog == "101atton"));
        if started || w.conversation_view().is_some_and(|v| v.dialog == "101atton") {
            atton = true;
            break;
        }
    }
    assert!(atton, "opening the door did not start Atton's 101atton talk (conversation {:?})", w.conversation_view().map(|v| v.dialog));
}

/// Before the switch, the prison door is locked: its use says why (its talk) and it
/// stays shut, and the use does not unlock it.
#[test]
fn a_locked_prison_door_says_why_and_stays_shut() {
    let Some(mut w) = fresh_101per() else { return };
    let pc = w.party_leader();
    let door = w.by_tag("PrisonRoomDr", 0).expect("101per has PrisonRoomDr");
    assert!(door_locked(&w, door), "PrisonRoomDr starts locked");

    // OnFailToOpen (a_compdlg) queues the door's talk, which starts on the next ticks. The
    // line is read from the conversation view (a message box is a conversation too).
    w.use_object(pc, door);
    for _ in 0..60 {
        w.tick(1.0 / 30.0);
        let _ = w.drain_events();
    }
    let line = w.conversation_view();
    assert!(
        line.as_ref().is_some_and(|v| v.dialog == "prsndoor" && !v.text.trim().is_empty()),
        "the locked door says its line (conversation view: {:?})",
        line.map(|v| (v.dialog, v.text))
    );
    assert!(!door_open(&w, door), "a locked door stays shut");
    assert!(door_locked(&w, door), "a use does not unlock the door");
}

/// The was-locked-at-load field is part of the door's saved state: a load keeps it, so
/// the reloaded unlocked prison door still opens on use.
#[test]
fn the_locked_at_load_field_survives_a_save_and_load() {
    let Some(mut w) = fresh_101per() else { return };
    let Some(inst) = install() else { return };
    let door = w.by_tag("PrisonRoomDr", 0).expect("101per has PrisonRoomDr");
    assert!(w.get(door).and_then(|o| o.door()).is_some_and(|d| d.locked_at_load == Some(true)), "PrisonRoomDr is locked at load");
    // Unlocked by a script, as a_unlock_prison's heartbeat does (set directly here).
    w.get_mut(door).and_then(|o| o.door_mut()).expect("the door has a door state").locked = false;

    let data = w.save();
    let mut back = World::load(inst, &data).expect("the save loads");
    let door = back.by_tag("PrisonRoomDr", 0).expect("the reloaded world has PrisonRoomDr");
    let state = back.get(door).and_then(|o| o.door()).expect("the reloaded door has a door state");
    assert!(state.locked_at_load == Some(true), "the reloaded door still remembers it was locked at load");
    assert!(!state.locked, "the reloaded door is still unlocked");

    let pc = back.party_leader();
    back.use_object(pc, door);
    let _ = back.drain_events();
    assert!(door_open(&back, door), "the reloaded unlocked prison door opens on use");
}

/// Kills every mining droid through the death path (slay_creature: EffectDeath, which
/// combat.rs kill() handles), then lets the corpses fade. A corpse is destroyed 12 s after
/// death (CORPSE_SECONDS) through the world's normal destroy timer, so the ticks run past
/// that. The droids are not walked to: many sit behind doors or in other rooms, and combat
/// is not what this test checks. Asserts that no mining droid is left in the world.
fn kill_mining_droids(w: &mut World) {
    let pc = w.party_leader();
    for d in mining_droids(w) {
        w.slay_creature(d, pc);
    }
    for _ in 0..(13 * 30) {
        w.tick(1.0 / 30.0);
        let _ = w.drain_events();
    }
    assert!(mining_droids(w).is_empty(), "the mining droids faded: {:?}", mining_droids(w));
}

/// Removes every `locked_at_load` key from a decoded save, as a save made before the
/// field existed has none.
fn strip_locked_at_load(v: &mut Value) {
    match v {
        Value::Map(m) => {
            m.retain(|(k, _)| k.as_text() != Some("locked_at_load"));
            for (_, child) in m.iter_mut() {
                strip_locked_at_load(child);
            }
        }
        Value::Array(a) => {
            for child in a.iter_mut() {
                strip_locked_at_load(child);
            }
        }
        _ => {}
    }
}

/// A save from before the field existed loads, takes the lock state at load from the
/// door's template (Locked 1 for PrisonRoomDr), and the unlocked prison door opens on use.
#[test]
fn a_save_from_before_the_field_takes_the_lock_state_from_the_template() {
    let Some(mut w) = fresh_101per() else { return };
    let Some(inst) = install() else { return };
    let door = w.by_tag("PrisonRoomDr", 0).expect("101per has PrisonRoomDr");
    w.get_mut(door).and_then(|o| o.door_mut()).expect("the door has a door state").locked = false;

    let save = w.save();
    let mut tree: Value = ciborium::from_reader(save.state.as_slice()).expect("the save decodes");
    strip_locked_at_load(&mut tree);
    let mut state = Vec::new();
    ciborium::into_writer(&tree, &mut state).expect("the edited save encodes");
    let old = SaveData { version: save.version, module: save.module.clone(), state };

    let mut back = World::load(inst, &old).expect("a save without locked_at_load loads");
    let door = back.by_tag("PrisonRoomDr", 0).expect("the reloaded world has PrisonRoomDr");
    assert_eq!(back.get(door).and_then(|o| o.door()).and_then(|d| d.locked_at_load), Some(true), "the template says PrisonRoomDr is locked at load");
    let pc = back.party_leader();
    back.use_object(pc, door);
    let _ = back.drain_events();
    assert!(door_open(&back, door), "the unlocked prison door opens on use in a save from before the field");
}

/// The mining droids of 101per (tag g_assassindrd01): the objects k_door_heart walks
/// with GetObjectByTag.
fn mining_droids(w: &World) -> Vec<ObjectId> {
    w.iter().filter(|o| o.kind == ObjectKind::Creature && o.tag.eq_ignore_ascii_case("g_assassindrd01")).map(|o| o.id).collect()
}

/// The droid's mindrd talk switches it (a_set_friend: ChangeToStandardFaction(OBJECT_SELF, 2)),
/// so the party's faction reads it as friendly and not hostile.
fn switch_to_friend(w: &mut World, droid: ObjectId) {
    assert!(w.run_script("a_set_friend", droid, ScriptCtx::default()).is_some(), "a_set_friend runs");
}

/// The heartbeat path: the switch is thrown while the droids are hostile, and the
/// door's heartbeat (k_door_heart) unlocks it only once no droid is hostile.
#[test]
fn the_heartbeat_unlocks_the_prison_door_once_no_mining_droid_is_hostile() {
    let Some(mut w) = fresh_101per() else { return };
    let pc = w.party_leader();
    let door = w.by_tag("PrisonRoomDr", 0).expect("101per has PrisonRoomDr");
    let droids = mining_droids(&w);
    assert!(!droids.is_empty(), "101per has mining droids");
    assert!(w.run_script("a_unlock_prison", pc, ScriptCtx::default()).is_some(), "a_unlock_prison runs");
    // The switch's delayed run of k_door_heart (0.8 s) finds hostile droids and leaves the lock.
    tick_for(&mut w, 2.0);
    assert!(door_locked(&w, door), "the hostile droids hold the lock");

    // All but the last droid are switched. The heartbeat (6 s) runs and must not unlock.
    let (last, rest) = droids.split_last().expect("at least one mining droid");
    for &d in rest {
        switch_to_friend(&mut w, d);
    }
    tick_for(&mut w, 7.0);
    assert!(door_locked(&w, door), "one hostile droid still holds the lock");

    switch_to_friend(&mut w, *last);
    tick_for(&mut w, 13.0);
    assert!(!door_locked(&w, door), "the heartbeat unlocks the door once no droid is hostile");
    assert!(!door_open(&w, door), "the unlock does not open the door");
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

/// A point of the trigger polygon (corners in game coordinates) that lies inside
/// it and on the walkmesh: a 0.5 m grid over the polygon's bounds.
fn walkable_point_in(w: &World, poly: &[[f32; 3]]) -> Option<Vec3> {
    let xy: Vec<Vec2> = poly.iter().map(|p| Vec2::new(p[0], p[1])).collect();
    let lo = xy.iter().fold(Vec2::splat(f32::MAX), |a, &p| a.min(p));
    let hi = xy.iter().fold(Vec2::splat(f32::MIN), |a, &p| a.max(p));
    let z = poly.iter().map(|p| p[2]).sum::<f32>() / poly.len() as f32;
    let mut y = lo.y;
    while y <= hi.y {
        let mut x = lo.x;
        while x <= hi.x {
            if inside(Vec2::new(x, y), &xy)
                && let Some(h) = w.walk.ground(x, y, z)
            {
                return Some(Vec3::new(x, y, h.z));
            }
            x += 0.5;
        }
        y += 0.5;
    }
    None
}

/// Even-odd test of `p` against the polygon `poly` in the plane.
fn inside(p: Vec2, poly: &[Vec2]) -> bool {
    let n = poly.len();
    let mut hit = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            hit = !hit;
        }
    }
    hit
}
