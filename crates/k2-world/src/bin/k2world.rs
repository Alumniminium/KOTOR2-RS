//! k2world: headless harness for k2-world.
//!
//!   k2world [--install DIR] run MODULE [--seconds N] [--trace] [--manual-dialog]
//!   k2world [--install DIR] all [--seconds N]
//!   k2world [--install DIR] talk MODULE TAG
//!   k2world [--install DIR] list MODULE
//!   k2world [--install DIR] talkall
//!   k2world [--install DIR] fight MODULE [--seconds N] [--settle N]
//!   k2world [--install DIR] inv MODULE [--seconds N]
//!   k2world [--install DIR] store MODULE
//!   k2world [--install DIR] items
//!   k2world [--install DIR] craft MODULE
//!   k2world [--install DIR] saves [MODULE...] [--seconds N]
//!
//! `run` loads a module, fires its load events and ticks at 30 Hz, picking
//! dialog reply 0 automatically. `all` does that for every module and
//! summarizes. `talk` starts a conversation with the object tagged TAG and
//! walks it with reply 0.

#[path = "k2world/progress.rs"]
mod progress;
#[path = "k2world/minigames.rs"]
mod minigames;

use std::collections::BTreeMap;
use std::process::ExitCode;

use k2_res::Install;
use k2_world::{Action, Detail, World, WorldEvent};

const DT: f32 = 1.0 / 30.0;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut install = std::env::var("K2_INSTALL").unwrap_or_else(|_| "..".into());
    if args.first().is_some_and(|a| a == "--install") && args.len() >= 2 {
        install = args[1].clone();
        args.drain(..2);
    }
    let flag = |name: &str| args.iter().any(|a| a == name);
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let seconds: f32 = opt("--seconds").and_then(|s| s.parse().ok()).unwrap_or(30.0);
    let install = match Install::discover(&install) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    match args.first().map(String::as_str) {
        Some("run") => {
            let Some(m) = args.get(1) else { return usage() };
            run_one(&install, m, seconds, flag("--trace"), !flag("--manual-dialog"), true).map_or(ExitCode::FAILURE, |_| ExitCode::SUCCESS)
        }
        Some("all") => run_all(&install, seconds),
        Some("talkall") => talk_all(&install),
        Some("list") => {
            let Some(m) = args.get(1) else { return usage() };
            list(&install, m)
        }
        Some("talk") => {
            let (Some(m), Some(tag)) = (args.get(1), args.get(2)) else { return usage() };
            talk(&install, m, tag)
        }
        Some("fight") => {
            let Some(m) = args.get(1) else { return usage() };
            let settle: f32 = opt("--settle").and_then(|s| s.parse().ok()).unwrap_or(2.0);
            fight(&install, m, if opt("--seconds").is_some() { seconds } else { 120.0 }, settle)
        }
        Some("inv") => {
            let Some(m) = args.get(1) else { return usage() };
            inv(&install, m, if opt("--seconds").is_some() { seconds } else { 5.0 })
        }
        Some("store") => {
            let Some(m) = args.get(1) else { return usage() };
            store(&install, m)
        }
        Some("items") => items_survey(&install),
        Some("crowd") => {
            let Some(m) = args.get(1) else { return usage() };
            crowd(&install, m, if opt("--seconds").is_some() { seconds } else { 60.0 })
        }
        Some("craft") => {
            let Some(m) = args.get(1) else { return usage() };
            craft(&install, m)
        }
        Some("levelup") => {
            let m = args.get(1).filter(|a| !a.starts_with("--")).map_or("101per", String::as_str);
            let class: i32 = opt("--class").and_then(|s| s.parse().ok()).unwrap_or(3);
            let level: i32 = opt("--level").and_then(|s| s.parse().ok()).unwrap_or(5);
            progress::levelup(&install, m, class, level)
        }
        Some("powers") => {
            let Some(m) = args.get(1) else { return usage() };
            progress::powers(&install, m, if opt("--seconds").is_some() { seconds } else { 60.0 })
        }
        Some("forceusers") => progress::force_users(&install),
        Some("minigame") => {
            let Some(m) = args.get(1) else { return usage() };
            if flag("--audit") {
                return minigames::audit(&install, m);
            }
            if flag("--probe") {
                return minigames::swoop_probe(&install, m);
            }
            minigames::minigame(&install, m, if opt("--seconds").is_some() { seconds } else { 120.0 }, flag("--trace"))
        }
        Some("pazaak") => {
            let (Some(m), Some(s)) = (args.get(1), args.get(2)) else { return usage() };
            minigames::pazaak(&install, m, s, opt("--wager").and_then(|v| v.parse().ok()).unwrap_or(100), opt("--param").and_then(|v| v.parse().ok()).unwrap_or(0), flag("--trace"))
        }
        Some("saves") => {
            let mods: Vec<String> = args[1..].iter().take_while(|a| !a.starts_with("--")).cloned().collect();
            save_sweep(&install, &mods, if opt("--seconds").is_some() { seconds } else { 10.0 })
        }
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: k2world [--install DIR] run MODULE [--seconds N] [--trace] [--manual-dialog] | all [--seconds N] | talk MODULE TAG");
    ExitCode::FAILURE
}

#[derive(Default)]
struct Summary {
    module: String,
    objects: usize,
    conversations: Vec<String>,
    transitions: Vec<String>,
    events: usize,
    vm_errors: Vec<String>,
    warnings: Vec<String>,
    unimplemented: BTreeMap<String, usize>,
    /// Encounters that began a spawn cycle, and creatures they spawned.
    encounters: (usize, usize),
    panicked: Option<String>,
}

fn run_one(install: &Install, module: &str, seconds: f32, trace: bool, auto: bool, print: bool) -> Option<Summary> {
    let mut w = match World::new(install.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return None;
        }
    };
    w.trace = trace;
    w.auto_dialog = auto;
    let mut sum = Summary { module: module.to_owned(), ..Default::default() };
    if let Err(e) = w.enter_module(module, None) {
        eprintln!("error: {module}: {e}");
        return None;
    }
    sum.objects = w.iter().count();
    if print {
        let leader = w.party_leader();
        let p = w.get(leader).map(|o| o.position).unwrap_or_default();
        println!("module {module}: {} objects, player at ({:.2}, {:.2}, {:.2})", sum.objects, p.x, p.y, p.z);
    }
    let steps = (seconds / DT) as usize;
    // K2_VISIT_ENCOUNTERS=1: the party leader steps into each encounter in
    // turn, every 0.5 s, so the spawns run (nothing else moves the party).
    let visit = std::env::var_os("K2_VISIT_ENCOUNTERS").is_some();
    let encounters: Vec<k2_script::ObjectId> = w.iter().filter(|o| o.encounter().is_some()).map(|o| o.id).collect();
    for step in 0..=steps {
        if visit && step % 15 == 0 && !encounters.is_empty() {
            if let Some(c) = encounter_centre(&w, encounters[(step / 15) % encounters.len()]) {
                let leader = w.party_leader();
                w.place(leader, c);
            }
        }
        for e in w.drain_events() {
            sum.events += 1;
            match &e {
                WorldEvent::ConversationStarted { owner, dialog } => {
                    sum.conversations.push(dialog.clone());
                    if print {
                        println!("[{:6.2}] conversation {dialog} (owner {})", w.time, w.describe(*owner));
                    }
                }
                WorldEvent::TransitionRequested(t) => {
                    sum.transitions.push(format!("{} @ {:?}", t.module, t.waypoint_tag));
                    if print {
                        println!("[{:6.2}] transition -> {} {:?} movies {:?}", w.time, t.module, t.waypoint_tag, t.movies);
                    }
                }
                WorldEvent::ObjectSpawned(_) | WorldEvent::PlayAnimation { .. } | WorldEvent::SoundObject { .. } | WorldEvent::SoundObjectVolume { .. } | WorldEvent::SoundObjectVariance { .. } => {}
                other => {
                    if print && !trace {
                        println!("[{:6.2}] {other:?}", w.time);
                    }
                }
            }
        }
        if step == steps {
            break;
        }
        w.tick(DT);
    }
    sum.vm_errors = w.vm_errors.clone();
    sum.warnings = w.warnings.clone();
    sum.unimplemented = w.unimplemented.clone();
    sum.encounters = encounter_counts(&w);
    if print {
        let leader = w.party_leader();
        let p = w.get(leader).map(|o| o.position).unwrap_or_default();
        println!("after {seconds}s: player at ({:.2}, {:.2}, {:.2}), party {}, conversation {:?}", p.x, p.y, p.z, w.party.len(), w.conversation.as_ref().map(|c| c.dialog.resref.clone()));
        println!("vm errors: {}", sum.vm_errors.len());
        for e in sum.vm_errors.iter().take(10) {
            println!("  {e}");
        }
        println!("warnings: {}", sum.warnings.len());
        for e in sum.warnings.iter().take(10) {
            println!("  {e}");
        }
        let mut un: Vec<_> = sum.unimplemented.iter().collect();
        un.sort_by(|a, b| b.1.cmp(a.1));
        println!("unimplemented routines: {}", un.len());
        for (n, c) in un.iter().take(20) {
            println!("  {c:6} {n}");
        }
        let undeclared: Vec<_> = w.globals.undeclared.keys().take(10).collect();
        if !undeclared.is_empty() {
            println!("undeclared globals: {undeclared:?}");
        }
    }
    Some(sum)
}

/// Centre of an encounter polygon, on the ground.
fn encounter_centre(w: &World, id: k2_script::ObjectId) -> Option<glam::Vec3> {
    let poly = &w.encounter(id)?.polygon;
    let c = poly.iter().fold(glam::Vec3::ZERO, |s, p| s + glam::Vec3::from(*p)) / poly.len() as f32;
    Some(w.walk.ground(c.x, c.y, c.z + 1.0).map_or(c, |h| glam::Vec3::new(c.x, c.y, h.z)))
}

/// Encounters that began a spawn cycle, and the creatures they spawned.
fn encounter_counts(w: &World) -> (usize, usize) {
    let fired = w.iter().filter(|o| o.encounter().is_some_and(|e| e.spawns_current > 0)).count();
    let made = w.iter().filter(|o| o.creature().is_some_and(|c| c.encounter.is_some())).count();
    (fired, made)
}

fn run_all(install: &Install, seconds: f32) -> ExitCode {
    let modules = install.module_names();
    let mut total_un: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let (mut errs, mut panics, mut convs, mut trans) = (0, 0, 0, 0);
    let (mut enc_fired, mut enc_made) = (0, 0);
    println!("{:<8} {:>5} {:>6} {:>5} {:>5} {:>4} {:>4} {:>7}  first conversation / transition", "module", "objs", "events", "conv", "trans", "vme", "warn", "enc");
    for m in &modules {
        let inst = install.clone();
        let mm = m.clone();
        let r = std::panic::catch_unwind(move || run_one(&inst, &mm, seconds, false, true, false));
        let sum = match r {
            Ok(Some(s)) => s,
            Ok(None) => {
                errs += 1;
                println!("{m:<8} load failed");
                continue;
            }
            Err(p) => {
                panics += 1;
                let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()));
                println!("{m:<8} PANIC {msg:?}");
                Summary { module: m.clone(), panicked: msg, ..Default::default() }
            }
        };
        if sum.panicked.is_some() {
            continue;
        }
        errs += sum.vm_errors.len();
        convs += sum.conversations.len();
        trans += sum.transitions.len();
        enc_fired += sum.encounters.0;
        enc_made += sum.encounters.1;
        for (n, c) in &sum.unimplemented {
            let e = total_un.entry(n.clone()).or_default();
            e.0 += c;
            e.1 += 1;
        }
        println!(
            "{:<8} {:>5} {:>6} {:>5} {:>5} {:>4} {:>4} {:>7}  {} {}",
            sum.module,
            sum.objects,
            sum.events,
            sum.conversations.len(),
            sum.transitions.len(),
            sum.vm_errors.len(),
            sum.warnings.len(),
            format!("{}/{}", sum.encounters.0, sum.encounters.1),
            sum.conversations.first().map(String::as_str).unwrap_or("-"),
            sum.transitions.first().map(String::as_str).unwrap_or("-"),
        );
        for e in sum.vm_errors.iter().take(3) {
            println!("         vm: {e}");
        }
    }
    println!("\nmodules {} panics {panics} vm/load errors {errs} conversations {convs} transitions {trans}", modules.len());
    println!("encounters fired {enc_fired} (creatures spawned {enc_made}; enc = fired/creatures per module)");
    let mut un: Vec<_> = total_un.into_iter().collect();
    un.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(b.1.0.cmp(&a.1.0)));
    println!("unimplemented routines ({}), by modules hit:", un.len());
    for (n, (c, mods)) in un.iter().take(30) {
        println!("  {mods:3} modules {c:7} calls  {n}");
    }
    if panics > 0 || errs > 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// Every object with a conversation in every module: talk, reply 0,
/// up to 60 lines each. Reports VM errors and unimplemented routines.
/// Bare-line barks in `evs` (BarkLine events).
fn count_barks(evs: &[WorldEvent]) -> usize {
    evs.iter().filter(|e| matches!(e, WorldEvent::BarkLine { .. })).count()
}

/// The k_ai_master gate flags of an owner (local booleans 97, 87, 73, 26):
/// 97 sends the first talk down ResetDialogState and a no-op event, 87 to
/// the non-talk branch, 73 to a queued start.
fn gate_flags(w: &World, id: u32) -> String {
    let Some(o) = w.get(id) else { return String::new() };
    [97, 87, 73, 26].iter().map(|i| format!("{i}={}", o.local_bools.get(i).copied().unwrap_or(false))).collect::<Vec<_>>().join(" ")
}

/// Whether `evs` holds a bare line barked by `speaker`.
fn sees_bark(evs: &[WorldEvent], speaker: u32) -> bool {
    evs.iter().any(|e| matches!(e, WorldEvent::BarkLine { speaker: s, .. } if *s == speaker))
}

/// `talkall`: every object with a conversation in every module is used (or
/// talked to), then its conversation is walked. Columns: `conversations` open
/// right after use/talk, without ticks, the retried talks included (the old
/// metric); `queued` open only after up to 3 ticks, a talk a script queued;
/// `stuck` still waiting in the owner's actions after the ticks; `barks` bare
/// lines (not conversations); `credit` conversations that another owner's
/// talk opened (walked, not counted for this owner).
fn talk_all(install: &Install) -> ExitCode {
    let mut total_un: BTreeMap<String, usize> = BTreeMap::new();
    let (mut convs, mut queued, mut stuck, mut barks, mut retry, mut credit) = (0, 0, 0, 0, 0, 0);
    let (mut lines, mut errs, mut warns) = (0, 0, 0);
    for m in install.module_names() {
        let lines_before = lines;
        let Ok(mut w) = World::new(install.clone()) else { return ExitCode::FAILURE };
        if w.enter_module(&m, None).is_err() {
            println!("{m}: load failed");
            continue;
        }
        // Run the module's entry to the end before any owner is talked to: its
        // intro cutscenes (queued ActionStartConversation, DelayCommand) would
        // otherwise be open when an owner's first talk comes, and that talk
        // would fail. Quiet means no conversation, no queued talk and a settled
        // party, held for a second; the tick count is capped.
        w.auto_dialog = true;
        let mut quiet = 0;
        for _ in 0..1500 {
            w.tick(DT);
            let talk_queued = w.iter().any(|o| o.actions.iter().any(|a| matches!(a, Action::StartConversation { .. })));
            if w.conversation.is_none() && !talk_queued && w.settled() {
                quiet += 1;
                if quiet >= 30 {
                    break;
                }
            } else {
                quiet = 0;
            }
        }
        w.auto_dialog = false;
        w.end_conversation(true);
        w.drain_events();
        let owners: Vec<_> = w.iter().filter(|o| o.conversation.is_some()).map(|o| o.id).collect();
        let pc = w.party_leader();
        let (mut mconv, mut mqueued, mut mstuck, mut mbarks, mut mretry, mut mcredit) = (0, 0, 0, 0, 0, 0);
        for id in owners {
            if !w.is_valid(id) {
                continue;
            }
            w.pending_transition = None;
            let flags_before = gate_flags(&w, id);
            // use_object is the whole talk: a creature is talked to (talk_to),
            // a placeable starts its own conversation.
            w.use_object(pc, id);
            let evs = w.drain_events();
            mbarks += count_barks(&evs);
            let mut barked = sees_bark(&evs, id);
            // No conversation was running before this talk, so one that is open
            // now is this talk's.
            let immediate = w.conversation.is_some();
            // Only a talk a script queued (StartConversation in the owner's
            // actions) needs ticks; it runs when it reaches the front of the
            // queue (k_ai_master's ClearAllActions puts it there). Ticking for
            // anyone else would let the world move on under the later owners.
            let queued_here = |w: &World| w.get(id).is_some_and(|o| o.actions.iter().any(|a| matches!(a, Action::StartConversation { .. })));
            if !immediate && queued_here(&w) {
                for _ in 0..3 {
                    if w.conversation.is_some() {
                        break;
                    }
                    w.tick(DT);
                    let evs = w.drain_events();
                    mbarks += count_barks(&evs);
                    barked |= sees_bark(&evs, id);
                }
            }
            // A talk that failed on its first try gets one more. The first fire may
            // change state the retry then sees; the gate flags on the retry line show
            // whether it did. Retries are counted apart.
            let mut retried = false;
            // Why the first try did not talk, taken before the retry changes anything.
            let mut first = String::new();
            if w.conversation.is_none() && !queued_here(&w) && !barked {
                let (kind, tag, dialog, script, state) = w.get(id).map_or((String::new(), String::new(), String::new(), false, String::new()), |o| {
                    let state = match &o.detail {
                        Detail::Door(d) => format!(" open={} locked={}", d.open, d.locked),
                        Detail::Placeable(p) => format!(" open={} locked={}", p.open, p.locked),
                        _ => String::new(),
                    };
                    (format!("{:?}", o.kind), o.tag.clone(), o.conversation.clone().unwrap_or_default(), o.script("ScriptDialogue").is_some(), state)
                });
                let why = if dialog.is_empty() { "no conversation field".to_owned() } else { w.explain_start(id, pc, &dialog) };
                first = format!("{m} first {id} {tag} {kind}{state} dialog {dialog} script {script}: {why}");
                w.talk_to(pc, id);
                let evs = w.drain_events();
                mbarks += count_barks(&evs);
                barked |= sees_bark(&evs, id);
                retried = w.conversation.is_some();
                if retried {
                    mretry += 1;
                    println!("{first} -> retry talks; flags before [{flags_before}], after the retry [{}]", gate_flags(&w, id));
                }
            }
            if w.conversation.is_none() {
                if queued_here(&w) {
                    mstuck += 1;
                } else if !barked {
                    println!("{first} -> lost");
                }
                continue;
            }
            // A conversation another object owns (a module intro, a door's line) is
            // walked but not counted for this owner: it is credited apart, and said.
            if let Some(owner) = w.conversation.as_ref().map(|c| c.owner).filter(|&o| o != id) {
                let (owner_tag, dialog) = w.get(owner).map(|o| (o.tag.clone(), w.conversation.as_ref().map(|c| c.dialog.resref.clone()).unwrap_or_default())).unwrap_or_default();
                println!("{m} credit {id} {}: conversation owned by {owner_tag} dialog {dialog}", w.get(id).map(|o| o.tag.clone()).unwrap_or_default());
                mcredit += 1;
            } else if immediate || retried {
                mconv += 1;
            } else {
                mqueued += 1;
            }
            let mut n = 0;
            while w.conversation.is_some() && n < 60 {
                n += 1;
                w.tick(DT);
                if w.conversation.as_ref().is_some_and(|c| c.replies.is_empty()) {
                    w.end_conversation(false);
                } else {
                    w.choose_reply(0);
                }
            }
            w.end_conversation(true);
            mbarks += count_barks(&w.drain_events());
            lines += n;
        }
        println!("{m:<8} conversations {mconv:>4} queued {mqueued:>3} stuck {mstuck:>3} barks {mbarks:>3} credit {mcredit:>2} lines {}", lines - lines_before);
        convs += mconv;
        queued += mqueued;
        retry += mretry;
        credit += mcredit;
        stuck += mstuck;
        barks += mbarks;
        errs += w.vm_errors.len();
        warns += w.warnings.len();
        for e in w.vm_errors.iter().take(3) {
            println!("{m} vm: {e}");
        }
        for e in w.warnings.iter().take(3) {
            println!("{m} warn: {e}");
        }
        for (k, v) in &w.unimplemented {
            *total_un.entry(k.clone()).or_default() += v;
        }
    }
    println!("conversations {convs} (no tick, incl. retried {retry}) queued {queued} stuck {stuck} barks {barks} credit {credit} lines {lines} vm errors {errs} warnings {warns}");
    let mut un: Vec<_> = total_un.into_iter().collect();
    un.sort_by_key(|x| std::cmp::Reverse(x.1));
    println!("unimplemented routines ({}):", un.len());
    for (n, c) in un.iter().take(40) {
        println!("  {c:7} {n}");
    }
    if errs > 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

fn list(install: &Install, module: &str) -> ExitCode {
    let mut w = match World::new(install.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = w.enter_module(module, None) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    for o in w.iter() {
        if matches!(o.kind, k2_world::ObjectKind::Item | k2_world::ObjectKind::Sound) {
            continue;
        }
        let extra = match &o.detail {
            k2_world::Detail::Door(d) => format!("door open={} locked={} -> {:?}/{}", d.open, d.locked, d.linked_to_module, d.linked_to),
            k2_world::Detail::Trigger(t) if t.is_transition => format!("transition -> {:?}/{:?}", t.linked_to_module, t.linked_to),
            k2_world::Detail::Creature(c) => format!("body {:?} head {:?}", c.body_model, c.head_model),
            k2_world::Detail::Placeable(p) => format!("model {:?} useable={}", p.model, p.useable),
            _ => String::new(),
        };
        println!(
            "{:5} {:10} {:24} {:28} dlg={:<14} ({:.1},{:.1},{:.1}) {}",
            o.id,
            format!("{:?}", o.kind),
            o.tag,
            o.name.chars().take(28).collect::<String>(),
            o.conversation.as_deref().unwrap_or("-"),
            o.position.x,
            o.position.y,
            o.position.z,
            extra
        );
    }
    ExitCode::SUCCESS
}

fn talk(install: &Install, module: &str, tag: &str) -> ExitCode {
    let mut w = match World::new(install.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = w.enter_module(module, None) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    // Let OnEnter cutscene conversations finish first.
    w.auto_dialog = true;
    for _ in 0..30 {
        w.tick(DT);
    }
    w.auto_dialog = false;
    w.end_conversation(true);
    let Some(target) = w.by_tag(tag, 0) else {
        eprintln!("no object tagged {tag}");
        return ExitCode::FAILURE;
    };
    let pc = w.party_leader();
    w.talk_to(pc, target);
    if w.conversation.is_none() {
        println!("{} did not start a conversation (conversation file {:?})", w.describe(target), w.get(target).and_then(|o| o.conversation.clone()));
        return ExitCode::FAILURE;
    }
    let mut lines = 0;
    let mut chained = 0;
    loop {
        // Scripts often hand off to a follow-up conversation after a short
        // DelayCommand (console logs start a hologram's dialog); follow up to
        // 5 such hand-offs.
        if w.conversation.is_none() {
            for _ in 0..60 {
                w.tick(DT);
                if w.conversation.is_some() {
                    break;
                }
            }
            if w.conversation.is_none() || chained >= 5 {
                break;
            }
            chained += 1;
            let owner = w.conversation_view().map(|v| v.speaker);
            println!("-- follow-up conversation {chained} (owner {})", owner.map(|o| w.describe(o)).unwrap_or_default());
        }
        let Some(v) = w.conversation_view() else { continue };
        lines += 1;
        if lines > 200 {
            println!("(stopping after 200 lines)");
            break;
        }
        let vo = v.vo.as_deref().or(v.sound.as_deref()).unwrap_or("-");
        let alien = if v.alien_race != 0 { format!(", alien row {} emotion {}", v.alien_race, v.emotion) } else { String::new() };
        println!("[{}] {} ({vo}{alien}): {}", v.entry, w.describe(v.speaker), v.text);
        for (i, r) in v.replies.iter().enumerate() {
            println!("    {i}) {}", if r.text.is_empty() { "[Continue]" } else { &r.text });
        }
        // Let paused/scripted beats run before answering.
        for _ in 0..3 {
            w.tick(DT);
        }
        if w.conversation_view().map(|n| n.entry) != Some(v.entry) {
            continue;
        }
        if v.replies.is_empty() {
            w.end_conversation(false);
        } else {
            w.choose_reply(0);
        }
    }
    println!("conversation ended after {lines} lines; vm errors {}", w.vm_errors.len());
    for e in w.drain_events() {
        if let WorldEvent::TransitionRequested(t) = e {
            println!("transition -> {} {:?}", t.module, t.waypoint_tag);
        }
    }
    ExitCode::SUCCESS
}

/// `fight MODULE`: load, let the module scripts settle, then run the
/// combat harness and print the round log.
fn fight(install: &Install, module: &str, seconds: f32, settle: f32) -> ExitCode {
    let mut w = match World::new(install.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    w.auto_dialog = true;
    if let Err(e) = w.enter_module(module, None) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    let mut t = 0.0;
    while t < settle {
        w.tick(DT);
        let _ = w.drain_events();
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
        t += DT;
    }
    let Some(r) = k2_world::combat::fight(&mut w, seconds, DT) else {
        eprintln!("no creature to fight in {module}");
        return ExitCode::FAILURE;
    };
    for line in &r.log {
        println!("{line}");
    }
    let winner = r.winner.map_or("nobody (timeout)".to_string(), |id| w.describe(id));
    println!("result after {:.1} s: winner {winner}", r.seconds);
    println!("vm errors: {}  unimplemented: {:?}", w.vm_errors.len(), w.unimplemented);
    for e in w.vm_errors.iter().take(5) {
        println!("  vm error: {e}");
    }
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------
// Items, containers, stores
// ---------------------------------------------------------------------------

fn load(install: &Install, module: &str) -> Option<World> {
    let mut w = match World::new(install.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return None;
        }
    };
    w.auto_dialog = true;
    if let Err(e) = w.enter_module(module, None) {
        eprintln!("error: {module}: {e}");
        return None;
    }
    Some(w)
}

fn settle(w: &mut World, seconds: f32) -> Vec<WorldEvent> {
    let mut evs = Vec::new();
    for _ in 0..(seconds / DT) as usize {
        evs.extend(w.drain_events());
        w.tick(DT);
    }
    evs.extend(w.drain_events());
    evs
}

fn print_item(w: &mut World, id: k2_script::ObjectId, indent: &str) {
    if let Some(v) = w.item_view(id) {
        println!(
            "{indent}{:<34} x{:<3} {:<22} {:>6} cr  icon {:<22} tag {}{}",
            v.name,
            v.stack,
            v.base_label,
            v.value,
            v.icon.as_deref().unwrap_or("-"),
            v.tag,
            if v.plot { "  [plot]" } else { "" }
        );
    }
}

fn print_party(w: &mut World) {
    println!("credits {}", w.credits());
    let inv = w.party_inventory();
    println!("party inventory ({} stacks):", inv.len());
    for id in inv {
        print_item(w, id, "  ");
    }
    for member in w.party.clone() {
        println!("equipment of {}:", w.describe(member));
        for (slot, item) in w.equipment(member) {
            let model = w.item_def_of(item).and_then(|d| d.model.clone()).unwrap_or_else(|| "-".into());
            print!("  {:<18} model {:<18} ", k2_world::inventory::SLOT_NAMES.get(slot as usize).copied().unwrap_or("?"), model);
            print_item(w, item, "");
        }
        if let Some(st) = w.combat_stats(member) {
            println!("  combat: bab {} defense {} weapon {}d{}+{} ranged {}", st.bab, st.defense, st.main.num_dice, st.main.die, st.main.damage_bonus, st.main.ranged);
        }
    }
}

fn inv(install: &Install, module: &str, seconds: f32) -> ExitCode {
    let Some(mut w) = load(install, module) else { return ExitCode::FAILURE };
    let evs = settle(&mut w, seconds);
    for e in &evs {
        if let WorldEvent::ItemReceived { name, stack, .. } = e {
            println!("item received: {name} x{stack}");
        }
    }
    print_party(&mut w);

    // Loot the nearest unlocked, non-empty container.
    let leader = w.party_leader();
    let lp = w.get(leader).map(|o| o.position).unwrap_or_default();
    let mut containers: Vec<(f32, k2_script::ObjectId)> = w
        .iter()
        .filter(|o| o.placeable().is_some_and(|p| p.has_inventory))
        .map(|o| (o.position.distance(lp), o.id))
        .collect();
    containers.sort_by(|a, b| a.0.total_cmp(&b.0));
    println!("\n{} containers in the area", containers.len());
    let locked = containers.iter().filter(|(_, c)| w.get(*c).and_then(|o| o.placeable()).is_some_and(|p| p.locked)).count();
    println!("  {locked} locked");
    let pick = containers.iter().map(|&(_, c)| c).find(|&c| {
        !w.get(c).and_then(|o| o.placeable()).is_some_and(|p| p.locked) && !w.inventory_of(c).is_empty()
    });
    if let Some(c) = pick {
        println!("open {} ({})", w.describe(c), w.get(c).map(|o| o.name.clone()).unwrap_or_default());
        match w.open_container(leader, c) {
            Ok(items) => {
                for i in items {
                    print_item(&mut w, i, "  ");
                }
                let got = w.take_all(c, leader);
                w.close_container(c, leader);
                println!("took {} items; container now holds {}", got.len(), w.inventory_of(c).len());
                let evs = settle(&mut w, 1.0);
                for e in &evs {
                    match e {
                        WorldEvent::ItemReceived { name, stack, .. } => println!("  item received: {name} x{stack}"),
                        WorldEvent::ContainerOpened { .. } | WorldEvent::ContainerClosed { .. } | WorldEvent::PlaceableChanged { .. } => println!("  {e:?}"),
                        _ => {}
                    }
                }
                print_party(&mut w);
            }
            Err(e) => println!("  open failed: {e}"),
        }
    }
    if let Some(c) = containers.iter().map(|&(_, c)| c).find(|&c| w.get(c).and_then(|o| o.placeable()).is_some_and(|p| p.locked)) {
        println!("locked container {}: {:?}", w.describe(c), w.open_container(leader, c).err());
    }

    // Equip test: wear whatever fits (armour changes the body model).
    let before = w.get(leader).and_then(|o| o.creature()).and_then(|c| c.body_model.clone());
    for item in w.party_inventory() {
        let Some(def) = w.item_def_of(item) else { continue };
        if let Some(slot) = [1, 0, 3, 7, 8, 9, 10].into_iter().find(|&s| def.fits(s)) {
            let r = w.equip(leader, item, slot);
            println!("equip {} -> {}: {:?}", def.name, k2_world::inventory::SLOT_NAMES[slot as usize], r);
        }
    }
    let after = w.get(leader).and_then(|o| o.creature()).and_then(|c| c.body_model.clone());
    println!("body model {before:?} -> {after:?}");
    // The best melee/ranged weapon from the party inventory.
    for ranged in [false, true] {
        if let Some(item) = w.equip_best_weapon(leader, ranged) {
            println!("equip best {}: {}", if ranged { "ranged" } else { "melee" }, w.get(item).map(|o| o.name.clone()).unwrap_or_default());
        }
    }
    let evs = settle(&mut w, 0.2);
    println!("events: {:?}", evs.iter().filter(|e| matches!(e, WorldEvent::EquipmentChanged { .. } | WorldEvent::AppearanceChanged { .. })).collect::<Vec<_>>());
    println!("vm errors: {}  warnings: {}", w.vm_errors.len(), w.warnings.len());
    for wn in w.warnings.iter().filter(|s| s.contains("Equip") || s.contains("store") || s.contains("item")).take(10) {
        println!("  warning: {wn}");
    }
    if w.vm_errors.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn store(install: &Install, module: &str) -> ExitCode {
    let Some(mut w) = load(install, module) else { return ExitCode::FAILURE };
    settle(&mut w, 1.0);
    let stores: Vec<k2_script::ObjectId> = w.iter().filter(|o| o.kind == k2_world::ObjectKind::Store).map(|o| o.id).collect();
    println!("{} stores", stores.len());
    let pc = w.player;
    w.add_credits(2000);
    let mut chosen = None;
    for &s in &stores {
        w.open_store(s, pc, 0, 0);
        let Some(view) = w.store_view(s) else { continue };
        println!("  {} tag {:?} \"{}\": {} stock entries", w.describe(s), w.get(s).map(|o| o.tag.clone()), view.name, view.stock.len());
        if chosen.is_none() && !view.stock.is_empty() {
            chosen = Some((s, view));
        }
    }
    let Some((s, view)) = chosen else {
        println!("no stocked store (scripts stock some on open): {:?}", w.warnings);
        return ExitCode::SUCCESS;
    };
    w.open_store(s, pc, 0, 0);
    println!("store {} \"{}\" buy {} sell {}  credits {}", w.describe(s), view.name, view.can_buy, view.can_sell, view.credits);
    for (v, price, inf) in &view.stock {
        println!("  {:<34} {:>6} cr{}  ({})", v.name, price, if *inf { " inf" } else { "" }, v.template);
    }
    let Some((cheap, price, _)) = view.stock.iter().min_by_key(|(_, p, _)| *p).cloned() else { return ExitCode::FAILURE };
    let before = w.credits();
    let bought = match w.buy(s, cheap.id) {
        Ok(b) => b,
        Err(e) => {
            println!("buy failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("bought {} for {price}: credits {before} -> {}", cheap.name, w.credits());
    let still = w.store_view(s).map_or(0, |v| v.stock.len());
    println!("store stock now {still} entries");
    match w.sell(s, bought) {
        Ok(p) => println!("sold it back for {p}: credits {}", w.credits()),
        Err(e) => println!("sell failed: {e}"),
    }
    println!("vm errors: {}", w.vm_errors.len());
    ExitCode::SUCCESS
}

/// Resolves every UTI in the install: names, icons, models.
fn items_survey(install: &Install) -> ExitCode {
    let mut w = match World::new(install.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let uti = rakata_core::ResourceType::Uti.type_id();
    let mut templates: std::collections::BTreeSet<String> = w.res.chitin_keys().filter(|k| k.ty == uti).map(|k| k.name.clone()).collect();
    let mut by_module: Vec<(String, String)> = Vec::new();
    for m in install.module_names() {
        if w.res.load_module(&m).is_err() {
            continue;
        }
        for k in w.res.module_keys().filter(|k| k.ty == uti) {
            if templates.insert(k.name.clone()) {
                by_module.push((k.name.clone(), m.clone()));
            }
        }
    }
    let (mut total, mut no_name, mut no_icon, mut weapons, mut no_wmodel) = (0, 0, 0, 0, 0);
    let mut missing_icons: BTreeMap<String, usize> = BTreeMap::new();
    let mut missing_models: BTreeMap<String, usize> = BTreeMap::new();
    let module_of: std::collections::HashMap<String, String> = by_module.into_iter().collect();
    for t in &templates {
        if let Some(m) = module_of.get(t) {
            let _ = w.res.load_module(m);
        }
        let Some(d) = w.item_def(t) else { continue };
        total += 1;
        if d.name.is_empty() {
            no_name += 1;
        }
        if d.icon.is_none() {
            no_icon += 1;
            *missing_icons.entry(format!("{} ({})", d.base_label, d.item_class)).or_default() += 1;
        }
        if d.is_weapon() {
            weapons += 1;
            if d.model.is_none() {
                no_wmodel += 1;
                *missing_models.entry(format!("{}_{:03}", d.item_class, d.model_variation)).or_default() += 1;
            }
        }
    }
    println!("{total} item templates: {no_name} without a name, {no_icon} without an icon; {weapons} weapons, {no_wmodel} without a model");
    for (k, n) in missing_icons.iter().take(20) {
        println!("  no icon: {k} x{n}");
    }
    for (k, n) in missing_models.iter().take(20) {
        println!("  no weapon model: {k} x{n}");
    }
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------
// Crowd: creature-creature collision scenario
// ---------------------------------------------------------------------------

/// Smallest gap (distance minus both personal-space radii) over all pairs of
/// living `ids` on the same floor. Negative = overlap.
fn min_gap(w: &mut World, ids: &[k2_script::ObjectId]) -> (f32, String) {
    let live: Vec<(k2_script::ObjectId, glam::Vec3)> = ids
        .iter()
        .filter_map(|&id| w.get(id).filter(|o| o.creature().is_some_and(|c| !c.dead)).map(|o| (id, o.position)))
        .collect();
    let mut best = (f32::MAX, String::new());
    for i in 0..live.len() {
        for j in i + 1..live.len() {
            let (a, b) = (live[i], live[j]);
            if (a.1.z - b.1.z).abs() > 1.5 {
                continue;
            }
            let gap = (a.1 - b.1).truncate().length() - w.personal_space(a.0) - w.personal_space(b.0);
            if gap < best.0 {
                best = (gap, format!("{} / {}", w.describe(a.0), w.describe(b.0)));
            }
        }
    }
    best
}

fn crowd(install: &Install, module: &str, seconds: f32) -> ExitCode {
    use k2_world::object::{Action, Target};
    let Some(mut w) = load(install, module) else { return ExitCode::FAILURE };
    for _ in 0..(2.0 / DT) as usize {
        w.tick(DT);
        let _ = w.drain_events();
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
    }
    let leader = w.party_leader();
    let others: Vec<(k2_script::ObjectId, glam::Vec3)> = w
        .iter()
        .filter(|o| o.kind == k2_world::object::ObjectKind::Creature && !w.party.contains(&o.id) && o.id != leader)
        .filter(|o| o.creature().is_some_and(|c| !c.dead))
        .map(|o| (o.id, o.position))
        .collect();
    // Densest cluster: the creature with the most others within 15 m.
    let Some(&(_, centre)) = others.iter().max_by_key(|(_, p)| others.iter().filter(|(_, q)| q.distance(*p) < 15.0).count()) else {
        eprintln!("no creatures in {module}");
        return ExitCode::FAILURE;
    };
    let mut near: Vec<(f32, k2_script::ObjectId)> = others.iter().map(|&(id, p)| (p.distance(centre), id)).filter(|(d, _)| *d < 25.0).collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    let crowd: Vec<k2_script::ObjectId> = near.iter().take(6).map(|x| x.1).collect();
    if crowd.len() < 2 {
        eprintln!("fewer than 2 creatures near each other in {module}");
        return ExitCode::FAILURE;
    }
    // The player stands 6 m off the cluster centre, on ground.
    let spot = (1..=8)
        .map(|k| centre + glam::Vec3::new((k as f32 * 0.785).cos(), (k as f32 * 0.785).sin(), 0.0) * 6.0)
        .find(|p| w.walk.ground(p.x, p.y, p.z + 1.0).is_some_and(|h| (h.z - centre.z).abs() < 1.5))
        .unwrap_or(centre);
    w.place(leader, spot);
    if let Some(c) = w.get_mut(leader).and_then(|o| o.creature_mut()) {
        c.min1hp = true;
    }
    let mut ids = crowd.clone();
    ids.push(leader);
    println!("crowd in {module}: {} creatures + {}", crowd.len(), w.describe(leader));
    for &id in &crowd {
        println!("  {} space {:.2}", w.describe(id), w.personal_space(id));
    }
    let mut worst = f32::MAX;
    let report = |w: &mut World, phase: &str, t: f32, worst: &mut f32| {
        let (g, who) = min_gap(w, &ids);
        *worst = worst.min(g);
        println!("  {phase} t={t:>4.1}s min gap {g:+.3} m ({who})");
    };

    // Phase A: everyone converges on the player.
    for &id in &crowd {
        w.push_action(id, Action::MoveTo { target: Target::Object(leader), run: true, range: 0.5, timeout: Some(10.0), elapsed: 0.0, stuck: 0.0 });
    }
    let mut t = 0.0;
    while t < 10.0 {
        w.tick(DT);
        let _ = w.drain_events();
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
        t += DT;
        if (t / 1.0).fract() < DT {
            report(&mut w, "converge", t, &mut worst);
        }
    }

    // Phase B: they turn hostile and fight the player.
    for &id in &crowd {
        if let Some(c) = w.get_mut(id).and_then(|o| o.creature_mut()) {
            c.faction = 1;
        }
        w.push_action(id, Action::Attack(leader));
    }
    if let Some(&first) = crowd.first() {
        w.player_attack(first);
    }
    let mut t = 0.0;
    while t < seconds {
        let target = crowd.iter().copied().find(|&id| w.get(id).and_then(|o| o.creature()).is_some_and(|c| !c.dead));
        let Some(target) = target else { break };
        let attacking = w.get(leader).is_some_and(|o| o.actions.iter().any(|a| matches!(a, Action::Attack(_))));
        if !attacking {
            w.player_attack(target);
        }
        w.tick(DT);
        let _ = w.drain_events();
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
        t += DT;
        if (t / 1.0).fract() < DT {
            report(&mut w, "fight", t, &mut worst);
            if (t - 4.0).abs() < 0.5 {
                // Where the attackers stand around the player (ring slots).
                let lp = w.get(leader).map(|o| o.position).unwrap_or_default();
                let mut spots: Vec<String> = Vec::new();
                for &id in &crowd {
                    if let Some(o) = w.get(id).filter(|o| o.creature().is_some_and(|c| !c.dead)) {
                        let d = (o.position - lp).truncate();
                        spots.push(format!("{:.1}m@{:.0}deg", d.length(), d.y.atan2(d.x).to_degrees()));
                    }
                }
                println!("    attackers around player: {}", spots.join(" "));
            }
        }
    }
    let dead = crowd.iter().filter(|&&id| w.get(id).and_then(|o| o.creature()).is_none_or(|c| c.dead)).count();
    println!("fight: {dead}/{} dead after {t:.1} s", crowd.len());
    println!("worst min gap {worst:+.3} m  vm errors {}  unimplemented {:?}", w.vm_errors.len(), w.unimplemented);
    ExitCode::SUCCESS
}

/// Drives the current conversation, picking the first reply whose text
/// contains `want` (else reply 0), until it ends. Returns the events.
fn drive_dialog(w: &mut World, want: &str) -> Vec<WorldEvent> {
    let mut evs = Vec::new();
    for _ in 0..200 {
        evs.extend(w.drain_events());
        let Some(v) = w.conversation_view() else { break };
        for _ in 0..3 {
            w.tick(DT);
        }
        evs.extend(w.drain_events());
        if w.conversation_view().map(|n| n.entry) != Some(v.entry) {
            continue;
        }
        if v.replies.is_empty() {
            w.end_conversation(false);
            continue;
        }
        let pick = v.replies.iter().position(|r| r.text.to_ascii_lowercase().contains(want)).unwrap_or(0);
        println!("    dialog: \"{}\" -> \"{}\"", v.text.chars().take(60).collect::<String>(), v.replies[pick].text);
        w.choose_reply(pick);
    }
    for _ in 0..10 {
        w.tick(DT);
    }
    evs.extend(w.drain_events());
    evs
}

fn print_screen_events(evs: &[WorldEvent]) -> usize {
    let mut n = 0;
    for e in evs {
        match e {
            WorldEvent::UpgradeScreen { .. } | WorldEvent::ItemCreateScreen { .. } | WorldEvent::SkillCheck { .. } => {
                println!("    event: {e:?}");
                n += 1;
            }
            _ => {}
        }
    }
    n
}

/// `craft MODULE`: uses the module's workbench and lab station through
/// their own scripts, then creates, breaks down and upgrades items, and
/// picks a lock with Security.
fn craft(install: &Install, module: &str) -> ExitCode {
    use k2_world::craft::{Currency, Station};
    let Some(mut w) = load(install, module) else { return ExitCode::FAILURE };
    settle(&mut w, 2.0);
    w.auto_dialog = false;
    if w.conversation.is_some() {
        w.end_conversation(true);
    }
    w.drain_events();
    let pc = w.party_leader();
    println!("leader {}  skills {:?}", w.describe(pc), (0..8).map(|s| w.skill_rank(pc, s)).collect::<Vec<_>>());
    w.give_new_item(Currency::Components.template(), pc, 60, true);
    w.give_new_item(Currency::Chemicals.template(), pc, 30, true);
    for t in ["w_blaste_03", "w_blaste_03", "u_r_targ_02", "u_r_powe_01", "g_i_medeqpmnt01", "g_w_lghtsbr01", "u_l_colo_02"] {
        w.give_new_item(t, pc, 1, true);
    }
    let mut ok = true;
    let find = |w: &World, kind: &str| {
        w.iter()
            .find(|o| o.kind == k2_world::object::ObjectKind::Placeable && o.scripts.contains_key("OnUsed") && o.conversation.as_deref().is_some_and(|c| c.contains(kind)))
            .map(|o| o.id)
    };

    // Workbench, create/breakdown branch, then the upgrade branch.
    let bench = find(&w, "workbnch").or_else(|| find(&w, "workbench"));
    match bench {
        Some(b) => {
            println!("workbench {}", w.describe(b));
            w.use_object(pc, b);
            let evs = drive_dialog(&mut w, "create or breakdown");
            let mut screens = print_screen_events(&evs);
            // Tutorial benches (001ebo) only offer creation.
            w.use_object(pc, b);
            let evs = drive_dialog(&mut w, "upgrade items");
            screens += print_screen_events(&evs);
            ok &= screens > 0;
        }
        None => println!("no workbench placeable in {module}"),
    }
    let screen = w.craft.screen.clone();
    let character = screen.as_ref().map_or(pc, |s| s.character);
    let recipes = w.recipes("itemcreate", character);
    println!("itemcreate: {} recipes, {} creatable now", recipes.len(), recipes.iter().filter(|r| r.skill_ok && r.affordable).count());
    for r in recipes.iter().take(5) {
        println!("    [{:>3}] {:<34} skill {} rank {:>2} cost {:>3} {:?}  ok={} afford={}", r.row, r.name, r.skill, r.rank_required, r.cost, r.currency, r.skill_ok, r.affordable);
    }
    let before = w.currency_count(Currency::Components);
    match recipes.iter().find(|r| r.skill_ok && r.affordable) {
        Some(r) => match w.create_item_from_recipe("itemcreate", r.row, character) {
            Ok(item) => {
                print!("created: ");
                print_item(&mut w, item, "");
                println!("    components {before} -> {}", w.currency_count(Currency::Components));
            }
            Err(e) => {
                println!("create failed: {e}");
                ok = false;
            }
        },
        None => {
            println!("nothing creatable");
            ok = false;
        }
    }
    let blasters: Vec<_> = w.party_inventory().into_iter().filter(|&i| w.get(i).is_some_and(|o| o.template == "w_blaste_03")).collect();
    let units = |w: &World, i| w.get(i).and_then(|o| o.item()).map_or(0, |x| x.stack_size);
    println!("blasters in party inventory: {} objects, {} units", blasters.len(), blasters.iter().map(|&b| units(&w, b)).sum::<i32>());
    if let Some(&spare) = blasters.first() {
        let before = w.currency_count(Currency::Components);
        match w.break_down(spare, Station::Workbench, character) {
            Ok((c, n)) => println!("broke down w_blaste_03: +{n} {c:?} (components {before} -> {})", w.currency_count(Currency::Components)),
            Err(e) => {
                println!("breakdown failed: {e}");
                ok = false;
            }
        }
    }

    // Upgrade: equip the blaster, install a targeting scope.
    if let Some(&gun) = blasters.first() {
        let eq = w.equip(pc, gun, k2_world::stats::SLOT_RIGHTWEAPON);
        let gun = w.equipment(pc).into_iter().find(|&(s, _)| s == k2_world::stats::SLOT_RIGHTWEAPON).map_or(gun, |(_, i)| i);
        println!("equip blaster: {eq:?}  main weapon base {:?}", w.combat_stats(pc).map(|s| (s.main.base_item, s.main.attack_bonus)));
        let ab = |w: &mut World| w.combat_stats(pc).map(|s| s.attack_bonus(false)).unwrap_or(0);
        let before = ab(&mut w);
        let slots = w.upgrade_slots(gun);
        for s in &slots {
            println!("    slot {} {:<15} installed {:?} candidates {:?}", s.slot, s.label, s.installed, s.candidates.iter().map(|&c| w.get(c).map(|o| o.template.clone()).unwrap_or_default()).collect::<Vec<_>>());
        }
        if let Some(s) = slots.iter().find(|s| !s.candidates.is_empty()) {
            let up = s.candidates.iter().copied().find(|&c| w.get(c).is_some_and(|o| o.template == "u_r_targ_02")).unwrap_or(s.candidates[0]);
            match w.apply_upgrade(gun, s.slot, up) {
                Ok(gun) => {
                    let props = w.upgrade_slots(gun).into_iter().filter_map(|s| s.installed).collect::<Vec<_>>();
                    println!("upgraded blaster: installed {props:?}; attack {before} -> {}", ab(&mut w));
                    match w.remove_upgrade(gun, s.slot) {
                        Ok(back) => println!("    removed again -> {}  attack {}", w.get(back).map(|o| o.template.clone()).unwrap_or_default(), ab(&mut w)),
                        Err(e) => println!("    remove failed: {e}"),
                    }
                    let _ = w.apply_upgrade(gun, s.slot, w.party_inventory().into_iter().find(|&i| w.get(i).is_some_and(|o| o.template == "u_r_targ_02")).unwrap_or(up));
                }
                Err(e) => {
                    println!("upgrade failed: {e}");
                    ok = false;
                }
            }
        } else {
            println!("no fitting upgrade for the blaster");
            ok = false;
        }
    }
    if let Some(saber) = w.party_inventory().into_iter().find(|&i| w.get(i).is_some_and(|o| o.template == "g_w_lghtsbr01")) {
        let slots = w.upgrade_slots(saber);
        println!("lightsaber slots: {:?}", slots.iter().map(|s| (s.label.clone(), s.candidates.len())).collect::<Vec<_>>());
        if let Some(s) = slots.iter().find(|s| s.ty == k2_world::craft::COLOR_CRYSTAL && !s.candidates.is_empty()) {
            let r = w.apply_upgrade(saber, s.slot, s.candidates[0]);
            println!("    colour crystal: {r:?} model now {:?}", w.item_model_override(saber));
        }
    }

    // Lab station.
    let lab = find(&w, "medstation");
    match lab {
        Some(l) => {
            println!("lab station {}", w.describe(l));
            w.use_object(pc, l);
            let evs = drive_dialog(&mut w, "use the lab station");
            ok &= print_screen_events(&evs) > 0;
            let chem = w.recipes("chemicalcreate", pc);
            println!("chemicalcreate: {} recipes, {} creatable now", chem.len(), chem.iter().filter(|r| r.skill_ok && r.affordable).count());
            if let Some(r) = chem.iter().find(|r| r.skill_ok && r.affordable) {
                let before = w.currency_count(Currency::Chemicals);
                match w.create_item_from_recipe("chemicalcreate", r.row, pc) {
                    Ok(item) => {
                        print!("created: ");
                        print_item(&mut w, item, "");
                        println!("    chemicals {before} -> {}", w.currency_count(Currency::Chemicals));
                    }
                    Err(e) => {
                        println!("create failed: {e}");
                        ok = false;
                    }
                }
            }
            if let Some(med) = w.party_inventory().into_iter().find(|&i| w.get(i).is_some_and(|o| o.template == "g_i_medeqpmnt01")) {
                println!("lab breakdown medpac: {:?}  (workbench refuses: {:?})", w.break_down(med, Station::LabStation, pc), w.breakdown_preview(med, Station::Workbench, pc).err());
            }
        }
        None => println!("no lab station in {module}"),
    }

    // Security on the first locked door that needs no key.
    let locked: Vec<_> = w
        .iter()
        .filter(|o| o.door().is_some_and(|d| d.locked && !d.key_required))
        .map(|o| o.id)
        .collect();
    let mut picked = false;
    for d in locked {
        let Some(info) = w.lock_info(pc, d) else { continue };
        if info.dc <= 0 {
            continue;
        }
        println!("lock {}: {info:?}", w.describe(d));
        let first = w.try_unlock(pc, d);
        println!("    try_unlock (rank {}): {first:?}", info.security_rank);
        if first.is_err() {
            w.set_skill_rank(pc, k2_world::craft::SKILL_SECURITY, info.dc);
            println!("    try_unlock (rank {}): {:?}", info.dc, w.try_unlock(pc, d));
        }
        let evs = settle(&mut w, 0.5);
        print_screen_events(&evs);
        let open = w.get(d).and_then(|o| o.door()).map(|x| (x.locked, x.open));
        println!("    door now locked/open {open:?}");
        picked = open == Some((false, true));
        break;
    }
    ok &= picked;
    println!("vm errors {}  unimplemented {:?}", w.vm_errors.len(), w.unimplemented);
    println!("{}", if ok { "craft: OK" } else { "craft: FAILED" });
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// `saves`: for each module, run it, save, load into a fresh world, run both
/// 5 s more with reply 0 and compare state digests every second.
fn save_sweep(install: &Install, modules: &[String], seconds: f32) -> ExitCode {
    let modules: Vec<String> = if modules.is_empty() { install.module_names() } else { modules.to_vec() };
    let mut bad = 0;
    let mut skipped = 0;
    let mut total_bytes = 0usize;
    for m in &modules {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(usize, f64), String> {
            let mut a = World::new(install.clone()).map_err(|e| e.to_string())?;
            a.auto_dialog = true;
            a.enter_module(m, None).map_err(|e| e.to_string())?;
            step(&mut a, seconds);
            // Save only where the game allows it (no conversation,
            // pending transition): conversations are not part of a save.
            let mut waited = 0;
            while let Some(why) = a.save_blocked() {
                if waited == 60 {
                    return Err(format!("SKIP save blocked for 60 s: {why}"));
                }
                step(&mut a, 1.0);
                waited += 1;
            }
            let t0 = std::time::Instant::now();
            let bytes = a.save().to_bytes();
            let data = k2_world::save::SaveData::from_bytes(&bytes).map_err(|e| e.to_string())?;
            let mut b = World::load(install.clone(), &data).map_err(|e| e.to_string())?;
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            b.auto_dialog = true;
            let _ = b.drain_events();
            if a.module.as_ref().map(|x| x.name.clone()) != b.module.as_ref().map(|x| x.name.clone()) {
                return Err("module differs".into());
            }
            let frame_by_frame = std::env::var_os("K2_SAVE_FRAMES").is_some();
            let (steps, dt) = if frame_by_frame { (150, DT) } else { (5, 1.0) };
            for s in 0..=steps {
                if s > 0 {
                    let tr = std::env::var("K2_SAVE_TRACE").ok().and_then(|v| v.parse::<usize>().ok());
                    let on = tr.is_some_and(|t| s >= t);
                    a.trace = on;
                    b.trace = on;
                    if on {
                        println!("=== A step {s}");
                    }
                    step(&mut a, dt);
                    if on {
                        println!("=== B step {s}");
                    }
                    step(&mut b, dt);
                }
                if a.state_digest() != b.state_digest() {
                    let mut diff = Vec::new();
                    for o in a.objects.values() {
                        let p = b.objects.get(&o.id);
                        let same = p.is_some_and(|p| {
                            p.position == o.position
                                && p.bearing == o.bearing
                                && p.destroyed == o.destroyed
                                && p.actions.len() == o.actions.len()
                                && p.local_numbers == o.local_numbers
                                && p.local_bools == o.local_bools
                                && p.creature().map(|c| (c.hp, c.dead, c.faction)) == o.creature().map(|c| (c.hp, c.dead, c.faction))
                        });
                        if !same && let Some(p) = p {
                            if p.local_numbers != o.local_numbers {
                                diff.push(format!("{} locals {:?} vs {:?}", a.describe(o.id), sorted(&o.local_numbers), sorted(&p.local_numbers)));
                            }
                            if p.local_bools != o.local_bools {
                                diff.push(format!("{} bools {:?} vs {:?}", a.describe(o.id), sorted(&o.local_bools), sorted(&p.local_bools)));
                            }
                        }
                        if !same {
                            diff.push(format!(
                                "{} a:{:?}/{} b:{:?}/{}",
                                a.describe(o.id),
                                o.position,
                                o.actions.len(),
                                p.map(|p| p.position),
                                p.map_or(0, |p| p.actions.len())
                            ));
                        }
                    }
                    if a.time != b.time {
                        diff.push(format!("time {} vs {}", a.time, b.time));
                    }
                    if a.globals.numbers != b.globals.numbers || a.globals.booleans != b.globals.booleans {
                        diff.push("globals differ".into());
                    }
                    if a.objects.len() != b.objects.len() {
                        diff.push(format!("object count {} vs {}", a.objects.len(), b.objects.len()));
                    }
                    return Err(format!("diverged after step {s} x {dt} s: {}", diff.into_iter().take(8).collect::<Vec<_>>().join("\n  ")));
                }
                // A transition mid-run ends the comparison (both took it).
                if a.pending_transition.is_some() || b.pending_transition.is_some() {
                    break;
                }
            }
            if !b.vm_errors.is_empty() {
                return Err(format!("vm errors after load: {:?}", b.vm_errors));
            }
            Ok((bytes.len(), ms))
        }));
        match r {
            Ok(Ok((n, ms))) => {
                total_bytes += n;
                println!("{m:<8} ok   {:>7} bytes  save+load {ms:.0} ms", n);
            }
            Ok(Err(e)) if e.starts_with("SKIP") => {
                skipped += 1;
                println!("{m:<8} {e}");
            }
            Ok(Err(e)) => {
                bad += 1;
                println!("{m:<8} FAIL {e}");
            }
            Err(_) => {
                bad += 1;
                println!("{m:<8} PANIC");
            }
        }
    }
    let saved = modules.len() - bad - skipped;
    println!("\nmodules {} saved {saved} failures {bad} skipped {skipped} avg size {} bytes", modules.len(), total_bytes / saved.max(1));
    if bad == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// Ticks at 30 Hz, walking conversations with reply 0 (as `run` does).
fn step(w: &mut World, seconds: f32) {
    let mut t = 0.0;
    while t < seconds {
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

fn sorted<V: Clone + Ord>(m: &std::collections::HashMap<i32, V>) -> Vec<(i32, V)> {
    let mut v: Vec<(i32, V)> = m.iter().map(|(k, v)| (*k, v.clone())).collect();
    v.sort();
    v
}
