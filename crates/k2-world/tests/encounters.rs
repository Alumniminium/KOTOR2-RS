//! GIT encounters on real modules: walking a party member into 103per's
//! droid ambush spawns its creatures once, a spent encounter stays spent
//! through a save, a scripted encounter runs its OnEntered script, and a
//! reset encounter respawns after ResetTime. Skips when no install is found.

use glam::Vec3;
use k2_data::module::point_in_polygon;
use k2_res::Install;
use k2_script::ObjectId;
use k2_world::{ObjectKind, World};

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

fn install() -> Option<Install> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    Install::discover(dir).ok()
}

/// The encounter object with this template (GIT TemplateResRef).
fn encounter_by_template(w: &World, template: &str) -> Option<ObjectId> {
    w.iter().find(|o| o.kind == ObjectKind::Encounter && o.template == template).map(|o| o.id)
}

/// Centre of an encounter's polygon, on the ground.
fn centre(w: &World, id: ObjectId) -> Vec3 {
    let poly = &w.encounter(id).unwrap().polygon;
    let sum = poly.iter().fold(Vec3::ZERO, |s, p| s + Vec3::from(*p));
    let c = sum / poly.len() as f32;
    w.walk.ground(c.x, c.y, c.z + 1.0).map_or(c, |h| Vec3::new(c.x, c.y, h.z))
}

/// Puts the party leader inside the encounter and ticks until the trigger ran.
fn walk_in(w: &mut World, id: ObjectId) {
    let leader = w.party_leader();
    let p = centre(w, id);
    w.place(leader, p);
    assert!(point_in_polygon(&w.encounter(id).unwrap().polygon, p.x, p.y));
    for _ in 0..3 {
        w.tick(1.0 / 30.0);
    }
}

fn kill_spawns(w: &mut World, id: ObjectId) {
    let spawned = w.encounter(id).unwrap().spawned.clone();
    for c in spawned {
        w.objects.get_mut(&c).unwrap().creature_mut().unwrap().dead = true;
    }
}

#[test]
fn ambush_spawns_at_spawn_points_and_spends_itself() {
    let Some(mut w) = world() else { return };
    w.enter_module("103per", None).unwrap();
    let id = encounter_by_template(&w, "103drdambush").expect("103per has the droid ambush");
    let st = w.encounter(id).unwrap();
    assert!(st.active && st.player_only && st.single_shot && !st.reset);
    assert_eq!((st.rec_creatures, st.max_creatures, st.spawns_max), (4, 4, 1));
    assert_eq!(st.creatures[0].resref, "g_droid_mine_02");
    assert_eq!(st.spawn_points.len(), 4);

    walk_in(&mut w, id);
    let st = w.encounter(id).unwrap();
    assert_eq!(st.spawns_current, 1, "one cycle begun");
    assert!(st.running);
    let spawned = st.spawned.clone();
    assert_eq!(spawned.len(), 4, "RecCreatures creatures alive");
    for &c in &spawned {
        let o = w.get(c).unwrap();
        assert_eq!(o.template, "g_droid_mine_02");
        assert_eq!(o.creature().unwrap().encounter, Some(id));
        // k_def_spawn01, the droid's ScriptSpawn, ran: it sets local 11 and listens.
        assert!(o.creature().unwrap().listening, "ScriptSpawn ran");
        assert_eq!(o.local_numbers.get(&11), Some(&6));
        assert!(
            st.spawn_points.iter().any(|(p, _)| (Vec3::from(*p).truncate() - o.position.truncate()).length() < 0.5),
            "spawned at a spawn point"
        );
    }
    assert_eq!(w.iter().filter(|o| o.creature().is_some_and(|c| c.encounter == Some(id))).count(), 4);

    // Staying inside does not spawn a second cycle.
    for _ in 0..60 {
        w.tick(1.0 / 30.0);
    }
    assert_eq!(w.encounter(id).unwrap().spawns_current, 1);
    assert_eq!(w.encounter(id).unwrap().spawned.len(), 4);

    // Single shot, no reset: the cycle ends with its creatures and the encounter is exhausted.
    kill_spawns(&mut w, id);
    w.tick(1.0 / 30.0);
    let st = w.encounter(id).unwrap();
    assert!(st.exhausted && !st.active && !st.running && st.spawned.is_empty());
    assert_eq!(w.vm_errors, Vec::<String>::new());

    // Walking back in spawns nothing more.
    let leader = w.party_leader();
    let far = centre(&w, id) + Vec3::new(40.0, 0.0, 0.0);
    w.place(leader, far);
    w.tick(1.0 / 30.0);
    walk_in(&mut w, id);
    assert_eq!(w.encounter(id).unwrap().spawns_current, 1);
    assert!(w.encounter(id).unwrap().spawned.is_empty());
}

#[test]
fn spent_encounter_stays_spent_through_save() {
    let Some(inst) = install() else { return };
    let Some(mut w) = world() else { return };
    w.enter_module("103per", None).unwrap();
    let id = encounter_by_template(&w, "103drdambush").unwrap();
    walk_in(&mut w, id);
    kill_spawns(&mut w, id);
    w.tick(1.0 / 30.0);
    assert!(w.encounter(id).unwrap().exhausted);

    let data = w.save();
    let loaded = World::load(inst, &data).expect("save loads");
    let st = loaded.encounter(id).expect("encounter object survives the save");
    assert!(st.exhausted, "exhausted flag saved");
    assert_eq!((st.spawns_current, st.spawns_max), (1, 1));
    assert!(!st.active && !st.running);
    assert_eq!(st.creatures.len(), 1);
    assert_eq!(st.polygon.len(), 4);
}

#[test]
fn scripted_encounter_runs_onentered_and_stays_inactive_until_activated() {
    let Some(mut w) = world() else { return };
    w.enter_module("301nar", None).unwrap();
    let id = encounter_by_template(&w, "redeclipse001").expect("301nar has redeclipse001");
    assert!(!w.encounter(id).unwrap().active);
    assert_eq!(w.get(id).unwrap().script("OnEntered"), Some("k_recspawn"));
    walk_in(&mut w, id);
    assert!(w.vm_errors.is_empty(), "OnEntered ran without VM errors: {:?}", w.vm_errors);
    // k_recspawn reads the encounter's local 30 and the 301NAR_Red_Eclipse_At
    // global (seen with K2_CALLS=1) and does not activate it in a fresh game.
    assert!(!w.encounter(id).unwrap().active);
    assert_eq!(w.encounter(id).unwrap().spawns_current, 0);
}

#[test]
fn reset_encounter_respawns_after_reset_time() {
    let Some(mut w) = world() else { return };
    w.enter_module("402dxn", None).unwrap();
    let id = encounter_by_template(&w, "g_drdassagrou001").expect("402dxn has the Dxun encounter");
    let st = w.encounter(id).unwrap();
    assert!(st.reset && !st.single_shot);
    assert_eq!((st.spawns_max, st.reset_time), (11, 1.0));
    // The budget is the party level plus the difficulty value; CR 4 and up need a higher level than a fresh PC.
    let leader = w.party_leader();
    for (_, level) in w.objects.get_mut(&leader).unwrap().creature_mut().unwrap().classes.iter_mut() {
        *level = 8;
    }
    walk_in(&mut w, id);
    assert_eq!(w.encounter(id).unwrap().spawns_current, 1);
    let first = w.encounter(id).unwrap().spawned.len();
    assert!(first > 0);

    kill_spawns(&mut w, id);
    w.tick(1.0 / 30.0);
    let st = w.encounter(id).unwrap();
    assert!(!st.running && !st.exhausted && !st.active, "a reset encounter waits, inactive");
    assert!(st.reset_in.is_some());
    for _ in 0..45 {
        w.tick(1.0 / 30.0);
    }
    let st = w.encounter(id).unwrap();
    assert!(st.reset_in.is_none(), "armed after ResetTime");
    assert!(st.active);
    assert_eq!(st.spawns_current, 2, "the next cycle begins while the party stands inside");
    assert!(!st.spawned.is_empty());
}

#[test]
fn area_script_switches_encounters_on_enter() {
    let Some(mut w) = world() else { return };
    w.enter_module("504ond", None).unwrap();
    // The 504ond area OnEnter calls SetEncounterActive(TRUE) on ls_cowards and
    // SetEncounterActive(FALSE) on the first ds_barrack_guard (seen with K2_CALLS=1).
    let cowards = encounter_by_template(&w, "ls_cowards").expect("504ond has ls_cowards");
    assert!(w.encounter(cowards).unwrap().active);
    let guard = encounter_by_template(&w, "ds_barrack_guard").expect("504ond has ds_barrack_guard");
    assert!(!w.encounter(guard).unwrap().active);
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

#[test]
fn onentered_script_activates_encounter_which_then_spawns() {
    let Some(mut w) = world() else { return };
    w.enter_module("106per", None).unwrap();
    let id = encounter_by_template(&w, "106drdambush").expect("106per has the droid ambush");
    // t_spawndrd (OnEntered) does nothing until 101PER_Sion_Arrives is 1; then
    // it calls SetEncounterActive(TRUE, self) and sets 106PER_Droid_Ambush.
    assert!(!w.encounter(id).unwrap().active);
    w.globals.set_number("101PER_Sion_Arrives", 1);
    walk_in(&mut w, id);
    let st = w.encounter(id).unwrap();
    assert!(st.active, "the script activated the encounter");
    assert_eq!(st.spawns_current, 1);
    assert_eq!(st.spawned.len(), 3, "RecCreatures 3");
    assert_eq!(w.globals.number("106PER_Droid_Ambush"), 1);
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

fn encode(v: &ciborium::Value) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::into_writer(v, &mut out).unwrap();
    out
}

/// Turns one encounter object of a decoded save back into the pre-change
/// form: detail None, no tag, no scripts. Returns whether it was one.
fn blank_encounter_object(o: &mut ciborium::Value) -> bool {
    let ciborium::Value::Map(fields) = o else { return false };
    if !fields.iter().any(|(k, v)| k.as_text() == Some("kind") && v.as_text() == Some("Encounter")) {
        return false;
    }
    for (k, v) in fields.iter_mut() {
        match k.as_text() {
            Some("detail") => *v = ciborium::Value::Text("None".into()),
            Some("tag") => *v = ciborium::Value::Text(String::new()),
            Some("scripts") => *v = ciborium::Value::Map(Vec::new()),
            _ => {}
        }
    }
    true
}

/// Blanks the encounter objects in the live objects and in the remembered
/// modules of a decoded save. Returns how many were blanked.
fn blank_encounters(tree: &mut ciborium::Value) -> usize {
    let ciborium::Value::Map(top) = tree else { return 0 };
    let mut n = 0;
    for (k, v) in top.iter_mut() {
        match (k.as_text(), v) {
            (Some("objects"), ciborium::Value::Map(objs)) => {
                for (_, o) in objs.iter_mut() {
                    n += blank_encounter_object(o) as usize;
                }
            }
            (Some("module_memory"), ciborium::Value::Map(mems)) => {
                for (_, snap) in mems.iter_mut() {
                    let ciborium::Value::Map(fields) = snap else { continue };
                    for (k2, v2) in fields.iter_mut() {
                        if let (Some("objects"), ciborium::Value::Array(objs)) = (k2.as_text(), v2) {
                            for o in objs.iter_mut() {
                                n += blank_encounter_object(o) as usize;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    n
}

#[test]
fn pre_change_save_rebuilds_encounters() {
    let Some(inst) = install() else { return };
    let Some(mut w) = world() else { return };
    w.enter_module("103per", None).unwrap();
    // Rewrite the save as an older build wrote it: encounter objects with no
    // detail, tag or scripts (Detail::None), as spawn_git left them.
    let save = w.save();
    let mut tree: ciborium::Value = ciborium::from_reader(save.state.as_slice()).unwrap();
    let rewritten = blank_encounters(&mut tree);
    assert!(rewritten > 0, "103per has encounters to rewrite");
    let old = k2_world::save::SaveData { version: save.version, module: save.module.clone(), state: encode(&tree) };
    let mut loaded = World::load(inst, &old).expect("pre-change save loads");
    let id = encounter_by_template(&loaded, "103drdambush").expect("encounter object survives");
    let st = loaded.encounter(id).expect("state rebuilt from the GIT");
    assert_eq!((st.rec_creatures, st.spawn_points.len(), st.polygon.len()), (4, 4, 4));
    assert_eq!(loaded.get(id).unwrap().tag, "103drdambush");
    assert!(loaded.get(id).unwrap().script("OnEntered").is_none());
    walk_in(&mut loaded, id);
    assert_eq!(loaded.encounter(id).unwrap().spawned.len(), 4, "the rebuilt encounter spawns");
}

#[test]
fn leaving_clears_occupancy_and_reentry_fires_onentered_again() {
    let Some(mut w) = world() else { return };
    w.enter_module("106per", None).unwrap();
    let id = encounter_by_template(&w, "106drdambush").expect("106per has the droid ambush");
    // t_spawndrd (OnEntered) activates the encounter and sets 106PER_Droid_Ambush.
    w.globals.set_number("101PER_Sion_Arrives", 1);
    walk_in(&mut w, id);
    assert_eq!(w.globals.number("106PER_Droid_Ambush"), 1, "OnEntered ran");
    assert!(w.encounter(id).unwrap().inside.contains(&w.party_leader()));

    w.enter_module("101per", None).unwrap();
    let left = w.module_memory["106per"].objects.iter().find(|o| o.id == id).expect("remembered");
    assert!(left.encounter().unwrap().inside.is_empty(), "leaving clears occupancy");

    w.globals.set_number("106PER_Droid_Ambush", 0);
    w.enter_module("106per", None).unwrap();
    let at = w.get(w.party_leader()).unwrap().position;
    assert!(!point_in_polygon(&w.encounter(id).unwrap().polygon, at.x, at.y), "the party re-enters outside");
    walk_in(&mut w, id);
    assert_eq!(w.globals.number("106PER_Droid_Ambush"), 1, "walking in after a visit fires OnEntered again");
}

#[test]
fn continuous_encounter_without_reset_is_exhausted_when_its_cycle_ends() {
    let Some(mut w) = world() else { return };
    w.enter_module("402dxn", None).unwrap();
    let id = encounter_by_template(&w, "g_drdassagrou001").expect("402dxn has the Dxun encounter");
    // Reset 0 means no respawn (format document), so the cycle's end exhausts
    // the encounter even though Respawns would allow more cycles.
    w.objects.get_mut(&id).unwrap().encounter_mut().unwrap().reset = false;
    let leader = w.party_leader();
    for (_, level) in w.objects.get_mut(&leader).unwrap().creature_mut().unwrap().classes.iter_mut() {
        *level = 8;
    }
    walk_in(&mut w, id);
    assert_eq!(w.encounter(id).unwrap().spawns_current, 1);
    kill_spawns(&mut w, id);
    w.tick(1.0 / 30.0);
    let st = w.encounter(id).unwrap();
    assert!(st.exhausted && !st.active && st.reset_in.is_none(), "exhausted, no re-arm");
    assert_eq!(st.exhaustions, 1);
    for _ in 0..45 {
        w.tick(1.0 / 30.0);
    }
    let st = w.encounter(id).unwrap();
    assert!(!st.active && st.spawned.is_empty() && st.spawns_current == 1, "stays spent with the party inside");
    assert_eq!(st.exhaustions, 1);
}

#[test]
fn script_deactivation_cancels_the_reset_wait() {
    let Some(mut w) = world() else { return };
    w.enter_module("402dxn", None).unwrap();
    let id = encounter_by_template(&w, "g_drdassagrou001").expect("402dxn has the Dxun encounter");
    let leader = w.party_leader();
    for (_, level) in w.objects.get_mut(&leader).unwrap().creature_mut().unwrap().classes.iter_mut() {
        *level = 8;
    }
    walk_in(&mut w, id);
    kill_spawns(&mut w, id);
    w.tick(1.0 / 30.0);
    assert!(w.encounter(id).unwrap().reset_in.is_some(), "waiting to re-arm");
    // A script's SetEncounterActive(FALSE) while the party stands inside.
    w.set_encounter_active(id, false);
    assert!(w.encounter(id).unwrap().reset_in.is_none());
    for _ in 0..45 {
        w.tick(1.0 / 30.0);
    }
    let st = w.encounter(id).unwrap();
    assert!(!st.active && st.spawned.is_empty() && st.spawns_current == 1, "the wait did not switch it back on");
}

#[test]
fn script_activation_cancels_the_reset_wait_and_spawns_now() {
    let Some(mut w) = world() else { return };
    w.enter_module("402dxn", None).unwrap();
    let id = encounter_by_template(&w, "g_drdassagrou001").expect("402dxn has the Dxun encounter");
    let leader = w.party_leader();
    for (_, level) in w.objects.get_mut(&leader).unwrap().creature_mut().unwrap().classes.iter_mut() {
        *level = 8;
    }
    walk_in(&mut w, id);
    kill_spawns(&mut w, id);
    w.tick(1.0 / 30.0);
    assert!(w.encounter(id).unwrap().reset_in.is_some(), "waiting to re-arm");
    w.set_encounter_active(id, true);
    w.tick(1.0 / 30.0);
    let st = w.encounter(id).unwrap();
    assert!(st.reset_in.is_none() && st.active, "the script's TRUE replaces the wait");
    assert_eq!(st.spawns_current, 2, "the party inside gets the next cycle at once");
}

#[test]
fn exhaustion_count_increments_once_per_exhaustion() {
    let Some(mut w) = world() else { return };
    w.enter_module("103per", None).unwrap();
    let id = encounter_by_template(&w, "103drdambush").unwrap();
    walk_in(&mut w, id);
    kill_spawns(&mut w, id);
    w.tick(1.0 / 30.0);
    assert!(w.encounter(id).unwrap().exhausted);
    assert_eq!(w.encounter(id).unwrap().exhaustions, 1);
    // Counted once, not every tick. (The counter counts the OnExhausted dispatch;
    // no test script is attached, so the script's own effect is not checked.)
    for _ in 0..30 {
        w.tick(1.0 / 30.0);
    }
    assert_eq!(w.encounter(id).unwrap().exhaustions, 1);
}

#[test]
fn exhausted_clears_when_a_cycle_starts_again() {
    let Some(mut w) = world() else { return };
    w.enter_module("402dxn", None).unwrap();
    let id = encounter_by_template(&w, "g_drdassagrou001").expect("402dxn has the Dxun encounter");
    for (_, level) in w.objects.get_mut(&w.party_leader()).unwrap().creature_mut().unwrap().classes.iter_mut() {
        *level = 8;
    }
    // Reset 0: the first cycle's end exhausts the encounter (Respawns 10 notwithstanding).
    w.objects.get_mut(&id).unwrap().encounter_mut().unwrap().reset = false;
    walk_in(&mut w, id);
    kill_spawns(&mut w, id);
    w.tick(1.0 / 30.0);
    assert!(w.encounter(id).unwrap().exhausted && !w.encounter(id).unwrap().active);
    assert_eq!(w.encounter(id).unwrap().exhaustions, 1);
    // A script activates it again; the party is still inside.
    w.set_encounter_active(id, true);
    w.tick(1.0 / 30.0);
    let st = w.encounter(id).unwrap();
    assert!(!st.exhausted && st.running, "a new cycle clears exhausted");
    assert_eq!(st.spawns_current, 2);
    assert!(!st.spawned.is_empty());
    kill_spawns(&mut w, id);
    w.tick(1.0 / 30.0);
    assert_eq!(w.encounter(id).unwrap().exhaustions, 2, "the second exhaustion fires OnExhausted again");
}

#[test]
fn setting_active_on_a_spent_encounter_changes_the_flag_only() {
    let Some(mut w) = world() else { return };
    w.enter_module("103per", None).unwrap();
    let id = encounter_by_template(&w, "103drdambush").unwrap();
    walk_in(&mut w, id);
    kill_spawns(&mut w, id);
    w.tick(1.0 / 30.0);
    // No cycles left (SpawnsMax 1): the flag is set, nothing spawns.
    w.set_encounter_active(id, true);
    for _ in 0..30 {
        w.tick(1.0 / 30.0);
    }
    let st = w.encounter(id).unwrap();
    assert!(st.active, "GetEncounterActive reports the flag as set");
    assert_eq!((st.spawns_current, st.running), (1, false));
    assert!(st.spawned.is_empty());
}

#[test]
fn single_spawn_allows_one_of_each_creature_alive() {
    let Some(mut w) = world() else { return };
    w.enter_module("103per", None).unwrap();
    let id = encounter_by_template(&w, "103drdambush").unwrap();
    w.objects.get_mut(&id).unwrap().encounter_mut().unwrap().creatures[0].single_spawn = true;
    walk_in(&mut w, id);
    let st = w.encounter(id).unwrap();
    assert_eq!(st.spawned.len(), 1, "one g_droid_mine_02 alive at a time");
    assert_eq!(st.spawns_current, 1);
}

#[test]
fn guaranteed_copies_spawn_regardless_of_the_cr_budget() {
    // ls_vaklu_enc001 (506ond): CR 4, 4, 5, 5, one guaranteed copy each. A
    // level 1 party (1 + Normal 2 = 3) cannot take CR 4 by the budget.
    let Some(mut w) = world() else { return };
    w.enter_module("506ond", None).unwrap();
    let id = encounter_by_template(&w, "ls_vaklu_enc001").expect("506ond has ls_vaklu_enc001");
    for pc in w.party.clone() {
        if let Some(c) = w.objects.get_mut(&pc).and_then(|o| o.creature_mut()) {
            for (_, level) in c.classes.iter_mut() {
                *level = 1;
            }
        }
    }
    walk_in(&mut w, id);
    let mut names: Vec<String> = w.encounter(id).unwrap().spawned.iter().map(|&c| w.get(c).unwrap().template.clone()).collect();
    names.sort();
    assert_eq!(names, ["c_boma2", "si_jedi_appr", "sith_elite", "sith_wrangler"]);

    // Control: no guaranteed copies, so nothing fits the budget.
    let mut bare = world().unwrap();
    bare.enter_module("506ond", None).unwrap();
    let id = encounter_by_template(&bare, "ls_vaklu_enc001").unwrap();
    for pc in bare.party.clone() {
        if let Some(c) = bare.objects.get_mut(&pc).and_then(|o| o.creature_mut()) {
            for (_, level) in c.classes.iter_mut() {
                *level = 1;
            }
        }
    }
    for e in bare.objects.get_mut(&id).unwrap().encounter_mut().unwrap().creatures.iter_mut() {
        e.guaranteed = 0;
    }
    walk_in(&mut bare, id);
    assert!(bare.encounter(id).unwrap().spawned.is_empty());
}

#[test]
fn spawn_pool_is_sorted_by_cr_and_keeps_file_order_for_ties() {
    let entry = |resref: &str, cr: f32| k2_data::EncounterCreature { resref: resref.into(), cr, ..Default::default() };
    let bp = k2_data::Encounter { creatures: vec![entry("a", 5.0), entry("b", 2.0), entry("c", 4.0), entry("d", 2.0)], ..Default::default() };
    let st = k2_world::encounter::EncounterState::from_blueprint(&bp, &k2_data::module::EncounterInstance::default());
    let order: Vec<&str> = st.creatures.iter().map(|c| c.resref.as_str()).collect();
    assert_eq!(order, ["b", "d", "c", "a"]);
}

#[test]
fn revisit_with_the_party_inside_does_not_fire_onentered() {
    let Some(mut w) = world() else { return };
    w.enter_module("106per", None).unwrap();
    let id = encounter_by_template(&w, "106drdambush").expect("106per has the droid ambush");
    let poly = w.encounter(id).unwrap().polygon.clone();
    // A uniquely tagged object inside the polygon to start the party on.
    let tag = w
        .iter()
        .filter(|o| o.kind != ObjectKind::Item && !o.tag.is_empty() && point_in_polygon(&poly, o.position.x, o.position.y))
        .map(|o| o.tag.clone())
        .find(|t| w.iter().filter(|o| &o.tag == t).count() == 1)
        .expect("106per has a uniquely tagged object inside the ambush");
    w.globals.set_number("101PER_Sion_Arrives", 1);
    walk_in(&mut w, id);
    assert_eq!(w.globals.number("106PER_Droid_Ambush"), 1, "OnEntered ran on the first visit");
    w.enter_module("101per", None).unwrap();
    w.globals.set_number("106PER_Droid_Ambush", 0);
    w.enter_module("106per", Some(&tag)).unwrap();
    let leader = w.party_leader();
    let at = w.get(leader).unwrap().position;
    assert!(point_in_polygon(&poly, at.x, at.y), "the party starts inside");
    assert!(w.encounter(id).unwrap().inside.contains(&leader), "the occupant is primed");
    for _ in 0..6 {
        w.tick(1.0 / 30.0);
    }
    assert_eq!(w.globals.number("106PER_Droid_Ambush"), 0, "no OnEntered on the revisit");
}

#[test]
fn revisit_with_a_non_party_creature_inside_a_non_playeronly_encounter() {
    let Some(mut w) = world() else { return };
    w.enter_module("506ond", None).unwrap();
    let enc = w
        .iter()
        .filter(|o| o.encounter().is_some_and(|e| !e.player_only && e.polygon.len() >= 3))
        .map(|o| o.id)
        .next()
        .expect("506ond has a non-PlayerOnly encounter");
    let npc = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && !w.party.contains(&o.id))
        .find(|o| o.creature().is_some_and(|c| !c.dead && c.encounter.is_none()))
        .map(|o| o.id)
        .expect("506ond has an NPC that is not in the party");
    let c = centre(&w, enc);
    w.place(npc, c);
    w.tick(1.0 / 30.0);
    assert!(w.encounter(enc).unwrap().inside.contains(&npc), "the NPC is qualifying: PlayerOnly is off");
    // Leave with the NPC inside, then come back.
    w.place(npc, c);
    w.enter_module("101per", None).unwrap();
    w.enter_module("506ond", None).unwrap();
    let at = w.get(npc).unwrap().position;
    assert!(point_in_polygon(&w.encounter(enc).unwrap().polygon, at.x, at.y), "the NPC is restored inside");
    assert!(w.encounter(enc).unwrap().inside.contains(&npc), "primed as already inside before any tick");
    for _ in 0..6 {
        w.tick(1.0 / 30.0);
    }
    assert!(w.encounter(enc).unwrap().inside.contains(&npc), "no exit and re-entry on the revisit");
}

#[test]
fn knocked_down_creatures_neither_enter_nor_count_as_inside() {
    let Some(mut w) = world() else { return };
    w.enter_module("506ond", None).unwrap();
    let enc = w
        .iter()
        .filter(|o| o.encounter().is_some_and(|e| !e.player_only && e.polygon.len() >= 3))
        .map(|o| o.id)
        .next()
        .expect("506ond has a non-PlayerOnly encounter");
    let npc = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && !w.party.contains(&o.id))
        .find(|o| o.creature().is_some_and(|c| !c.dead && !c.down && c.encounter.is_none()))
        .map(|o| o.id)
        .expect("506ond has an NPC that is not in the party");
    let c = centre(&w, enc);
    let set_down = |w: &mut World, down: bool| w.objects.get_mut(&npc).unwrap().creature_mut().unwrap().down = down;
    // Knocked down inside the polygon: it does not enter.
    set_down(&mut w, true);
    w.place(npc, c);
    w.tick(1.0 / 30.0);
    assert!(!w.encounter(enc).unwrap().inside.contains(&npc), "a downed creature does not enter");
    // Back on its feet: it enters.
    set_down(&mut w, false);
    w.tick(1.0 / 30.0);
    assert!(w.encounter(enc).unwrap().inside.contains(&npc), "a standing creature enters");
    // Knocked down while inside: it is no longer counted as inside.
    set_down(&mut w, true);
    w.tick(1.0 / 30.0);
    assert!(!w.encounter(enc).unwrap().inside.contains(&npc), "a downed creature is not inside");
}

#[test]
fn module_memory_from_a_pre_change_save_rebuilds_on_reentry() {
    let Some(inst) = install() else { return };
    let Some(mut w) = world() else { return };
    w.enter_module("103per", None).unwrap();
    w.enter_module("101per", None).unwrap();
    let save = w.save();
    let mut tree: ciborium::Value = ciborium::from_reader(save.state.as_slice()).unwrap();
    assert!(blank_encounters(&mut tree) >= 1, "103per's remembered encounter was blanked");
    let old = k2_world::save::SaveData { version: save.version, module: save.module.clone(), state: encode(&tree) };
    let mut loaded = World::load(inst, &old).expect("pre-change save loads");
    loaded.enter_module("103per", None).unwrap();
    let id = encounter_by_template(&loaded, "103drdambush").expect("the remembered encounter is back");
    assert_eq!(loaded.get(id).unwrap().tag, "103drdambush");
    assert_eq!(loaded.encounter(id).unwrap().polygon.len(), 4);
    walk_in(&mut loaded, id);
    assert_eq!(loaded.encounter(id).unwrap().spawned.len(), 4, "the rebuilt encounter spawns");
}

#[test]
fn pre_change_save_rebuilds_shared_template_encounters_one_to_one() {
    let Some(inst) = install() else { return };
    let Some(fresh) = world() else { return };
    let mut fresh = fresh;
    fresh.enter_module("506ond", None).unwrap();
    let shared = fresh.iter().filter(|o| o.kind == ObjectKind::Encounter && o.template == "ls_vaklu_enc").count();
    assert!(shared >= 2, "506ond has several ls_vaklu_enc instances");
    let expected: Vec<(u32, String, Vec<[f32; 3]>, usize, Vec<String>)> = fresh
        .iter()
        .filter(|o| o.kind == ObjectKind::Encounter)
        .map(|o| {
            let st = o.encounter().unwrap();
            (o.id, o.tag.clone(), st.polygon.clone(), st.spawn_points.len(), st.creatures.iter().map(|c| c.resref.clone()).collect())
        })
        .collect();
    let save = fresh.save();
    let mut tree: ciborium::Value = ciborium::from_reader(save.state.as_slice()).unwrap();
    assert_eq!(blank_encounters(&mut tree), expected.len());
    let old = k2_world::save::SaveData { version: save.version, module: save.module.clone(), state: encode(&tree) };
    let loaded = World::load(inst, &old).expect("pre-change save loads");
    for (id, tag, polygon, spawn_points, resrefs) in expected {
        let o = loaded.get(id).unwrap_or_else(|| panic!("encounter {id} survives"));
        let st = o.encounter().unwrap_or_else(|| panic!("encounter {id} rebuilt"));
        assert_eq!(o.tag, tag, "tag of {id}");
        assert_eq!(st.polygon, polygon, "polygon of {id} is its own instance's");
        assert_eq!(st.spawn_points.len(), spawn_points, "spawn points of {id}");
        assert_eq!(st.creatures.iter().map(|c| c.resref.clone()).collect::<Vec<_>>(), resrefs);
    }
}

/// Idle tick cost of the encounter pass: 506ond (26 encounters) with the
/// party at the entry, timed with the encounters as they are and with their
/// detail removed (the same tick without the encounter pass).
/// Run: cargo test --release -p k2-world --test encounters idle_tick -- --ignored --nocapture
#[test]
#[ignore]
fn idle_tick_cost_of_the_encounter_pass() {
    let Some(mut w) = world() else { return };
    w.enter_module("506ond", None).unwrap();
    let ids: Vec<ObjectId> = w.iter().filter(|o| o.encounter().is_some()).map(|o| o.id).collect();
    let mut bare = world().unwrap();
    bare.enter_module("506ond", None).unwrap();
    for id in &ids {
        bare.objects.get_mut(id).unwrap().detail = k2_world::Detail::None;
    }
    let time = |w: &mut World| {
        let start = std::time::Instant::now();
        for _ in 0..3000 {
            w.tick(1.0 / 30.0);
        }
        start.elapsed().as_secs_f64() * 1e6 / 3000.0
    };
    let (mut with, mut without) = (f64::MAX, f64::MAX);
    for _ in 0..3 {
        with = with.min(time(&mut w));
        without = without.min(time(&mut bare));
    }
    println!("ENCOUNTERS {} idle tick: with {with:.1} us, without {without:.1} us, pass {:.1} us", ids.len(), with - without);
}
