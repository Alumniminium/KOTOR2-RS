//! Bare lines (barks) and conversations against the real install. Skips
//! (passes, printing why) when no install is found at K2_INSTALL or ../..
//! (the repo inside the game folder). With K2_INSTALL set, an install that
//! does not load is a failure.
//!
//! A bark is a conversation whose starting line has no reply links at all.
//! Any reply link, even a single [Continue], makes a real conversation.

use std::io::Write;
use k2_data::Dialog;
use k2_res::Install;
use k2_world::{Action, ObjectKind, World, WorldEvent};

/// The install under test: K2_INSTALL, else the repo inside the game folder.
/// None when there is no install there (the test skips). With K2_INSTALL set,
/// a directory that is not an install panics.
fn install() -> Option<Install> {
    let env = std::env::var("K2_INSTALL").ok();
    let dir = env.clone().unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    match Install::discover(&dir) {
        Ok(install) => Some(install),
        Err(e) if env.is_some() => panic!("K2_INSTALL={dir} is not an install: {e}"),
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "SKIP barks: no install at {dir} ({e})");
            None
        }
    }
}

fn world() -> Option<World> {
    let install = install()?;
    Some(World::new(install).expect("the install loads into a world"))
}

/// The BarkLine events in `evs`: (speaker, dialog, text).
fn bark_lines(evs: &[WorldEvent]) -> Vec<(u32, String, String)> {
    evs.iter()
        .filter_map(|e| match e {
            WorldEvent::BarkLine { speaker, dialog, text, .. } => Some((*speaker, dialog.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

/// Dialogs the install does not ship (the Onderon and Telos scripts still name
/// them). A survey may hit these without failing.
const NOT_SHIPPED: &[&str] = &["sion", "351ycs"];

/// What a survey could not load. Every failure is printed to stderr as it
/// happens; at the end, a module that does not load, or a dialog the install
/// ships but does not load, fails the survey.
#[derive(Default)]
struct Failures {
    seen: usize,
    unexpected: Vec<String>,
}

impl Failures {
    fn module(&mut self, m: &str, err: impl std::fmt::Display) {
        let msg = err.to_string();
        self.seen += 1;
        eprintln!("{m}: module does not load: {msg}");
        self.unexpected.push(format!("module {m}: {msg}"));
    }

    fn dialog(&mut self, m: &str, name: &str, err: impl std::fmt::Display) {
        let msg = err.to_string();
        self.seen += 1;
        eprintln!("{m}: dialog {name} does not load: {msg}");
        if !NOT_SHIPPED.contains(&name) {
            self.unexpected.push(format!("{m}: dialog {name}: {msg}"));
        }
    }

    fn finish(self) {
        eprintln!("survey: {} load failures, {} unexpected", self.seen, self.unexpected.len());
        assert!(self.unexpected.is_empty(), "loads that must succeed failed:\n{}", self.unexpected.join("\n"));
    }
}

/// A referenced dialog, or None after recording why it did not load.
fn load_dialog(w: &World, m: &str, name: &str, failures: &mut Failures) -> Option<Dialog> {
    match Dialog::load(&w.res, name) {
        Ok(dlg) => Some(dlg),
        Err(e) => {
            failures.dialog(m, name, e);
            None
        }
    }
}

/// The bare lines `speaker` barked in `evs`.
fn barks_by(evs: &[WorldEvent], speaker: u32) -> Vec<(u32, String, String)> {
    bark_lines(evs).into_iter().filter(|b| b.0 == speaker).collect()
}

#[test]
fn caged_animal_line_barks_without_a_conversation() {
    let Some(mut w) = world() else { return };
    w.enter_module("501ond", None).unwrap();
    let pc = w.party_leader();
    let animal = w
        .iter()
        .find(|o| o.kind == ObjectKind::Creature && o.conversation.as_deref() == Some("tame"))
        .map(|o| o.id)
        .expect("501ond has a tame caged animal");
    w.drain_events();

    w.talk_to(pc, animal);
    assert!(w.conversation.is_none(), "a bare line keeps no conversation state");
    assert!(w.conversation_view().is_none());
    let barked = bark_lines(&w.drain_events());
    assert_eq!(barked.len(), 1, "one bark for one click: {barked:?}");
    assert_eq!(barked[0].0, animal);
    assert_eq!(barked[0].1, "tame");
    assert!(barked[0].2.contains("restless"), "{barked:?}");

    w.use_object(pc, animal);
    assert_eq!(bark_lines(&w.drain_events()).len(), 1, "clicking says the same line once");
    assert!(w.conversation.is_none());
}

#[test]
fn a_queued_start_from_earlier_does_not_block_talk() {
    let Some(mut w) = world() else { return };
    w.enter_module("501ond", None).unwrap();
    // Let the module's entry cutscenes run out first (Talia's intro starts on
    // the first ticks and would otherwise be open during the test).
    w.auto_dialog = true;
    for _ in 0..300 {
        w.tick(0.1);
    }
    w.auto_dialog = false;
    w.end_conversation(true);
    let pc = w.party_leader();
    let animal = w
        .iter()
        .find(|o| o.kind == ObjectKind::Creature && o.conversation.as_deref() == Some("tame"))
        .map(|o| o.id)
        .expect("501ond has a tame caged animal");
    assert!(w.get(animal).unwrap().script("ScriptDialogue").is_none(), "no ScriptDialogue, so talk_to takes the fallback");
    w.drain_events();

    // A StartConversation queued before this talk is not the script's, so it
    // does not hold the fallback back: the bare line barks at once.
    w.get_mut(animal).unwrap().actions.push_back(Action::StartConversation { with: pc, dialog: "tame".into(), bark: None });
    w.talk_to(pc, animal);
    let barked = barks_by(&w.drain_events(), animal);
    assert_eq!(barked.len(), 1, "talk_to barks the bare line: {barked:?}");
    assert!(w.conversation.is_none());
}

#[test]
fn a_script_queued_talk_barks_once_on_the_next_tick() {
    let Some(mut w) = world() else { return };
    w.enter_module("501ond", None).unwrap();
    // Let the module's entry cutscenes run out first.
    w.auto_dialog = true;
    for _ in 0..300 {
        w.tick(0.1);
    }
    w.auto_dialog = false;
    w.end_conversation(true);
    let pc = w.party_leader();
    let animal = w
        .iter()
        .find(|o| o.kind == ObjectKind::Creature && o.conversation.as_deref() == Some("tame"))
        .map(|o| o.id)
        .expect("501ond has a tame caged animal");
    // ScriptDialogue = k_obj_talk. Only its compiled script is shipped
    // (k_obj_talk.ncs; the .nss source is not in the install), so the claim
    // that it queues exactly one start rests on its NCS call listing: one
    // ActionStartConversation(GetFirstPC) and no other queue call.
    w.get_mut(animal).unwrap().actions.clear();
    w.get_mut(animal).unwrap().scripts.insert("ScriptDialogue".into(), "k_obj_talk".into());
    w.drain_events();

    w.talk_to(pc, animal);
    let starts = |w: &World| w.get(animal).unwrap().actions.iter().filter(|a| matches!(a, Action::StartConversation { .. })).count();
    assert_eq!(starts(&w), 1, "the script queued one start");
    assert!(barks_by(&w.drain_events(), animal).is_empty(), "the queued start is the talk; the fallback does not bark now");
    assert!(w.conversation.is_none());

    w.tick(0.1);
    let barked = barks_by(&w.drain_events(), animal);
    assert_eq!(barked.len(), 1, "exactly one bark after one tick: {barked:?}");
    assert_eq!(starts(&w), 0, "the queued start was used up");
    assert!(w.conversation.is_none());
}

#[test]
fn citizen_with_a_continue_reply_is_a_conversation() {
    let Some(mut w) = world() else { return };
    w.enter_module("501ond", None).unwrap();
    let pc = w.party_leader();
    let npc = w
        .iter()
        .find(|o| o.kind == ObjectKind::Creature && o.conversation.as_deref() == Some("gen_citi"))
        .map(|o| o.id)
        .expect("501ond has an Iziz citizen");
    w.drain_events();

    w.talk_to(pc, npc);
    assert!(bark_lines(&w.drain_events()).is_empty(), "a [Continue] reply makes a conversation, not a bark");
    let view = w.conversation_view().expect("the citizen's line is a conversation");
    assert_eq!(view.dialog, "gen_citi");
    assert!(!view.text.trim().is_empty());
    w.end_conversation(true);
}

#[test]
fn prologue_galaxy_map_and_controls_are_conversations() {
    let Some(mut w) = world() else { return };
    w.enter_module("001ebo", None).unwrap();
    // The prologue's own opening talk may run on entry; the starts under test
    // need the conversation slot.
    w.end_conversation(true);
    let pc = w.party_leader();
    for name in ["glxy_map", "controls"] {
        let dlg = Dialog::load(&w.res, name).expect("the dialog loads");
        assert!((0..dlg.entries.len()).all(|e| !dlg.is_one_liner(e)), "{name} has reply links, so no entry is a bark");
        assert!(w.start_conversation(pc, pc, name), "{name} starts a conversation");
        assert_eq!(w.conversation_view().map(|v| v.dialog), Some(name.to_owned()));
        w.end_conversation(true);
        w.drain_events();
    }
}

#[test]
fn static_turret_keeps_its_facing_and_says_its_line_as_a_conversation() {
    let Some(mut w) = world() else { return };
    w.enter_module("502ond", None).unwrap();
    let pc = w.party_leader();
    let turret = w
        .iter()
        .find(|o| o.kind == ObjectKind::Creature && o.creature().is_some_and(|c| c.not_reorienting))
        .map(|o| o.id)
        .expect("502ond has a defense turret");
    assert_eq!(w.get(turret).unwrap().conversation.as_deref(), Some("turret"));

    // The PC stands in front of the turret; the turret keeps a bearing that
    // points away from the PC.
    let tp = w.get(turret).unwrap().position;
    let away = 1.234;
    w.get_mut(turret).unwrap().bearing = away;
    w.get_mut(pc).unwrap().position = tp + glam::Vec3::new(1.5, 0.0, 0.0);
    w.drain_events();

    w.talk_to(pc, turret);
    assert_eq!(w.get(turret).unwrap().bearing, away, "talk_to turned the turret");
    let view = w.conversation_view().expect("the turret's line is a conversation");
    assert!(view.text.contains("inactive"), "{}", view.text);
    w.end_conversation(true);

    w.use_object(pc, turret);
    assert_eq!(w.get(turret).unwrap().bearing, away, "clicking turned the turret");
    w.end_conversation(true);
    assert!(bark_lines(&w.drain_events()).is_empty());
}

#[test]
fn onderon_bare_lines_are_barks_and_real_dialogs_are_not() {
    let Some(mut w) = world() else { return };
    w.enter_module("501ond", None).unwrap();
    let mut barks = Vec::new();
    let mut dialogs = Vec::new();
    for o in w.iter().filter(|o| o.kind == ObjectKind::Creature) {
        let Some(name) = o.conversation.clone().filter(|s| !s.is_empty()) else { continue };
        let dlg = Dialog::load(&w.res, &name).expect("a referenced dialog loads");
        let starts: Vec<usize> = dlg.starting.iter().map(|l| l.index).collect();
        if !starts.is_empty() && starts.iter().all(|&e| dlg.is_one_liner(e)) {
            barks.push(name)
        } else {
            dialogs.push(name)
        }
    }
    barks.sort();
    barks.dedup();
    dialogs.sort();
    dialogs.dedup();
    assert!(barks.iter().any(|d| d == "tame"), "caged animals bark: {barks:?}");
    assert!(dialogs.iter().any(|d| d == "gen_citi"), "citizens have a [Continue] reply: {dialogs:?}");
    assert!(dialogs.iter().any(|d| d == "margar"), "Margar has a real conversation: {dialogs:?}");
    assert!(dialogs.iter().any(|d| d == "portmstr"), "the port master has a real conversation: {dialogs:?}");
}

/// Survey, not a check: `cargo test --release -p k2-world --test barks survey
/// -- --ignored --nocapture`. Per module: creature and placeable dialogs
/// whose starts have no reply links (barks) or do (conversations), and
/// static (NotReorienting) creatures.
#[test]
#[ignore]
fn survey_barks_and_static_creatures() {
    let Some(install) = install() else { return };
    let modules = install.module_names();
    let mut w = World::new(install).expect("the install loads into a world");
    let mut failures = Failures::default();
    let (mut bark_dlgs, mut real_dlgs) = (0, 0);
    for m in &modules {
        if let Err(e) = w.enter_module(m, None) {
            failures.module(m, e);
            continue;
        }
        let mut lines = Vec::new();
        for o in w.iter().filter(|o| matches!(o.kind, ObjectKind::Creature | ObjectKind::Placeable)) {
            let Some(name) = o.conversation.clone().filter(|s| !s.is_empty()) else { continue };
            let Some(dlg) = load_dialog(&w, m, &name, &mut failures) else { continue };
            let bare = dlg.starting.iter().filter(|l| dlg.is_one_liner(l.index)).count();
            let total = dlg.starting.len();
            if total > 0 && bare == total {
                bark_dlgs += 1;
            } else {
                real_dlgs += 1;
            }
            let kind = if o.kind == ObjectKind::Creature { "creature" } else { "placeable" };
            let static_ = o.creature().is_some_and(|c| c.not_reorienting);
            lines.push(format!(
                "  {kind} {} dlg {name}: {bare}/{total} starts are bare lines{}",
                o.tag,
                if static_ { " [NotReorienting]" } else { "" }
            ));
        }
        if !lines.is_empty() {
            println!("{m}");
            lines.iter().for_each(|l| println!("{l}"));
        }
    }
    println!("dialogs: {bark_dlgs} bare-line only, {real_dlgs} with real conversation starts");
    failures.finish();
}

/// Survey, not a check: `cargo test --release -p k2-world --test barks
/// survey_lines -- --ignored --nocapture`. Dialog entries whose text is empty
/// after tokens and director notes are stripped (the panel shows no subtitle
/// for them), split by whether the entry has a voice-over or sound.
#[test]
#[ignore]
fn survey_lines_without_subtitle_text() {
    let Some(install) = install() else { return };
    let modules = install.module_names();
    let mut w = World::new(install).expect("the install loads into a world");
    let mut failures = Failures::default();
    let (mut silent_audio, mut silent_none) = (0, 0);
    for m in &modules {
        if let Err(e) = w.enter_module(m, None) {
            failures.module(m, e);
            continue;
        }
        let mut seen = std::collections::BTreeSet::new();
        let owners: Vec<String> = w
            .iter()
            .filter(|o| matches!(o.kind, ObjectKind::Creature | ObjectKind::Placeable))
            .filter_map(|o| o.conversation.clone().filter(|s| !s.is_empty()))
            .collect();
        for name in owners {
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some(dlg) = load_dialog(&w, m, &name, &mut failures) else { continue };
            let talk = w.talk.as_ref().expect("the talk table loads");
            for (i, node) in dlg.entries.iter().enumerate() {
                let text = w.substitute(&talk.resolve(&node.text));
                if !text.trim().is_empty() {
                    continue;
                }
                let audio = node.vo_resref.as_deref().or(node.sound.as_deref()).filter(|s| !s.is_empty());
                match audio {
                    Some(a) => {
                        silent_audio += 1;
                        println!("{m} {name}[{i}] no subtitle, audio {a}");
                    }
                    None => silent_none += 1,
                }
            }
        }
    }
    println!("entries with no subtitle text: {silent_audio} with audio, {silent_none} without");
    failures.finish();
}

#[test]
fn bark_position_reaches_the_event() {
    let Some(mut w) = world() else { return };
    w.enter_module("501ond", None).unwrap();
    let pc = w.party_leader();
    let animal = w
        .iter()
        .find(|o| o.kind == ObjectKind::Creature && o.conversation.as_deref() == Some("tame"))
        .map(|o| o.id)
        .expect("501ond has a tame caged animal");

    // ActionStartConversation's nBarkX / nBarkY ride along into the bark.
    w.drain_events();
    assert!(w.start_conversation_at(animal, pc, "tame", Some((120, 300))));
    let pos: Vec<Option<(i32, i32)>> = w
        .drain_events()
        .iter()
        .filter_map(|e| match e {
            WorldEvent::BarkLine { pos, .. } => Some(*pos),
            _ => None,
        })
        .collect();
    assert_eq!(pos, vec![Some((120, 300))]);

    // Without one, the layout keeps its own place.
    assert!(w.start_conversation(animal, pc, "tame"));
    let pos: Vec<Option<(i32, i32)>> = w
        .drain_events()
        .iter()
        .filter_map(|e| match e {
            WorldEvent::BarkLine { pos, .. } => Some(*pos),
            _ => None,
        })
        .collect();
    assert_eq!(pos, vec![None]);
}

/// Survey, not a check: `cargo test --release -p k2-world --test barks
/// survey_camera_angle_four -- --ignored --nocapture`. Bare entries (no reply
/// links) whose node runs an animated camera (CameraAngle 4).
#[test]
#[ignore]
fn survey_camera_angle_four_bare_lines() {
    let Some(install) = install() else { return };
    let modules = install.module_names();
    let mut w = World::new(install).expect("the install loads into a world");
    let mut failures = Failures::default();
    let mut hits = 0;
    for m in &modules {
        if let Err(e) = w.enter_module(m, None) {
            failures.module(m, e);
            continue;
        }
        let names: std::collections::BTreeSet<String> = w
            .iter()
            .filter(|o| matches!(o.kind, ObjectKind::Creature | ObjectKind::Placeable))
            .filter_map(|o| o.conversation.clone().filter(|s| !s.is_empty()))
            .collect();
        for name in names {
            let Some(dlg) = load_dialog(&w, m, &name, &mut failures) else { continue };
            for (i, node) in dlg.entries.iter().enumerate() {
                if node.links.is_empty() && node.camera_angle == 4 {
                    hits += 1;
                    println!("{m} {name}[{i}] bare, camera angle 4, camera model {:?}, animated cut {}", dlg.camera_model, dlg.animated_cut);
                }
            }
        }
    }
    println!("bare camera-angle-4 entries: {hits}");
    failures.finish();
}
