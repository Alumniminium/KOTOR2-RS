//! Vitality and Force point regeneration (regen.rs, docs/notes/combat.md
//! "Regeneration"): the out-of-combat and in-combat rates from
//! regeneration.2da (Constitution for HP, max Force for FP), item properties
//! and effects that add to them, the innate party switch, and knocked-out
//! members. Skips (passes) when no install is found at K2_INSTALL or ../..
//! (the repo inside the game folder).

use std::io::Write;
use glam::Vec3;
use k2_res::Install;
use k2_script::{Call, ObjectId, ScriptHost, Value};
use k2_world::{ObjectKind, World};

const DT: f32 = 1.0 / 30.0;

fn world(module: &str) -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(&dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP regen tests: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    let mut w = World::new(install).expect("a world on the install");
    w.auto_dialog = true;
    w.enter_module(module, None).unwrap_or_else(|e| panic!("cannot enter {module}: {e:?}"));
    // Let the spawn scripts and the first heartbeats settle.
    for _ in 0..90 {
        w.tick(DT);
        let _ = w.drain_events();
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
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

/// A point `dist` m from `from` with a clear line of sight to it
/// (party_revival.rs).
fn point_near(w: &mut World, from: Vec3, dist: f32) -> Option<Vec3> {
    for k in 0..36 {
        let ang = k as f32 * 10.0f32.to_radians();
        let (x, y) = (from.x + dist * ang.cos(), from.y + dist * ang.sin());
        let Some(h) = w.walk.ground(x, y, from.z) else { continue };
        let p = Vec3::new(x, y, h.z);
        if w.walk.line_of_sight(p + Vec3::Z * 1.5, from + Vec3::Z * 1.5) {
            return Some(p);
        }
    }
    None
}

fn set_pools(w: &mut World, id: ObjectId, hp: i32, max_hp: i32, force: i32, max_force: i32) {
    let c = w.get_mut(id).and_then(|o| o.creature_mut()).expect("a creature");
    c.hp = hp;
    c.max_hp = max_hp;
    c.force = force;
    c.max_force = max_force;
}

fn creature(w: &mut World, id: ObjectId) -> (i32, i32) {
    let c = w.get(id).and_then(|o| o.creature()).expect("a creature");
    (c.hp, c.force)
}

fn run(w: &mut World, seconds: f32) {
    for _ in 0..(seconds / DT).round() as usize {
        w.tick(DT);
        let _ = w.drain_events();
    }
}

/// The HP rate the table gives out of combat: (outofcombathpbase 1.0 +
/// conmodbonus 0.3 * Constitution modifier) per timeperhp 6 s, not below 0.
fn expected_out_hp(con: i32) -> f32 {
    (1.0 + 0.3 * con as f32).max(0.0) / 6.0
}

#[test]
fn out_of_combat_hp_rate_follows_constitution() {
    let Some(mut w) = world("003ebo") else { return };
    let leader = w.party_leader();
    assert!(!w.in_combat(leader), "the Ebon Hawk starts without a fight");
    for (con_score, con_mod) in [(18, 4), (14, 2), (10, 0), (8, -1), (6, -2), (2, -4)] {
        w.get_mut(leader).and_then(|o| o.creature_mut()).unwrap().abilities[2] = con_score;
        let con = w.combat_stats(leader).expect("stats").modifier(2);
        assert_eq!(con, con_mod, "Constitution {con_score}");
        let (hp, _) = w.regeneration_rates(leader).expect("rates");
        assert!((hp - expected_out_hp(con)).abs() < 1e-4, "Constitution {con_score}: {hp} per s");
    }
}

#[test]
fn out_of_combat_force_rate_refills_in_90_seconds() {
    let Some(mut w) = world("003ebo") else { return };
    let leader = w.party_leader();
    set_pools(&mut w, leader, 30, 30, 0, 90);
    let (_, fp) = w.regeneration_rates(leader).expect("rates");
    assert!((fp - 1.0).abs() < 1e-4, "90 max Force refills 1 per s out of combat, got {fp}");
    run(&mut w, 45.0);
    let (_, force) = creature(&mut w, leader);
    assert!((force - 45).abs() <= 1, "45 s out of combat gave {force} Force");
    run(&mut w, 60.0);
    let (_, force) = creature(&mut w, leader);
    assert_eq!(force, 90, "Force is capped at the maximum");
}

#[test]
fn tick_gains_hp_at_the_out_of_combat_rate() {
    let Some(mut w) = world("003ebo") else { return };
    let leader = w.party_leader();
    set_pools(&mut w, leader, 10, 200, 0, 0);
    let con = w.combat_stats(leader).expect("stats").modifier(2);
    run(&mut w, 60.0);
    assert!(!w.in_combat(leader));
    let (hp, _) = creature(&mut w, leader);
    let expect = 60.0 * expected_out_hp(con);
    assert!(((hp - 10) as f32 - expect).abs() <= 1.5, "60 s gave {} HP, expected about {expect}", hp - 10);
}

#[test]
fn combat_stops_the_base_hp_and_slows_force() {
    let Some(mut w) = world("102per") else { return };
    let leader = w.party_leader();
    let target = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && o.id != leader)
        .map(|o| o.id)
        .collect::<Vec<_>>()
        .into_iter()
        .find(|&id| w.is_enemy(leader, id) && w.get(id).and_then(|o| o.creature()).is_some_and(|c| !c.dead))
        .expect("a hostile creature on Peragus");
    let at = w.get(target).expect("the hostile").position;
    let p = point_near(&mut w, at, 4.0).expect("open ground 4 m from the hostile");
    w.set_player_position(p, 0.0);
    set_pools(&mut w, leader, 10, 200, 0, 90);
    w.player_attack(target);
    for _ in 0..(20.0 / DT) as usize {
        if w.in_combat(leader) {
            break;
        }
        w.tick(DT);
        let _ = w.drain_events();
    }
    assert!(w.in_combat(leader), "the leader fights");
    let con = w.combat_stats(leader).expect("stats").modifier(2);
    let (hp, fp) = w.regeneration_rates(leader).expect("rates");
    // In combat the HP base is 0: only the Constitution term is left.
    assert!((hp - (0.3 * con as f32).max(0.0) / 6.0).abs() < 1e-4, "in combat HP rate {hp}");
    // Force refills in 900 s in combat: a tenth of the out-of-combat rate.
    assert!((fp - 90.0 / 900.0).abs() < 1e-4, "in combat FP rate {fp}");
}

#[test]
fn item_regeneration_adds_points_per_six_seconds() {
    let Some(mut w) = world("003ebo") else { return };
    let leader = w.party_leader();
    set_pools(&mut w, leader, 10, 200, 0, 90);
    let (base_hp, base_fp) = w.regeneration_rates(leader).expect("rates");
    // g_i_crhide018 (a creature hide template) carries item property
    // Regeneration 4. Worn here only to exercise the item path; no named
    // wearer is implied.
    let hide = w.create_item("g_i_crhide018", leader, 1).expect("the hide");
    w.equip(leader, hide, 17).expect("worn in the creature hide slot");
    let (hp, _) = w.regeneration_rates(leader).expect("rates");
    assert!((hp - base_hp - 4.0 / 6.0).abs() < 1e-4, "Regeneration 4 adds 4 HP per 6 s, got {}", hp - base_hp);
    // A robe with Regeneration_Force_Points 3 (a_robe_23): 3 Force per 6 s.
    let robe = w.create_item("a_robe_23", leader, 1).expect("the robe");
    w.equip(leader, robe, 1).expect("worn as body armour");
    let (_, fp) = w.regeneration_rates(leader).expect("rates");
    assert!((fp - base_fp - 3.0 / 6.0).abs() < 1e-4, "Regeneration_Force_Points 3 adds 3 Force per 6 s, got {}", fp - base_fp);
    // The item rate is paid out over a minute of play.
    run(&mut w, 60.0);
    let (hp_now, _) = creature(&mut w, leader);
    assert!(hp_now - 10 >= 35, "a minute with the item gave {} HP", hp_now - 10);
}

#[test]
fn effects_add_regeneration_and_scale_it_for_their_duration() {
    let Some(mut w) = world("003ebo") else { return };
    let leader = w.party_leader();
    set_pools(&mut w, leader, 10, 200, 0, 0);
    let (base, _) = w.regeneration_rates(leader).expect("rates");
    let permanent = w.constant("DURATION_TYPE_PERMANENT").expect("duration constant");
    let temporary = w.constant("DURATION_TYPE_TEMPORARY").expect("duration constant");
    // The regeneration implant effect applied by a_swapimplant.nss:
    // EffectRegenerate(4, 6.0).
    let regen = call(&mut w, "EffectRegenerate", leader, &[Value::Int(4), Value::Float(6.0)]);
    call(&mut w, "ApplyEffectToObject", leader, &[Value::Int(permanent), regen, Value::Object(leader)]);
    let (with_regen, _) = w.regeneration_rates(leader).expect("rates");
    assert!((with_regen - base - 4.0 / 6.0).abs() < 1e-4, "EffectRegenerate(4, 6) adds 4 per 6 s");
    // Battle Meditation's party bonus: EffectVPRegenModifier(50) for 20 s.
    let vp = call(&mut w, "EffectVPRegenModifier", leader, &[Value::Int(50)]);
    call(&mut w, "ApplyEffectToObject", leader, &[Value::Int(temporary), vp, Value::Object(leader), Value::Float(20.0)]);
    let (boosted, _) = w.regeneration_rates(leader).expect("rates");
    assert!((boosted - with_regen * 1.5).abs() < 1e-4, "+50% VP regeneration, got {boosted} from {with_regen}");
    run(&mut w, 25.0);
    let (after, _) = w.regeneration_rates(leader).expect("rates");
    assert!((after - with_regen).abs() < 1e-4, "the modifier ends after 20 s");
    run(&mut w, 30.0);
    let (hp, _) = creature(&mut w, leader);
    assert!(hp > 10, "the permanent effect keeps healing");
}

#[test]
fn knocked_out_members_do_not_regenerate() {
    let Some(mut w) = world("003ebo") else { return };
    let leader = w.party_leader();
    set_pools(&mut w, leader, 0, 200, 5, 90);
    {
        // Keep the knocked-out leader down for the whole test (party.rs would
        // get it up after the battle).
        let c = w.get_mut(leader).and_then(|o| o.creature_mut()).unwrap();
        c.dead = false;
        c.down = true;
        c.get_up_in = 1.0e9;
    }
    assert_eq!(w.regeneration_rates(leader), Some((0.0, 0.0)));
    run(&mut w, 30.0);
    let c = w.get(leader).and_then(|o| o.creature()).unwrap();
    assert!(c.down, "still knocked out");
    assert_eq!((c.hp, c.force), (0, 5), "no regeneration while down");
}

#[test]
fn disable_health_regen_stops_the_party_switch() {
    let Some(mut w) = world("003ebo") else { return };
    let leader = w.party_leader();
    set_pools(&mut w, leader, 10, 200, 0, 0);
    call(&mut w, "DisableHealthRegen", leader, &[Value::Int(1)]);
    assert_eq!(w.regeneration_rates(leader).map(|r| r.0), Some(0.0));
    run(&mut w, 30.0);
    let (hp, _) = creature(&mut w, leader);
    assert_eq!(hp, 10, "innate regeneration is off");
    call(&mut w, "DisableHealthRegen", leader, &[Value::Int(0)]);
    run(&mut w, 30.0);
    let (hp, _) = creature(&mut w, leader);
    assert!(hp > 10, "innate regeneration is back");
}
