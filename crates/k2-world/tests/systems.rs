//! Mines, traps, stealth and influence against the install. Skips when no
//! install is found. Run with --nocapture for the survey lines.

use glam::{Vec2, Vec3};
use k2_res::Install;
use k2_script::ObjectId;
use k2_world::{ObjectKind, ScriptCtx, World, WorldEvent};

const DT: f32 = 1.0 / 30.0;

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

fn hp(w: &World, id: ObjectId) -> i32 {
    w.get(id).and_then(|o| o.creature()).map_or(0, |c| c.hp)
}

/// The first module (in Peragus order) with a GIT trap trigger.
fn module_with_trap(w: &mut World) -> Option<(String, ObjectId)> {
    for m in ["101per", "102per", "103per", "104per", "105per", "106per", "107per", "201tel", "301nar", "601dan"] {
        if w.enter_module(m, None).is_err() {
            continue;
        }
        if let Some(t) = w.iter().find(|o| o.trap.as_ref().is_some_and(|t| !t.disabled)).map(|o| o.id) {
            return Some((m.to_owned(), t));
        }
    }
    None
}

#[test]
fn survey_trap_triggers() {
    let Some(mut w) = world() else { return };
    let mut total = 0;
    let mut modules = 0;
    let names: Vec<String> = w.res.install.module_names();
    for m in names {
        if w.enter_module(&m, None).is_err() {
            continue;
        }
        let n = w.iter().filter(|o| o.trap.is_some()).count();
        if n > 0 {
            modules += 1;
            total += n;
            println!("{m}: {n} trap triggers");
        }
    }
    println!("trap triggers: {total} in {modules} modules");
}

#[test]
fn stepping_on_a_trap_sets_it_off_and_hurts() {
    let Some(mut w) = world() else { return };
    let Some((m, trap)) = module_with_trap(&mut w) else {
        println!("no module with a trap trigger found");
        return;
    };
    let pc = w.party_leader();
    let others: Vec<_> = w.iter().filter(|o| o.kind == ObjectKind::Creature && o.id != pc).map(|o| o.id).collect();
    for id in others {
        w.destroy(id);
    }
    w.get_mut(pc).and_then(|o| o.creature_mut()).unwrap().min1hp = false;
    let at = w.trap_position(trap).unwrap();
    // Start just outside, then step onto the trap.
    let start = at + Vec3::new(4.0, 0.0, 0.0);
    w.get_mut(pc).unwrap().position = start;
    run(&mut w, 0.2);
    let before = hp(&w, pc);
    w.get_mut(pc).unwrap().position = at;
    let ev = run(&mut w, 1.0);
    let fired = ev.iter().any(|e| matches!(e, WorldEvent::TrapTriggered { trap: t, .. } if *t == trap));
    println!("{m}: trap {} fired={fired} hp {before} -> {}", w.describe(trap), hp(&w, pc));
    assert!(fired, "the trap should go off");
    let hit = ev.iter().any(|e| matches!(e, WorldEvent::Damaged { object, .. } if *object == pc));
    assert!(hp(&w, pc) < before || hit, "the trap should hurt the PC");
    let one_shot = w.trap(trap).is_none_or(|t| t.one_shot);
    if one_shot {
        assert!(!w.is_armed_trap(trap), "one-shot traps are gone after firing");
    }
}

#[test]
fn detect_and_disarm_a_trap() {
    let Some(mut w) = world() else { return };
    let Some((_m, trap)) = module_with_trap(&mut w) else { return };
    let pc = w.party_leader();
    let others: Vec<_> = w.iter().filter(|o| o.kind == ObjectKind::Creature && o.id != pc).map(|o| o.id).collect();
    for id in others {
        w.destroy(id);
    }
    // Awareness and Demolitions high enough to always pass.
    w.set_skill_rank(pc, 3, 60);
    w.set_skill_rank(pc, 1, 60);
    let at = w.trap_position(trap).unwrap();
    w.get_mut(pc).unwrap().position = at + Vec3::new(2.5, 0.0, 0.0);
    let ev = run(&mut w, 3.0);
    assert!(w.party_detected(trap), "Awareness 60 should detect the trap");
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::Feedback { text, .. } if text.contains("detects"))));
    assert!(w.visible_traps().iter().any(|m| m.id == trap && m.hostile));
    let ok = w.disarm_trap(pc, trap, false).expect("disarm");
    assert!(ok, "Demolitions 60 should disarm");
    assert!(!w.is_armed_trap(trap));
    // Walking over it now does nothing.
    w.get_mut(pc).unwrap().position = at;
    let ev = run(&mut w, 1.0);
    assert!(!ev.iter().any(|e| matches!(e, WorldEvent::TrapTriggered { .. })));
}

#[test]
fn recover_gives_the_mine_back() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    let at = w.get(pc).unwrap().position + Vec3::new(1.5, 0.0, 0.0);
    // A hostile frag mine (traps.2da row 3) nobody stands on.
    let mine = w.spawn_mine(3, at, 10, 15, None, 1);
    k2_world_set_detected(&mut w, mine, pc);
    w.set_skill_rank(pc, 1, 60);
    let before = w.party_inventory().iter().filter(|&&i| w.get(i).is_some_and(|o| o.template == "g_i_trapkit004")).count();
    assert!(w.disarm_trap(pc, mine, true).unwrap());
    let after = w.party_inventory().iter().filter(|&&i| w.get(i).is_some_and(|o| o.template == "g_i_trapkit004")).count();
    assert!(after > before || w.party_inventory().iter().any(|&i| w.get(i).is_some_and(|o| o.template == "g_i_trapkit004")));
}

fn k2_world_set_detected(w: &mut World, trap: ObjectId, by: ObjectId) {
    w.get_mut(trap).unwrap().trap.as_mut().unwrap().detected_by.insert(by);
}

/// A player mine next to the 001ebo engine-room door blows it open (the
/// prologue's "use a mine to open this door").
#[test]
fn mine_blows_the_engine_room_door() {
    let Some(mut w) = world() else { return };
    w.enter_module("001ebo", None).unwrap();
    // Let the intro scripts settle and hand control over.
    run(&mut w, 1.0);
    while w.conversation.is_some() {
        w.end_conversation(true);
        run(&mut w, 0.2);
    }
    let me = w.party_leader();
    let door = w.by_tag("engine_door", 0).expect("engine_door");
    assert!(!w.get(door).unwrap().door().unwrap().open);
    let (pos, bearing) = {
        let o = w.get(door).unwrap();
        (o.position, o.bearing)
    };
    let n = Vec2::new(-bearing.sin(), bearing.cos());
    let (spot, side) = [1.0f32, -1.0]
        .into_iter()
        .find_map(|s| {
            let p = pos.truncate() + n * 1.6 * s;
            w.walk.ground(p.x, p.y, pos.z + 1.0).map(|g| (p.extend(g.z), s))
        })
        .expect("ground by the door");
    {
        let o = w.get_mut(me).unwrap();
        o.position = spot;
        // Face the door.
        let d = -n * side;
        o.bearing = -d.x.atan2(d.y);
    }
    w.give_new_item("g_i_trapkit004", me, 1, true).expect("mine item");
    let item = w.party_mines().first().map(|m| m.0).expect("a mine in the party inventory");
    let mine = w.set_mine(me, item).expect("set mine");
    assert!(w.trap(mine).unwrap().fuse.is_some(), "a mine by a blastable door is a breaching charge");
    // Back away, as the door's text says.
    let away = spot.truncate() - (-n * side) * 6.0;
    if let Some(g) = w.walk.ground(away.x, away.y, spot.z + 1.0) {
        w.get_mut(me).unwrap().position = away.extend(g.z);
    }
    let ev = run(&mut w, 6.0);
    let fired = ev.iter().any(|e| matches!(e, WorldEvent::TrapTriggered { trap, .. } if *trap == mine));
    let open = w.get(door).unwrap().door().unwrap().open;
    println!("001ebo: mine fired={fired}, engine_door open={open}");
    assert!(fired);
    assert!(open, "the blast should open the engine-room door");
}

#[test]
fn stealth_hides_from_hostiles() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    let belt = w.create_item("a_belt_03", pc, 1).expect("stealth field generator");
    w.equip(pc, belt, 10).expect("equip belt");
    w.set_skill_rank(pc, 2, 30);
    w.set_stealth(pc, true).expect("enter stealth");
    assert!(w.is_stealthed(pc));
    // A hostile droid 6 m away.
    let candidates: Vec<ObjectId> = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && o.id != pc && o.creature().is_some_and(|c| !c.dead))
        .map(|o| o.id)
        .collect();
    let droid = candidates.into_iter().find(|&d| w.is_enemy(d, pc)).expect("a hostile");
    let dp = w.get(droid).unwrap().position;
    let mut spot = None;
    for k in 0..16 {
        let a = k as f32 * std::f32::consts::TAU / 16.0;
        let p = dp.truncate() + Vec2::new(a.cos(), a.sin()) * 6.0;
        if let Some(g) = w.walk.ground(p.x, p.y, dp.z + 1.0) {
            spot = Some(p.extend(g.z));
            break;
        }
    }
    w.get_mut(pc).unwrap().position = spot.expect("ground near the droid");
    run(&mut w, 1.0);
    assert!(w.hidden_from(droid, pc), "Stealth 30 should hide from a droid 6 m away");
    // Up close the droid sees through it.
    let close = dp + (spot.unwrap() - dp).normalize() * 1.0;
    w.get_mut(pc).unwrap().position = close;
    run(&mut w, 1.0);
    assert!(!w.hidden_from(droid, pc));
}

#[test]
fn influence_and_alignment_feedback() {
    let Some(mut w) = world() else { return };
    w.enter_module("103per", None).unwrap();
    let pc = w.party_leader();
    let before = w.influence_of(8);
    // 103atton.dlg runs a_influence_dec with params (8 = T3-M4, 1 = low).
    let ctx = ScriptCtx { params: [8, 1, 0, 0, 0], ..Default::default() };
    w.run_script("a_influence_dec", pc, ctx);
    let ev = w.drain_events();
    let after = w.influence_of(8);
    println!("T3-M4 influence {before} -> {after}");
    assert!(after < before);
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::Feedback { text, .. } if text.starts_with("Influence Lost"))), "{ev:?}");
    let ctx = ScriptCtx { params: [6, 3, 0, 0, 0], ..Default::default() };
    w.run_script("a_influence_inc", pc, ctx);
    let ev = w.drain_events();
    assert!(w.influence_of(6) > 50);
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::Feedback { text, .. } if text.contains("Influence Gained") && text.contains("Kreia"))), "{ev:?}");
    let ge = w.get(pc).unwrap().creature().unwrap().good_evil;
    w.run_script("a_givedark", pc, ScriptCtx { params: [3, 0, 0, 0, 0], ..Default::default() });
    let ev = w.drain_events();
    let ge2 = w.get(pc).unwrap().creature().unwrap().good_evil;
    println!("alignment {ge} -> {ge2}");
    assert!(ge2 < ge, "a_givedark should shift towards the dark side");
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::Feedback { cue: Some(c), .. } if c == "DarkSide")), "{ev:?}");
}
