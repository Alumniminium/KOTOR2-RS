use super::*;
use std::io::Write;

/// A world in the prologue (its area has the a_bet3m4 script), or None (the
/// test is skipped) without an install.
fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP area loop tests: no install (set K2_INSTALL)");
        return None;
    };
    let mut w = World::new(install).unwrap();
    w.enter_module("001ebo", None).unwrap();
    // The prologue's own scripts leave resumes pending: the tests add their own.
    w.timers.clear();
    Some(w)
}

/// A pending resume of the area's own script, due `delay` seconds from now, as
/// a DelayCommand in it would leave.
fn area_resume(w: &mut World, script: &str, delay: f32) {
    let area = w.area_id;
    let script = w.load_script(script).expect("the area script is in the install");
    let state = ScriptState { script, offset: 0, globals: Vec::new(), locals: Vec::new(), caller: area };
    w.add_timer(delay, TimerKind::Resume(Arc::new(state)));
}

#[test]
fn a_one_shot_area_delay_holds_the_party_and_an_area_loop_does_not() {
    let Some(mut w) = world() else { return };
    area_resume(&mut w, "a_bet3m4", 5.0);
    assert!(!w.resumes_settled(), "a one-shot area delay holds settled()");
    w.record_area_turn("a_bet3m4", true);
    assert!(w.resumes_settled(), "an area loop holds nothing");
    w.record_area_turn("a_bet3m4", false);
    assert!(!w.resumes_settled(), "a loop that stopped re-arming holds again");
}

#[test]
fn a_loop_is_keyed_by_area_and_a_module_change_clears_it() {
    let Some(mut w) = world() else { return };
    w.record_area_turn("a_bet3m4", true);
    let area = w.area_id;
    assert!(w.looping_scripts.contains(&(area, "a_bet3m4".to_owned())));
    // Another area id (the party leader stands in for one) does not share the loop.
    let other = w.party_leader();
    assert_ne!(other, area, "the prologue has a second object to stand in for another area");
    w.area_id = other;
    area_resume(&mut w, "a_bet3m4", 5.0);
    assert!(!w.resumes_settled(), "a loop of one area does not make the same script ambient in another");
    w.area_id = area;
    w.enter_module("201tel", None).unwrap();
    assert!(w.looping_scripts.is_empty(), "a new module kept the loops of the old one");
    area_resume(&mut w, "a_bet3m4", 5.0);
    assert!(!w.resumes_settled(), "the old loop does not hold back a one-shot delay in the new module");
}

/// The resume arm (tick.rs) keeps a loop while its next turn is queued.
#[test]
fn the_resume_arm_keeps_a_loop_while_its_next_turn_is_queued() {
    let Some(mut w) = world() else { return };
    w.record_area_turn("a_bet3m4", true);
    // The turn that fires now, and the loop's next turn queued for later.
    area_resume(&mut w, "a_bet3m4", 0.0);
    area_resume(&mut w, "a_bet3m4", 60.0);
    w.tick(1.0 / 30.0);
    assert!(w.looping_scripts.contains(&(w.area_id, "a_bet3m4".to_owned())), "a turn with the loop's next turn still queued keeps the loop");
}

/// The resume arm ends a loop on the turn that queues no next resume.
#[test]
fn the_resume_arm_ends_a_loop_whose_turn_queues_nothing() {
    let Some(mut w) = world() else { return };
    w.record_area_turn("a_bet3m4", true);
    area_resume(&mut w, "a_bet3m4", 0.0);
    w.tick(1.0 / 30.0);
    // Whatever the script queued, the key follows it: kept exactly when a next turn is queued.
    let next_turn = w.timers.iter().any(|t| matches!(&t.kind, TimerKind::Resume(s) if s.caller == w.area_id && s.script.name == "a_bet3m4"));
    assert_eq!(w.looping_scripts.contains(&(w.area_id, "a_bet3m4".to_owned())), next_turn, "the loop ends on the turn it stops re-arming");
}
