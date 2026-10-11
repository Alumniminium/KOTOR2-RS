//! Save/load against the real install. Skips (passes) when no install is
//! found at K2_INSTALL or ../.. (the repo inside the game folder).

use k2_res::Install;
use k2_script::ObjectId;
use k2_world::save::SaveData;
use k2_world::{Action, ObjectKind, World};

const DT: f32 = 1.0 / 30.0;

fn install() -> Option<Install> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    Install::discover(dir).ok()
}

fn dead(w: &World, id: ObjectId) -> bool {
    w.get(id).and_then(|o| o.creature()).is_none_or(|c| c.dead)
}

/// Nearest living hostile to the leader, ties broken by id (deterministic).
fn nearest_hostile(w: &mut World) -> Option<ObjectId> {
    let leader = w.party_leader();
    let lp = w.get(leader)?.position;
    let mut c: Vec<(ObjectId, f32)> = w
        .area_creatures()
        .into_iter()
        .filter(|&id| !dead(w, id))
        .filter_map(|id| Some((id, w.get(id)?.position.distance(lp))))
        .collect();
    c.retain(|&(id, _)| w.is_enemy(id, leader));
    c.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    c.first().map(|x| x.0)
}

/// Same input every frame in both worlds: the leader attacks the nearest
/// hostile whenever idle; conversations auto-pick reply 0.
fn drive(w: &mut World, seconds: f32) {
    let mut t = 0.0;
    while t < seconds {
        let leader = w.party_leader();
        let attacking = w.get(leader).is_some_and(|o| o.actions.iter().any(|a| matches!(a, Action::Attack(_))));
        if !attacking && w.conversation.is_none() && !dead(w, leader)
            && let Some(h) = nearest_hostile(w)
        {
            w.player_attack(h);
        }
        w.tick(DT);
        if w.conversation.is_some() {
            match w.conversation_view() {
                Some(v) if v.replies.is_empty() => w.advance_conversation(),
                Some(_) => w.choose_reply(0),
                None => w.end_conversation(true),
            }
        }
        let _ = w.drain_events();
        t += DT;
    }
}

/// Saves `a`, loads the save into a fresh world and runs both side by side
/// for 10 s with identical input, comparing state digests every second.
fn round_trip_matches(inst: Install, a: &mut World, reseed_loaded: bool) -> Result<usize, String> {
    let _ = a.drain_events();
    let bytes = a.save().to_bytes();
    let mut b = World::load(inst, &SaveData::from_bytes(&bytes).unwrap()).unwrap();
    let _ = b.drain_events();
    if reseed_loaded {
        b.combat.seed(12345);
    }
    if a.state_digest() != b.state_digest() {
        return Err("digest differs right after load".into());
    }
    for second in 1..=10 {
        drive(a, 1.0);
        drive(&mut b, 1.0);
        if a.state_digest() != b.state_digest() {
            return Err(format!("diverged after {second} s"));
        }
    }
    assert!(a.vm_errors.is_empty(), "{:?}", a.vm_errors);
    assert!(b.vm_errors.is_empty(), "{:?}", b.vm_errors);
    Ok(bytes.len())
}

fn hostiles_alive(w: &mut World) -> usize {
    let leader = w.party_leader();
    let ids = w.area_creatures();
    ids.into_iter().filter(|&id| !dead(w, id) && w.is_enemy(id, leader)).count()
}

#[test]
fn save_round_trip_after_a_fight_is_deterministic() {
    let Some(inst) = install() else { return };
    let mut a = World::new(inst.clone()).unwrap();
    a.enter_module("102per", None).unwrap();
    // 30 s of 102per with a fight: to the first death, then keep fighting.
    let r = k2_world::combat::fight(&mut a, 30.0, DT).expect("a fight");
    drive(&mut a, (30.0 - r.seconds).max(1.0));
    let n = round_trip_matches(inst, &mut a, false).unwrap();
    eprintln!("30 s save: {n} bytes");
}

#[test]
fn save_round_trip_mid_combat_is_deterministic() {
    let Some(inst) = install() else { return };
    let mut a = World::new(inst.clone()).unwrap();
    a.enter_module("102per", None).unwrap();
    // Stop 2 s into the fight: rounds, attack queues and AI timers in flight.
    let _ = k2_world::combat::fight(&mut a, 2.0, DT).expect("a fight");
    let leader = a.party_leader();
    assert!(a.in_combat(leader), "saved mid-combat");
    let alive = hostiles_alive(&mut a);
    assert!(alive > 0);
    let n = round_trip_matches(inst.clone(), &mut a, false).unwrap();
    eprintln!("mid-combat save: {n} bytes, {alive} hostiles alive at save");

    // Control: the comparison does notice a different combat RNG.
    let mut c = World::new(inst.clone()).unwrap();
    c.enter_module("102per", None).unwrap();
    let _ = k2_world::combat::fight(&mut c, 2.0, DT).expect("a fight");
    assert!(round_trip_matches(inst, &mut c, true).is_err(), "reseeding should diverge");
}

/// What a mid-fight save has to bring back, per creature: HP, whether it is
/// dead, in combat, position and its action queue (as text; Action is Debug).
fn fight_snapshot(w: &World) -> Vec<(u32, i32, bool, bool, String, String)> {
    let mut v: Vec<_> = w
        .iter()
        .filter_map(|o| {
            let c = o.creature()?;
            Some((o.id, c.hp, c.dead, w.in_combat(o.id), format!("{:?}", o.position), format!("{:?}", o.actions)))
        })
        .collect();
    v.sort();
    v
}

#[test]
fn saving_is_allowed_mid_fight_and_restores_the_fight() {
    let Some(inst) = install() else { return };
    let mut a = World::new(inst.clone()).unwrap();
    a.enter_module("102per", None).unwrap();
    let _ = k2_world::combat::fight(&mut a, 1.0, DT).expect("a fight");
    // Over the next seconds of the fight, count the frames in combat where
    // saving is allowed: combat itself must never refuse.
    let (mut in_fight, mut allowed, mut refused_by_combat) = (0, 0, 0);
    let mut saved_in_fight = None;
    for _ in 0..(8.0 / DT) as usize {
        drive(&mut a, DT);
        let leader = a.party_leader();
        if a.in_combat(leader) {
            in_fight += 1;
            match a.save_blocked() {
                None => {
                    allowed += 1;
                    saved_in_fight.get_or_insert_with(|| a.save().to_bytes());
                }
                Some(why) if why.contains("combat") => refused_by_combat += 1,
                Some(_) => {}
            }
        }
    }
    eprintln!("mid-fight: {allowed} of {in_fight} combat frames saveable");
    assert_eq!(refused_by_combat, 0);
    assert!(in_fight > 0 && allowed > 0, "never saveable while in combat");
    assert!(saved_in_fight.is_some());

    // Save at a moment in combat with attacks queued and load it back.
    let mut a = World::new(inst.clone()).unwrap();
    a.enter_module("102per", None).unwrap();
    let _ = k2_world::combat::fight(&mut a, 2.0, DT).expect("a fight");
    let leader = a.party_leader();
    assert!(a.in_combat(leader));
    assert!(a.save_blocked().is_none(), "{:?}", a.save_blocked());
    let queued = a.get(leader).is_some_and(|o| !o.actions.is_empty());
    let _ = a.drain_events();
    let bytes = a.save().to_bytes();
    let b = World::load(inst, &SaveData::from_bytes(&bytes).unwrap()).unwrap();
    assert_eq!(fight_snapshot(&a), fight_snapshot(&b), "HP, deaths, combat flags, positions and action queues");
    assert_eq!(a.in_combat(leader), b.in_combat(leader));
    assert!(b.in_combat(b.party_leader()), "still in combat after loading");
    assert_eq!(a.state_digest(), b.state_digest());
    eprintln!("leader had actions queued: {queued}");
}

/// The save made on exit is allowed during a conversation, and the loaded
/// world is a normal one: no conversation, saving allowed, no script errors,
/// the NPC and the party keep running.
#[test]
fn exit_save_in_a_conversation_loads_cleanly() {
    let Some(inst) = install() else { return };
    let mut a = World::new(inst.clone()).unwrap();
    a.enter_module("101per", None).unwrap();
    let pc = a.party_leader();
    let npcs: Vec<(ObjectId, String)> = a
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && o.id != pc)
        .filter_map(|o| o.conversation.clone().map(|d| (o.id, d)))
        .collect();
    let mut started = None;
    for (npc, dlg) in npcs {
        if a.start_conversation(npc, pc, &dlg) && a.conversation_view().is_some() {
            started = Some((npc, dlg));
            break;
        }
        a.end_conversation(true);
    }
    let (npc, dlg) = started.expect("a conversation in 101per");
    assert_eq!(a.save_blocked(), Some("in conversation"));
    assert_eq!(a.exit_save_blocked(), None, "{dlg}");
    let _ = a.drain_events();
    let bytes = a.save().to_bytes();
    let mut b = World::load(inst, &SaveData::from_bytes(&bytes).unwrap()).unwrap();
    assert!(b.conversation.is_none());
    drive(&mut b, 5.0);
    assert!(b.vm_errors.is_empty(), "{:?}", b.vm_errors);
    assert_eq!(b.save_blocked(), None, "the loaded world can be saved again");
    assert!(b.get(npc).is_some() && b.get(b.party_leader()).is_some());
}

#[test]
fn left_modules_remember_their_dead() {
    let Some(inst) = install() else { panic!("no install") };
    let mut w = World::new(inst).unwrap();
    w.enter_module("102per", None).unwrap();
    // Kill every mining droid outright.
    let droids: Vec<ObjectId> = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && o.tag.to_ascii_lowercase().contains("drd"))
        .map(|o| o.id)
        .collect();
    assert!(!droids.is_empty(), "102per has droids");
    let tags: Vec<String> = droids.iter().filter_map(|&d| w.get(d).map(|o| o.tag.clone())).collect();
    for &d in &droids {
        if let Some(c) = w.get_mut(d).and_then(|o| o.creature_mut()) {
            c.hp = 0;
            c.dead = true;
        }
    }
    w.enter_module("101per", None).unwrap();
    assert!(w.module_memory.contains_key("102per"));
    w.enter_module("102per", None).unwrap();
    for (id, tag) in droids.iter().zip(&tags) {
        // Same object, still dead, not respawned from the GIT.
        assert!(dead(&w, *id) || w.get(*id).is_none(), "{tag} came back");
        let live = w.iter().filter(|o| o.tag == *tag && o.creature().is_some_and(|c| !c.dead)).count();
        assert_eq!(live, 0, "{tag} respawned");
    }
    // And it survives a save in between.
    let data = SaveData::from_bytes(&w.save().to_bytes()).unwrap();
    assert_eq!(data.module, "102per");
}
