//! Shouts (SpeakString) reach listening creatures: within the talk volume's
//! range, with a listen pattern that matches the text. A party member that is
//! attacked shouts GEN_I_WAS_ATTACKED (k_hen_attacked01), and the other party
//! members answer it by joining the fight (GN_RespondToShout). Skips (passes)
//! when no install is found.

use std::io::Write;
use glam::Vec3;
use k2_res::Install;
use k2_script::ObjectId;
use k2_world::object::Action;
use k2_world::{ObjectKind, World, WorldEvent};

const DT: f32 = 1.0 / 30.0;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(&dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP shouts test: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    let mut w = World::new(install).unwrap_or_else(|e| panic!("{dir}: the install does not load into a world: {e:?}"));
    w.auto_dialog = true;
    w.enter_module("101per", None).unwrap();
    for _ in 0..60 {
        w.tick(DT);
        let _ = w.drain_events();
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
    }
    Some(w)
}

/// The nearest living, ranged-armed creature that is not in the party.
fn ranged_hostile(w: &mut World, from: Vec3) -> Option<ObjectId> {
    let mut ids: Vec<(f32, ObjectId)> = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && !w.party.contains(&o.id))
        // The default creature AI (k_def_percept01), not a scripted hench or plot creature.
        .filter(|o| o.scripts.get("ScriptOnNotice").is_some_and(|s| s == "k_def_percept01"))
        .filter(|o| o.creature().is_some_and(|c| !c.dead))
        .map(|o| (o.position.distance(from), o.id))
        .collect();
    ids.sort_by(|a, b| a.0.total_cmp(&b.0));
    ids.into_iter().map(|(_, id)| id).find(|&id| w.combat_stats(id).is_some_and(|s| s.main.ranged))
}

/// A walkable point `dist` metres from `from` in direction `angle`.
fn ground_at(w: &World, from: Vec3, dist: f32, angle: f32) -> Option<Vec3> {
    let p = from + Vec3::new(angle.cos(), angle.sin(), 0.0) * dist;
    w.walk.ground(p.x, p.y, from.z).map(|h| Vec3::new(p.x, p.y, h.z))
}

/// A walkable point `dist` metres from `from` that is also `min_away` metres
/// from `avoid`, searching all directions.
fn ground_away(w: &World, from: Vec3, dist: f32, avoid: Vec3, min_away: f32) -> Option<Vec3> {
    (0..72).map(|k| k as f32 * 5f32.to_radians()).filter_map(|a| ground_at(w, from, dist, a)).find(|p| p.distance(avoid) >= min_away)
}

#[test]
fn shout_reaches_listening_creatures_in_range_with_a_matching_pattern() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    let base = w.get(leader).unwrap().position;
    let speaker = w.spawn_creature("p_hk47", base + Vec3::new(40.0, 0.0, 0.0), 0.0).expect("HK-47");
    let near = w.spawn_creature("p_mand", base + Vec3::new(50.0, 0.0, 0.0), 0.0).expect("Mandalore");
    let far = w.spawn_creature("p_hk47", base + Vec3::new(70.0, 0.0, 0.0), 0.0).expect("a second HK-47");
    let mute = w.spawn_creature("p_mand", base + Vec3::new(52.0, 0.0, 0.0), 0.0).expect("a deaf creature");
    // Let the spawn scripts run (they set listening and the shout patterns);
    // then set the listeners' state explicitly so the test does not depend on them.
    for _ in 0..30 {
        w.tick(DT);
        let _ = w.drain_events();
    }
    w.get_mut(speaker).unwrap().position = base + Vec3::new(40.0, 0.0, 0.0);
    w.get_mut(near).unwrap().position = base + Vec3::new(50.0, 0.0, 0.0);
    w.get_mut(far).unwrap().position = base + Vec3::new(70.0, 0.0, 0.0);
    w.get_mut(mute).unwrap().position = base + Vec3::new(52.0, 0.0, 0.0);
    for id in [near, far] {
        let c = w.get_mut(id).unwrap().creature_mut().unwrap();
        c.listening = true;
        c.listen_patterns = vec![("GEN_I_WAS_ATTACKED".to_string(), 1)];
    }
    let c = w.get_mut(mute).unwrap().creature_mut().unwrap();
    c.listening = false;
    c.listen_patterns = vec![("GEN_I_WAS_ATTACKED".to_string(), 1)];

    // Silent shout (4) carries 40 m: the near and the far listener both hear it;
    // the deaf one does not.
    let heard = w.hear_speech(speaker, "GEN_I_WAS_ATTACKED", 4);
    let ids: Vec<ObjectId> = heard.iter().map(|&(id, _)| id).collect();
    assert!(ids.contains(&near) && ids.contains(&far), "the shout reaches both listeners within 40 m: {ids:?}");
    assert!(!ids.contains(&mute), "a creature that is not listening hears nothing");
    assert!(heard.iter().filter(|&&(id, _)| id == near || id == far).all(|&(_, n)| n == 1), "the pattern number is passed on");

    // Talk (0) carries 20 m: the far listener, at about 30 m, does not hear it.
    let talk = w.hear_speech(speaker, "GEN_I_WAS_ATTACKED", 0);
    let talk_ids: Vec<ObjectId> = talk.iter().map(|&(id, _)| id).collect();
    assert!(talk_ids.contains(&near) && !talk_ids.contains(&far), "talk carries 20 m: {talk_ids:?}");

    // A different string matches no pattern; patterns match ignoring case.
    assert!(!w.hear_speech(speaker, "GEN_I_AM_DEAD", 4).iter().any(|&(id, _)| id == near), "no pattern for that string");
    assert!(w.hear_speech(speaker, "gen_i_was_attacked", 4).iter().any(|&(id, _)| id == far), "case does not matter");

    // The queued responses run without starting a conversation: a shout answers
    // in combat, it does not open dialogue.
    for _ in 0..10 {
        w.tick(DT);
        let _ = w.drain_events();
    }
    assert!(w.conversation.is_none(), "a shout starts no conversation");
}

#[test]
fn a_knocked_out_listener_hears_nothing() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    let base = w.get(leader).unwrap().position;
    let speaker = w.spawn_creature("p_hk47", base + Vec3::new(40.0, 0.0, 0.0), 0.0).expect("HK-47");
    let listener = w.spawn_creature("p_mand", base + Vec3::new(45.0, 0.0, 0.0), 0.0).expect("Mandalore");
    for _ in 0..30 {
        w.tick(DT);
        let _ = w.drain_events();
    }
    w.get_mut(speaker).unwrap().position = base + Vec3::new(40.0, 0.0, 0.0);
    w.get_mut(listener).unwrap().position = base + Vec3::new(45.0, 0.0, 0.0);
    let c = w.get_mut(listener).unwrap().creature_mut().unwrap();
    c.listening = true;
    c.listen_patterns = vec![("GEN_I_WAS_ATTACKED".to_string(), 1)];

    let up = w.hear_speech(speaker, "GEN_I_WAS_ATTACKED", 4);
    assert!(up.iter().any(|&(id, _)| id == listener), "a standing listener hears the shout");

    w.get_mut(listener).unwrap().creature_mut().unwrap().down = true;
    let down = w.hear_speech(speaker, "GEN_I_WAS_ATTACKED", 4);
    assert!(!down.iter().any(|&(id, _)| id == listener), "a knocked-out listener hears nothing");
}

#[test]
fn companion_joins_when_a_party_member_is_attacked() {
    // The enemy shoots HK-47; HK-47's OnAttacked shouts, and Mandalore, another
    // party member out of the enemy's sight, answers the shout and attacks.
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    let lp = w.get(leader).unwrap().position;
    let hk = w.spawn_creature("p_hk47", lp + Vec3::new(-2.0, 1.0, 0.0), 0.0).expect("HK-47");
    let mand = w.spawn_creature("p_mand", lp + Vec3::new(-3.0, 2.0, 0.0), 0.0).expect("Mandalore");
    w.party.push(hk);
    w.party.push(mand);
    // A creature made at runtime runs its spawn script: the companion scripts set
    // their listening patterns there (GN_SetListeningPatterns).
    for id in [hk, mand] {
        w.fire(id, "ScriptSpawn", k2_world::ScriptCtx::default());
    }
    for _ in 0..30 {
        w.tick(DT);
        let _ = w.drain_events();
    }

    let enemy = ranged_hostile(&mut w, lp).expect("a ranged creature in 101per");
    w.get_mut(enemy).unwrap().creature_mut().unwrap().faction = 1;
    // Durable, so the enemy outlives the companion's first shots.
    let c = w.get_mut(enemy).unwrap().creature_mut().unwrap();
    c.max_hp = 500;
    c.hp = 500;
    let ep = w.get(enemy).unwrap().position;
    let hp = ground_away(&w, ep, 4.0, lp, 1.0).expect("ground next to the enemy");
    w.place(hk, hp);
    let mp = ground_away(&w, ep, 12.0, ep, 10.0).expect("ground 12 m from the enemy");
    w.place(mand, mp);
    w.push_action(enemy, Action::Attack(hk));
    let mut mand_attacks = 0;
    let mut t = 0.0;
    while t < 40.0 {
        w.tick(DT);
        t += DT;
        for e in w.drain_events() {
            if let WorldEvent::AttackStarted { attacker, target, .. } = e
                && attacker == mand
                && target == enemy
            {
                mand_attacks += 1;
            }
        }
    }
    assert!(mand_attacks >= 1, "Mandalore answers HK-47's shout and attacks the enemy");
    assert!(w.conversation.is_none(), "the shout answers in combat and starts no conversation");
}
