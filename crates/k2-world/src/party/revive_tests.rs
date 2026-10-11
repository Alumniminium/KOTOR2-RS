//! Unit tests that need the queues and the routine table: the event queue is
//! private to the crate root, so these cannot be integration tests.

use std::io::Write;
use super::*;
use crate::progression::LevelUpChoice;
use crate::Pending;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = k2_res::Install::discover(&dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP party revive unit tests: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    let mut w = World::new(install).expect("world");
    w.auto_dialog = true;
    w.enter_module("102per", None).expect("102per");
    Some(w)
}

/// Kreia joins as the second member; returns her id.
fn with_kreia(w: &mut World) -> ObjectId {
    w.available_npcs.insert(6, crate::AvailableNpc { template: "p_kreia".into(), selectable: true, ..Default::default() });
    w.apply_party_selection(&[6]).unwrap();
    w.party.iter().copied().find(|&p| p != w.party_leader()).expect("Kreia joined")
}

/// Event scripts queued for `object` with `label` and not yet run.
fn queued(w: &World, object: ObjectId, label: &str) -> usize {
    w.pending.iter().filter(|p| matches!(p, Pending::Event { object: o, label: l, .. } if *o == object && l == label)).count()
}

fn hp_of(w: &World, id: ObjectId) -> i32 {
    w.get(id).and_then(|o| o.creature()).map_or(-1, |c| c.hp)
}

/// The downed member killed by EffectDeath queues ScriptDeath once (the
/// knock-down did) and calls party_member_down zero times: the party is
/// already defeated, so a call would report PartyDefeated again.
#[test]
fn effect_death_on_a_downed_member_queues_script_death_once_and_hands_over_nothing() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    let kreia = with_kreia(&mut w);
    w.harm(kreia, 10_000, OBJECT_INVALID, false);
    assert_eq!(queued(&w, kreia, "ScriptDeath"), 1, "the knock-down queues ScriptDeath once");
    w.harm(leader, 10_000, OBJECT_INVALID, false);
    let ev = w.drain_events();
    assert_eq!(ev.iter().filter(|e| matches!(e, WorldEvent::PartyDefeated)).count(), 1, "defeat not reported once: {ev:?}");
    w.harm(kreia, i32::MAX / 2, leader, true);
    let ev = w.drain_events();
    assert_eq!(queued(&w, kreia, "ScriptDeath"), 1, "kill() queued ScriptDeath again");
    assert!(!ev.iter().any(|e| matches!(e, WorldEvent::PartyDefeated | WorldEvent::LeaderChanged { .. })), "party_member_down ran again: {ev:?}");
    assert!(ev.iter().any(|e| matches!(e, WorldEvent::Died { object, .. } if *object == kreia)), "no Died: {ev:?}");
}

/// Level-up adds HP: a dead or knocked-out creature must not take it.
#[test]
fn the_dead_and_the_knocked_out_cannot_level_up() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    let kreia = with_kreia(&mut w);
    w.harm(kreia, 10_000, OBJECT_INVALID, false);
    let before = hp_of(&w, kreia);
    assert!(w.apply_level_up_unchecked(kreia, &LevelUpChoice::default()).is_err(), "a downed creature levelled up");
    assert_eq!(hp_of(&w, kreia), before);
    w.slay_creature(kreia, leader);
    assert!(w.apply_level_up_unchecked(kreia, &LevelUpChoice::default()).is_err(), "a dead creature levelled up");
    let c = w.get(kreia).and_then(|o| o.creature()).unwrap();
    assert!(c.dead && c.hp == 0, "dead {} hp {} after refused level-up", c.dead, c.hp);
}

