//! Trapped doors and placeables, the trap routines (Flag, Examine, Recover,
//! DetonateMine, GetNearestTrapToObject), stealth toggling through
//! ActionUseSkill and stealth breaking in combat or dialog. Runs against the
//! install through the same routine path the scripts use (ScriptHost::call).
//! Skips when no install is found. Run with --nocapture for the survey.

use std::sync::Arc;

use glam::Vec3;
use k2_data::blueprint::trap_fields;
use k2_res::Install;
use k2_script::{Call, ObjectId, ScriptHost, Value, OBJECT_INVALID};
use k2_world::stealth::StealthXp;
use k2_world::traps::TrapState;
use k2_world::{ObjectKind, World, WorldEvent};
use rakata_formats::gff::{Gff, GffLabel, GffStruct, GffValue};
use rakata_formats::{read_gff_from_bytes, write_gff_to_vec};

const DT: f32 = 1.0 / 30.0;
/// traps.2da row 3: TRAP_FRAGMENTATION_MINE_MINOR (TRAP_BASE_TYPE_FRAGMENTATION_MINE_MINOR).
const FRAG_MINE: i32 = 3;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

fn run(w: &mut World, secs: f32) -> Vec<WorldEvent> {
    let mut ev = Vec::new();
    let mut t = 0.0;
    while t < secs {
        w.tick(DT);
        ev.extend(w.drain_events());
        t += DT;
    }
    ev
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

fn object(v: Value) -> ObjectId {
    match v {
        Value::Object(o) => o,
        other => panic!("expected an object, got {other:?}"),
    }
}

/// ActionUseSkill(nSkill, oTarget, nSubSkill, oItemUsed).
fn use_skill(w: &mut World, user: ObjectId, skill: i32, target: ObjectId, sub: i32) {
    let args = [Value::Int(skill), Value::Object(target), Value::Int(sub), Value::Object(OBJECT_INVALID)];
    let _ = call(w, "ActionUseSkill", user, &args);
}

/// Only the PC stays in the area, so creature-side effects are easy to read.
fn clear_others(w: &mut World, pc: ObjectId) {
    let others: Vec<ObjectId> = w.iter().filter(|o| o.kind == ObjectKind::Creature && o.id != pc).map(|o| o.id).collect();
    for id in others {
        w.destroy(id);
    }
}

/// Disarms every module trap except `keep`, so the module's own triggers
/// (102per has several) cannot fire or win a nearest-trap search.
fn disarm_other_traps(w: &mut World, keep: ObjectId) {
    let others: Vec<ObjectId> = w.iter().filter(|o| o.trap.is_some() && o.id != keep).map(|o| o.id).collect();
    for id in others {
        w.get_mut(id).unwrap().trap.as_mut().unwrap().disabled = true;
    }
}

fn fragment_trap(detect_dc: i32, disarm_dc: i32) -> TrapState {
    TrapState {
        trap_type: FRAG_MINE,
        detect_dc,
        disarm_dc,
        detectable: true,
        disarmable: true,
        one_shot: true,
        faction: 1,
        ..Default::default()
    }
}

/// Puts a trap on an object of the area (what a trapped UTD/UTP would do).
fn trap_object(w: &mut World, id: ObjectId, trap: TrapState) {
    w.get_mut(id).expect("object").trap = Some(Box::new(trap));
}

fn trap_fired(ev: &[WorldEvent], trap: ObjectId) -> bool {
    ev.iter().any(|e| matches!(e, WorldEvent::TrapTriggered { trap: t, .. } if *t == trap))
}

/// A closed, unlocked door of 102per that is not a transition.
fn plain_door(w: &World) -> Option<ObjectId> {
    w.iter()
        .find(|o| o.kind == ObjectKind::Door && o.conversation.is_none() && o.door().is_some_and(|d| !d.open && !d.locked && !d.hp.destroyed && d.linked_to_module.is_none()))
        .map(|o| o.id)
}

/// A closed, unlocked container placeable (HasInventory) of 102per.
fn closed_container(w: &World) -> Option<ObjectId> {
    w.iter()
        .find(|o| o.kind == ObjectKind::Placeable && o.placeable().is_some_and(|p| p.has_inventory && !p.open && !p.locked && !p.hp.destroyed))
        .map(|o| o.id)
}

fn equip_stealth_unit(w: &mut World, pc: ObjectId) {
    let belt = w.create_item("a_belt_03", pc, 1).expect("stealth field generator");
    w.equip(pc, belt, 10).expect("equip belt");
    w.set_skill_rank(pc, 2, 30);
}

#[test]
fn survey_trapped_doors_and_placeables() {
    let Some(mut w) = world() else { return };
    let names: Vec<String> = w.res.install.module_names();
    let (mut doors, mut placeables) = (0, 0);
    for m in names {
        if w.enter_module(&m, None).is_err() {
            continue;
        }
        let d = w.iter().filter(|o| o.kind == ObjectKind::Door && o.trap.is_some()).count();
        let p = w.iter().filter(|o| o.kind == ObjectKind::Placeable && o.trap.is_some()).count();
        if d + p > 0 {
            println!("{m}: {d} trapped doors, {p} trapped placeables");
        }
        doors += d;
        placeables += p;
    }
    println!("trapped doors {doors}, trapped placeables {placeables}");
    // No shipped UTD or UTP has TrapFlag set, so this is 0 today. A change
    // here means a template now carries a trap: look at it before updating.
    assert_eq!((doors, placeables), (0, 0), "shipped trapped doors or placeables");
}

/// A closed placeable of 102per with no inventory (not a container).
fn closed_plain_placeable(w: &World) -> Option<ObjectId> {
    w.iter()
        .find(|o| o.kind == ObjectKind::Placeable && o.placeable().is_some_and(|p| !p.has_inventory && !p.open && !p.hp.destroyed))
        .map(|o| o.id)
}

/// Inferred: a locked placeable's trap does not go off, but its use still
/// runs (OnUsed and the conversation): 604dan knrthegg is Locked with
/// OnUsed a_compdlg and conversation egg, and the shipped templates rely on it.
#[test]
fn locked_placeable_keeps_its_trap_while_locked() {
    let Some(mut w) = world() else { return };
    w.enter_module("604dan", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let egg = w.iter().find(|o| o.kind == ObjectKind::Placeable && o.template == "knrthegg").map(|o| o.id).expect("a kinrath egg in 604dan");
    trap_object(&mut w, egg, fragment_trap(99, 99));
    w.get_mut(egg).unwrap().placeable_mut().unwrap().locked = true;
    w.use_object(pc, egg);
    let ev = w.drain_events();
    assert!(!trap_fired(&ev, egg), "a locked placeable does not go off");
    assert!(w.is_armed_trap(egg), "its trap stays armed");
    assert!(w.conversation.as_ref().is_some_and(|c| c.dialog.resref == "egg"), "the locked use still starts its conversation");

    w.end_conversation(false);
    w.get_mut(egg).unwrap().placeable_mut().unwrap().locked = false;
    w.use_object(pc, egg);
    let ev = w.drain_events();
    assert!(trap_fired(&ev, egg), "unlocked, the same use sets the trap off");
}

/// Examine: a mine gets the TLK 1517 line; a placeable's trap gets only its
/// traps.2da name (no original line says "trap").
#[test]
fn examine_gives_the_mine_line_only_for_a_mine() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let placeable = closed_plain_placeable(&w).expect("a closed placeable in 102per");
    trap_object(&mut w, placeable, fragment_trap(99, 99));
    w.get_mut(placeable).unwrap().trap.as_mut().unwrap().flagged = true;
    let text = w.examine_trap(pc, placeable).expect("examine the placeable trap");
    assert_eq!(text, "Frag Mine, Minor", "the traps.2da name alone");

    let at = w.get(pc).unwrap().position + Vec3::new(2.5, 0.0, 0.0);
    let mine = w.spawn_mine(FRAG_MINE, at, 1, 30, None, 1);
    w.get_mut(mine).unwrap().trap.as_mut().unwrap().flagged = true;
    let text = w.examine_trap(pc, mine).expect("examine the mine");
    assert_eq!(text, "This mine appears to be a: Frag Mine, Minor", "TLK 1517 with the name");
}

/// Detection of an unnamed placeable's trap falls back to its traps.2da name
/// (not a made-up "Mine").
#[test]
fn detection_of_an_unnamed_placeable_trap_uses_its_trap_name() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let placeable = closed_plain_placeable(&w).expect("a closed placeable in 102per");
    trap_object(&mut w, placeable, fragment_trap(1, 99));
    disarm_other_traps(&mut w, placeable);
    w.get_mut(placeable).unwrap().name = String::new();
    let at = w.get(placeable).unwrap().position + Vec3::new(2.0, 0.0, 0.0);
    w.get_mut(pc).unwrap().position = at;
    w.set_skill_rank(pc, 3, 60);
    let ev = run(&mut w, 3.0);
    assert!(w.party_detected(placeable), "Awareness 60 detects the placeable trap");
    assert!(
        ev.iter().any(|e| matches!(e, WorldEvent::Feedback { text, .. } if text.contains("detects Frag Mine, Minor:"))),
        "the detection line names the trap by its traps.2da name: {ev:?}"
    );
}

/// A party member stepping on a mine gets "You triggered a Mine!" (TLK 1461).
#[test]
fn stepping_on_a_mine_says_so() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let at = w.get(pc).unwrap().position + Vec3::new(2.5, 0.0, 0.0);
    let mine = w.spawn_mine(FRAG_MINE, at, 99, 99, None, 1);
    disarm_other_traps(&mut w, mine);
    w.get_mut(pc).unwrap().position = at;
    let ev = run(&mut w, 0.5);
    assert!(trap_fired(&ev, mine), "the mine goes off under the party");
    assert!(
        ev.iter().any(|e| matches!(e, WorldEvent::Feedback { text, .. } if text == "You triggered a Mine!")),
        "the party is told: {ev:?}"
    );
}

/// A trapped transition door goes off before the transit (inferred, as any
/// trapped door): the trap fires and the transit is still requested.
#[test]
fn trapped_transition_door_goes_off_before_the_transit() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let door = w
        .iter()
        .find(|o| o.kind == ObjectKind::Door && o.door().is_some_and(|d| !d.locked && !d.open && d.linked_to_module.as_deref().is_some_and(|m| !m.is_empty())))
        .map(|o| o.id)
        .expect("an unlocked transition door in 102per");
    trap_object(&mut w, door, fragment_trap(99, 99));
    w.use_object(pc, door);
    let ev = w.drain_events();
    assert!(trap_fired(&ev, door), "the transition door's trap goes off");
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::TransitionRequested(_))), "the transit is still requested");
    assert!(!w.is_armed_trap(door), "a one-shot trap stops being armed");
}

#[test]
fn trapped_door_goes_off_when_opened() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let door = plain_door(&w).expect("a closed door in 102per");
    trap_object(&mut w, door, fragment_trap(99, 99));
    w.set_skill_rank(pc, 3, 0);
    assert_eq!(int(call(&mut w, "GetIsTrapped", pc, &[Value::Object(door)])), 1, "the armed trap is reported");
    w.use_object(pc, door);
    let ev = w.drain_events();
    assert!(trap_fired(&ev, door), "opening an undetected trapped door sets it off");
    assert!(w.get(door).is_some(), "a trapped door is not destroyed by its trap");
    assert!(w.get(door).unwrap().door().unwrap().open, "the door still opens");
    assert!(!w.is_armed_trap(door), "a one-shot trap stops being armed");
    assert_eq!(int(call(&mut w, "GetIsTrapped", pc, &[Value::Object(door)])), 0);
}

#[test]
fn trapped_container_goes_off_when_opened() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let container = closed_container(&w).expect("a closed container in 102per");
    trap_object(&mut w, container, fragment_trap(99, 99));
    w.set_skill_rank(pc, 3, 0);
    w.use_object(pc, container);
    let ev = w.drain_events();
    assert!(trap_fired(&ev, container), "opening an undetected trapped container sets it off");
    assert!(w.get(container).is_some(), "the container survives its trap");
    assert!(w.get(container).unwrap().placeable().unwrap().open, "the container opens");
}

#[test]
fn disarmed_container_does_not_go_off() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let container = closed_container(&w).expect("a closed container in 102per");
    let at = w.get(container).unwrap().position;
    trap_object(&mut w, container, fragment_trap(99, 0));
    w.set_skill_rank(pc, 1, 60);
    w.get_mut(pc).unwrap().position = at + Vec3::new(1.5, 0.0, 0.0);
    assert!(w.disarm_trap(pc, container, false).expect("disarm"), "Demolitions 60 vs DC 0 disarms");
    assert!(!w.is_armed_trap(container));
    assert!(w.get(container).is_some(), "a disarmed container stays");
    w.use_object(pc, container);
    let ev = w.drain_events();
    assert!(!trap_fired(&ev, container), "a disarmed container does not go off");
    assert!(w.get(container).unwrap().placeable().unwrap().open, "the container opens");
}

#[test]
fn examine_flag_and_nearest_trap() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let at = w.get(pc).unwrap().position + Vec3::new(2.5, 0.0, 0.0);
    let mine = w.spawn_mine(FRAG_MINE, at, 1, 30, None, 1);
    disarm_other_traps(&mut w, mine);
    w.set_skill_rank(pc, 3, 60);
    let _ = run(&mut w, 3.0);
    assert!(w.party_detected(mine), "Awareness 60 detects the mine");

    w.drain_events();
    use_skill(&mut w, pc, 1, mine, 102);
    let ev = w.drain_events();
    assert!(
        ev.iter().any(|e| matches!(e, WorldEvent::Feedback { text, .. } if text.contains("appears to be a"))),
        "Examine Trap gives the trap type: {ev:?}"
    );

    let other = w.iter().find(|o| o.kind == ObjectKind::Creature && o.id != pc).map(|o| o.id);
    assert_eq!(int(call(&mut w, "GetTrapFlagged", pc, &[Value::Object(mine)])), 0);
    use_skill(&mut w, pc, 1, mine, 100);
    assert_eq!(int(call(&mut w, "GetTrapFlagged", pc, &[Value::Object(mine)])), 1, "Flag Trap makes it visible to all");
    if let Some(o) = other {
        assert_eq!(int(call(&mut w, "GetTrapDetectedBy", pc, &[Value::Object(mine), Value::Object(o)])), 1);
    }

    let near = object(call(&mut w, "GetNearestTrapToObject", pc, &[Value::Object(pc), Value::Int(1)]));
    assert_eq!(near, mine, "the nearest detected trap to the PC is the mine");
}

#[test]
fn recover_only_applies_to_triggers() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let door = plain_door(&w).expect("a closed door in 102per");
    trap_object(&mut w, door, fragment_trap(99, 0));
    w.set_skill_rank(pc, 1, 60);
    let at = w.get(door).unwrap().position;
    w.get_mut(pc).unwrap().position = at + Vec3::new(1.0, 0.0, 0.0);
    assert!(w.disarm_trap(pc, door, true).is_err(), "there is nothing to recover from a door");
    assert!(w.get(door).is_some());
}

#[test]
fn detonate_mine_removes_it_without_the_damage_script() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let at = w.get(pc).unwrap().position + Vec3::new(1.2, 0.0, 0.0);
    let mine = w.spawn_mine(FRAG_MINE, at, 10, 15, None, 1);
    disarm_other_traps(&mut w, mine);
    let hp = w.get(pc).unwrap().creature().unwrap().hp;
    let _ = call(&mut w, "DetonateMine", pc, &[Value::Object(mine)]);
    let ev = run(&mut w, 0.2);
    assert!(trap_fired(&ev, mine), "the explosion is reported");
    assert!(w.get(mine).is_none(), "the mine is removed");
    assert!(!ev.iter().any(|e| matches!(e, WorldEvent::Damaged { object, .. } if *object == pc)), "no damage script runs");
    assert_eq!(w.get(pc).unwrap().creature().unwrap().hp, hp);
}

#[test]
fn detonate_mine_removes_a_mine_that_is_not_one_shot() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let at = w.get(pc).unwrap().position + Vec3::new(1.2, 0.0, 0.0);
    let mine = w.spawn_mine(FRAG_MINE, at, 10, 15, None, 1);
    disarm_other_traps(&mut w, mine);
    w.get_mut(mine).unwrap().trap.as_mut().unwrap().one_shot = false;
    let _ = call(&mut w, "DetonateMine", pc, &[Value::Object(mine)]);
    let ev = run(&mut w, 0.2);
    assert!(trap_fired(&ev, mine), "the explosion is reported");
    assert!(w.get(mine).is_none(), "DetonateMine removes the mine whatever its one_shot flag");
}

#[test]
fn detonate_mine_removes_a_creator_set_mine() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let at = w.get(pc).unwrap().position + Vec3::new(1.2, 0.0, 0.0);
    let mine = w.spawn_mine(FRAG_MINE, at, 10, 15, Some(pc), 2);
    disarm_other_traps(&mut w, mine);
    let _ = call(&mut w, "DetonateMine", pc, &[Value::Object(mine)]);
    let ev = run(&mut w, 0.2);
    assert!(trap_fired(&ev, mine), "the set mine explodes");
    assert!(w.get(mine).is_none(), "and is destroyed");
}

/// Inferred from the wording of nwscript 857: DetonateMine only disables a
/// trapped placeable (it is not destroyed).
#[test]
fn detonate_mine_only_disables_a_trapped_placeable() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    let placeable = closed_plain_placeable(&w).expect("a closed placeable in 102per");
    trap_object(&mut w, placeable, fragment_trap(99, 99));
    let _ = call(&mut w, "DetonateMine", pc, &[Value::Object(placeable)]);
    let ev = run(&mut w, 0.2);
    assert!(trap_fired(&ev, placeable), "the trap goes off");
    assert!(w.get(placeable).is_some(), "the placeable stays");
    assert!(!w.is_armed_trap(placeable), "and its trap is disabled");
}

/// A plain trigger (HintKolto in 101per, no trap state) is not a mine: DetonateMine
/// on it does nothing and it stays.
#[test]
fn detonate_mine_leaves_a_plain_trigger_alone() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    let pc = w.party_leader();
    let hint = w.by_tag("HintKolto", 0).expect("the HintKolto trigger in 101per");
    assert!(w.trap(hint).is_none(), "a plain trigger has no trap state");
    let _ = call(&mut w, "DetonateMine", pc, &[Value::Object(hint)]);
    let ev = run(&mut w, 0.2);
    assert!(w.get(hint).is_some(), "the plain trigger is not destroyed");
    assert!(!trap_fired(&ev, hint), "and nothing goes off");
}

#[test]
fn stealth_toggles_through_action_use_skill() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    equip_stealth_unit(&mut w, pc);
    use_skill(&mut w, pc, 2, OBJECT_INVALID, 0);
    assert!(w.is_stealthed(pc), "ActionUseSkill(SKILL_STEALTH) enters stealth");
    assert_eq!(int(call(&mut w, "IsStealthed", pc, &[Value::Object(pc)])), 1);
    use_skill(&mut w, pc, 2, OBJECT_INVALID, 0);
    assert!(!w.is_stealthed(pc), "and a second use leaves it");
}

/// 101per with the PC equipped with a stealth unit, and an NPC to talk to.
fn stealth_setup(module: &str) -> Option<(World, ObjectId, ObjectId)> {
    let mut w = world()?;
    w.enter_module(module, None).unwrap();
    let pc = w.party_leader();
    let owner = w.iter().find(|o| o.kind == ObjectKind::Creature && o.id != pc).map(|o| o.id).expect("an NPC");
    equip_stealth_unit(&mut w, pc);
    Some((w, pc, owner))
}

/// 101awake is an AnimatedCut dialog with an unconditional start.
#[test]
fn stealth_ends_when_a_cutscene_starts() {
    let Some((mut w, pc, owner)) = stealth_setup("101per") else { return };
    w.set_stealth(pc, true).expect("enter stealth");
    assert!(w.start_conversation(owner, pc, "101awake"), "the cutscene starts");
    assert!(!w.is_stealthed(pc), "a cutscene ends stealth by default");
}

#[test]
fn set_keep_stealth_in_dialog_keeps_stealth_in_a_cutscene() {
    let Some((mut w, pc, owner)) = stealth_setup("101per") else { return };
    w.systems.stealth.keep_in_dialog = true;
    w.set_stealth(pc, true).expect("enter stealth");
    assert!(w.start_conversation(owner, pc, "101awake"), "the cutscene starts");
    assert!(w.is_stealthed(pc), "SetKeepStealthInDialog keeps stealth");
}

/// 101atton has no AnimatedCut and no CameraAngle 4 line. Guess: a plain
/// conversation keeps stealth (nwscript names cutscenes only).
#[test]
fn stealth_survives_a_plain_conversation() {
    let Some((mut w, pc, owner)) = stealth_setup("101per") else { return };
    w.set_stealth(pc, true).expect("enter stealth");
    assert!(w.start_conversation(owner, pc, "101atton"), "the dialog starts");
    assert!(w.is_stealthed(pc), "a plain conversation does not end stealth");
}

/// Entry 58 (the first passing link of 101atton's first reply) is made an
/// animated-camera line: reaching it ends stealth mid-conversation.
#[test]
fn stealth_ends_at_a_cutscene_line_inside_a_plain_dialog() {
    let Some((mut w, pc, owner)) = stealth_setup("101per") else { return };
    w.set_stealth(pc, true).expect("enter stealth");
    assert!(w.start_conversation(owner, pc, "101atton"), "the dialog starts");
    let conv = w.conversation.as_mut().expect("in conversation");
    Arc::get_mut(&mut conv.dialog).expect("sole owner of the dialog").entries[58].camera_angle = 4;
    w.choose_reply(0);
    assert_eq!(w.conversation.as_ref().and_then(|c| c.entry), Some(58), "the conversation reached entry 58");
    assert!(!w.is_stealthed(pc), "a CameraAngle 4 line ends stealth");
}

#[test]
fn keep_in_dialog_is_not_saved() {
    let flag = StealthXp { keep_in_dialog: true, ..Default::default() };
    let saved = serde_json::to_string(&flag).expect("serialize");
    let loaded: StealthXp = serde_json::from_str(&saved).expect("load");
    assert!(!loaded.keep_in_dialog, "nwscript: the flag is not saved ({saved})");
}

#[test]
fn stealth_breaks_when_attacking() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    equip_stealth_unit(&mut w, pc);
    w.set_stealth(pc, true).expect("enter stealth");
    disarm_other_traps(&mut w, OBJECT_INVALID);
    let candidates: Vec<ObjectId> = w.iter().filter(|o| o.kind == ObjectKind::Creature && o.id != pc).map(|o| o.id).collect();
    let Some(target) = candidates.into_iter().find(|&c| w.is_enemy(c, pc)) else {
        println!("no hostile in 102per to attack");
        return;
    };
    let tp = w.get(target).unwrap().position;
    w.get_mut(pc).unwrap().position = tp + Vec3::new(1.0, 0.0, 0.0);
    w.player_attack(target);
    let _ = run(&mut w, 1.0);
    assert!(!w.is_stealthed(pc), "an attack ends stealth");
}

/// Set Mine from a trap kit, detection by the party, then Recover Mine gives
/// the kit back (the traps.2da resref item).
#[test]
fn set_detect_and_recover_a_mine() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    clear_others(&mut w, pc);
    w.give_new_item("g_i_trapkit004", pc, 1, true).expect("a frag mine kit");
    let kit = w.party_mines().first().map(|m| m.0).expect("a mine in the party inventory");
    let mine = w.set_mine(pc, kit).expect("set mine");
    assert!(
        w.drain_events().iter().any(|e| matches!(e, WorldEvent::Feedback { text, .. } if text == "Mine successfully laid.")),
        "TLK 37251 is the Set Mine feedback"
    );
    assert!(w.is_armed_trap(mine), "a set mine is armed");
    assert!(w.party_mines().iter().all(|m| m.0 != kit), "the kit was used up");
    assert_eq!(w.trap(mine).unwrap().creator, Some(pc));

    // The party's own mines are known from the start.
    assert!(w.party_detected(mine));
    w.set_skill_rank(pc, 1, 60);
    let kits = |w: &World| w.party_inventory().iter().filter(|&&i| w.get(i).is_some_and(|o| o.template == "g_i_trapkit004")).count();
    let before = kits(&w);
    assert!(w.disarm_trap(pc, mine, true).expect("recover"), "Demolitions 60 recovers");
    let ev = w.drain_events();
    assert!(
        ev.iter().any(|e| matches!(e, WorldEvent::Feedback { text, .. } if text == "Recover Mine: success")),
        "the recover feedback line: {ev:?}"
    );
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::ItemReceived { .. })), "and the item-received line");
    assert!(w.get(mine).is_none(), "the recovered mine is removed");
    assert_eq!(kits(&w), before + 1, "the kit comes back");
}

/// Inferred rule: only the creature that set a mine recovers it without a
/// roll. Demolitions 1 + take-20 meets the recover DC (16 + 5); a roll of
/// Demolitions 0 can never reach it.
#[test]
fn only_the_setter_recovers_its_mine_without_a_roll() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    let other = w.iter().find(|o| o.kind == ObjectKind::Creature && o.id != pc).map(|o| o.id).expect("an NPC");
    let at = w.get(pc).unwrap().position + Vec3::new(1.0, 0.0, 0.0);
    w.set_skill_rank(pc, 1, 1);
    let own = w.spawn_mine(FRAG_MINE, at, 1, 16, Some(pc), 2);
    assert!(w.disarm_trap(pc, own, true).expect("recover"), "the setter recovers on take-20");

    let at_other = w.get(pc).unwrap().position + Vec3::new(-1.0, 0.0, 0.0);
    let theirs = w.spawn_mine(FRAG_MINE, at_other, 1, 16, Some(pc), 2);
    w.set_skill_rank(other, 1, 0);
    w.get_mut(other).unwrap().position = at_other;
    assert_eq!(w.disarm_trap(other, theirs, true), Ok(false), "someone else rolls and cannot reach DC 21");
    let ev = w.drain_events();
    assert!(
        ev.iter().any(|e| matches!(e, WorldEvent::SkillCheck { user, success: false, .. } if *user == other)),
        "the other creature made a roll"
    );
}

/// Inferred rule, as above: a second party member rolls for the setter's
/// mine. (A party faction used to count as the setter's, so it skipped the roll.)
#[test]
fn another_party_member_rolls_to_recover_a_mine() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    let other = w.iter().find(|o| o.kind == ObjectKind::Creature && o.id != pc).map(|o| o.id).expect("an NPC");
    w.party.push(other);
    let at = w.get(pc).unwrap().position + Vec3::new(1.0, 0.0, 0.0);
    w.set_skill_rank(pc, 1, 1);
    let mine = w.spawn_mine(FRAG_MINE, at, 1, 16, Some(pc), 2);
    w.set_skill_rank(other, 1, 0);
    w.get_mut(other).unwrap().position = at;
    assert_eq!(w.disarm_trap(other, mine, true), Ok(false), "a roll of at most 20 cannot reach the recover DC of 21");
    let ev = w.drain_events();
    assert!(
        ev.iter().any(|e| matches!(e, WorldEvent::SkillCheck { user, success: false, .. } if *user == other)),
        "the other party member made a roll"
    );
}

/// The trap block of a UTD or UTP (TrapFlag and the keys beside it) maps to
/// a TrapState. Built as GFF bytes, so it needs no install.
#[test]
fn trap_block_of_a_template_maps_to_trap_state() {
    let gff = |file_type: &[u8; 4], fields: Vec<(&str, GffValue)>| {
        let mut root = GffStruct::new(-1);
        for (label, value) in fields {
            root.push_field(GffLabel::new(label).expect("label"), value);
        }
        write_gff_to_vec(&Gff::new(*file_type, root)).expect("gff bytes")
    };
    let door = gff(
        b"UTD ",
        vec![
            ("TrapFlag", GffValue::UInt8(1)),
            ("TrapType", GffValue::UInt8(3)),
            ("TrapDetectable", GffValue::UInt8(1)),
            ("TrapDetectDC", GffValue::Int32(15)),
            ("TrapDisarmable", GffValue::UInt8(0)),
            ("DisarmDC", GffValue::Int32(20)),
            ("TrapOneShot", GffValue::UInt8(1)),
            ("Faction", GffValue::UInt32(0)),
        ],
    );
    let root = read_gff_from_bytes(&door).expect("read").root;
    let t = TrapState::from_fields(&trap_fields(&root)).expect("TrapFlag set");
    assert_eq!((t.trap_type, t.detect_dc, t.disarm_dc), (3, 15, 20));
    assert!(t.detectable && !t.disarmable && t.one_shot);
    assert_eq!(t.faction, 1, "Faction 0 reads as Hostile_1");
    assert!(t.creator.is_none() && !t.disabled && !t.flagged && t.fuse.is_none());

    let placeable = gff(b"UTP ", vec![("TrapFlag", GffValue::UInt8(1)), ("TrapType", GffValue::UInt8(5)), ("Faction", GffValue::UInt32(2))]);
    let root = read_gff_from_bytes(&placeable).expect("read").root;
    let t = TrapState::from_fields(&trap_fields(&root)).expect("TrapFlag set");
    assert_eq!((t.trap_type, t.faction), (5, 2), "a party faction is kept");
    assert_eq!((t.detect_dc, t.disarm_dc, t.detectable, t.disarmable, t.one_shot), (0, 0, false, false, false), "absent keys read as 0 / false");

    let plain = gff(b"UTP ", vec![("TrapType", GffValue::UInt8(5))]);
    let root = read_gff_from_bytes(&plain).expect("read").root;
    assert!(TrapState::from_fields(&trap_fields(&root)).is_none(), "no TrapFlag, no trap");
}
