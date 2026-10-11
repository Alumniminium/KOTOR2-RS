//! Control channel and `--continue`: lets a shell make the running game
//! save and restart it exactly where the player left off.
//!
//! Channel: a directory (`K2_CTL_DIR`, default
//! `$XDG_DATA_HOME/kotor2-rs/ctl`; disabled for `--screenshot` runs unless
//! `K2_CTL_DIR` is set, so test runs never touch the player's). The game
//! writes its pid to `pid` at startup and removes it on exit, polls `cmd`
//! about every 0.2 s (window focus does not matter) and consumes it: one
//! ASCII command per line, appended by the caller.
//!
//!   save        full save into the slot "restart" (only that slot is ever
//!               written); waits up to 10 s for saving to be allowed
//!   quit        save as above (when in game), then exit cleanly
//!   quit now    exit without saving
//!   load SLOT   load a slot (only while in game)
//!
//! Every command is answered with one line in `ack` (written atomically,
//! last command wins): `ok save slot=restart ts=UNIX digest=0x..`,
//! `err save not in game ts=UNIX`, ... Commands that need a running game
//! answer "not in game" in menus, movies or while an area loads past the
//! wait.
//!
//! `--continue` skips logos, movies and the main menu and loads the newest
//! readable slot (by save.bin mtime) among restart, timed, autosave,
//! quicksave and save-NNNN, restart winning a tie; `--load-slot NAME` loads that slot. The
//! loaded state digest is compared with the one stored beside the slot
//! (`digest.txt`) and logged: informational only, since a new build with new
//! state fields hashes differently.
//!
//! On every normal exit while in game (window close, Esc menu quit, signals
//! SIGTERM/SIGINT/SIGHUP, AppExit) the game saves the slot "restart" and
//! logs "ctl: saved restart on exit". `--screenshot` and `--edit` runs and
//! `quit now` do not. See docs/notes/saves.md.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::window::ExitSystems;
use k2_world::save;

use crate::Args;
use crate::game::{AppState, NeedAreaLoad, WorldRes};
use crate::saves::{PendingLoad, SaveJob, SaveRoot, SaveTest, save_now};

pub const SWITCHES: [&str; 1] = ["--continue"];
pub const FLAGS: [&str; 1] = ["--load-slot"];

/// The only slot the control channel writes.
pub const RESTART_SLOT: &str = "restart";

const POLL_SECS: f32 = 0.2;
/// How long `save` waits for saving to be allowed (combat, scripts, ...).
const SAVE_WAIT_SECS: f32 = 10.0;
/// How long the thumbnail may take before the ack goes out anyway.
const THUMB_WAIT_SECS: f32 = 3.0;
const LOAD_WAIT_SECS: f32 = 15.0;

pub struct CtlPlugin;

impl Plugin for CtlPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Ctl::new())
            .add_systems(Startup, (ctl_init, continue_start).chain())
            .add_systems(Update, (ctl_poll, signal_exit));
        add_exit_systems(app);
    }
}

/// The exit save and the pid file cleanup, in Last after Bevy's window systems.
/// When the last window closes, `exit_on_all_closed` writes AppExit in Last
/// (ExitSystems); a save ordered before it would never see that exit.
fn add_exit_systems(app: &mut App) {
    app.add_systems(Last, (exit_save, ctl_exit).chain().after(ExitSystems));
}

/// True for `--continue` and `--load-slot`: the run goes straight into a
/// save, so logos and menus are skipped.
pub fn wants_continue() -> bool {
    std::env::args().any(|a| a == "--continue" || a == "--load-slot")
}

fn arg_value(name: &str) -> Option<String> {
    let argv: Vec<String> = std::env::args().collect();
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

enum Job {
    Save { quit: bool, waited: f32, saved: Option<(PathBuf, u64)> },
    Load { slot: String, waited: f32 },
}

#[derive(Resource)]
struct Ctl {
    dir: Option<PathBuf>,
    poll: f32,
    queue: VecDeque<String>,
    job: Option<Job>,
    /// A load this module started; its digest is checked when it lands.
    awaiting: Option<String>,
    /// The exit is already saved (`quit`) or must not save (`quit now`).
    no_exit_save: bool,
    exit_handled: bool,
}

impl Ctl {
    fn new() -> Self {
        let test_run = std::env::args().any(|a| a == "--screenshot");
        let dir = std::env::var_os("K2_CTL_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| {
            if test_run {
                return None;
            }
            save::default_save_root().parent().map(|p| p.join("ctl"))
        });
        Self { dir, poll: 0.0, queue: VecDeque::new(), job: None, awaiting: None, no_exit_save: false, exit_handled: false }
    }

    /// Writes the ack line atomically.
    fn ack(&self, line: String) {
        let Some(dir) = &self.dir else { return };
        info!("ctl: ack {line}");
        let tmp = dir.join("ack.tmp");
        if std::fs::write(&tmp, format!("{line}\n")).and_then(|_| std::fs::rename(&tmp, dir.join("ack"))).is_err() {
            warn!("ctl: cannot write ack in {}", dir.display());
        }
    }
}

fn ctl_init(ctl: Res<Ctl>) {
    install_signal_handlers();
    let Some(dir) = &ctl.dir else { return };
    if let Err(e) = std::fs::create_dir_all(dir) {
        warn!("ctl: cannot create {}: {e}", dir.display());
        return;
    }
    // A command left by an earlier run must not act on this one.
    for stale in ["cmd", "cmd.read", "ack"] {
        let _ = std::fs::remove_file(dir.join(stale));
    }
    let tmp = dir.join("pid.tmp");
    if std::fs::write(&tmp, format!("{}\n", std::process::id())).and_then(|_| std::fs::rename(&tmp, dir.join("pid"))).is_err() {
        warn!("ctl: cannot write pid file in {}", dir.display());
    }
    info!("ctl: listening in {} (pid {})", dir.display(), std::process::id());
}

/// The slot `--continue` loads: the newest readable restart / timed / autosave /
/// quicksave / save-NNNN by save.bin mtime (restart wins a tie).
fn continue_slot(root: &Path) -> Option<String> {
    let mtime = |slot: &str| std::fs::metadata(root.join(slot).join("save.bin")).and_then(|m| m.modified()).ok();
    save::list_slots(root)
        .into_iter()
        .filter(|s| s.incompatible.is_none() && (s.slot == RESTART_SLOT || s.slot == crate::saves::TIMED_SLOT || s.slot == "autosave" || s.slot == "quicksave" || s.slot.starts_with("save-")))
        .filter_map(|s| mtime(&s.slot).map(|t| (t, s.slot == RESTART_SLOT, s.slot)))
        .max()
        .map(|(_, _, slot)| slot)
}

/// `--continue` / `--load-slot NAME`: queue the load before the first
/// frame; `apply_load` swaps the world in and moves to the game.
fn continue_start(mut ctl: ResMut<Ctl>, root: Res<SaveRoot>, mut load: ResMut<PendingLoad>) {
    if !wants_continue() {
        return;
    }
    let slot = match arg_value("--load-slot") {
        Some(name) => valid_slot(&name).then_some(name).filter(|n| root.0.join(n).join("save.bin").is_file()),
        None => continue_slot(&root.0),
    };
    match slot {
        Some(slot) => {
            info!("continue: loading slot {slot} from {}", root.0.display());
            load.0 = Some(slot.clone());
            ctl.awaiting = Some(slot);
        }
        None => error!("continue: no loadable slot in {} (staying at the main menu)", root.0.display()),
    }
}

fn valid_slot(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn leader_summary(w: &k2_world::World) -> String {
    let module = w.module.as_ref().map_or("?", |m| m.name.as_str());
    match w.get(w.party_leader()) {
        Some(o) => format!("module {module} leader at ({:.2}, {:.2}, {:.2})", o.position.x, o.position.y, o.position.z),
        None => format!("module {module} no leader"),
    }
}

fn read_digest(dir: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(dir.join("digest.txt")).ok()?;
    u64::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

#[allow(clippy::too_many_arguments)]
fn ctl_poll(
    time: Res<Time<Real>>,
    mut ctl: ResMut<Ctl>,
    state: Res<State<AppState>>,
    world: Res<WorldRes>,
    need: Res<NeedAreaLoad>,
    root: Res<SaveRoot>,
    mut job: ResMut<SaveJob>,
    mut load: ResMut<PendingLoad>,
    mut test: ResMut<SaveTest>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(dir) = ctl.dir.clone() else {
        // --continue without a channel still checks the digest.
        check_loaded(&mut ctl, &root, &world, &mut test);
        return;
    };
    let dt = time.delta_secs();
    let in_game = *state.get() == AppState::Game && world.0.module.is_some() && !need.0;

    check_loaded(&mut ctl, &root, &world, &mut test);

    // Collect new commands.
    ctl.poll += dt;
    if ctl.poll >= POLL_SECS {
        ctl.poll = 0.0;
        let cmd = dir.join("cmd");
        let taken = dir.join("cmd.read");
        if cmd.is_file() && std::fs::rename(&cmd, &taken).is_ok() {
            let text = std::fs::read(&taken).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            let _ = std::fs::remove_file(&taken);
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
                ctl.queue.push_back(line.to_owned());
            }
        }
    }

    // Advance the running job.
    if let Some(running) = ctl.job.take() {
        ctl.job = match running {
            Job::Save { quit, mut waited, saved: None } => {
                waited += dt;
                let name = "Restart";
                let attempt = if in_game && job.dir.is_none() { Some(save_now(&world.0, &root, RESTART_SLOT, name, &mut job)) } else { None };
                match attempt {
                    Some(Ok(sdir)) => {
                        let digest = world.0.state_digest();
                        if let Err(e) = std::fs::write(sdir.join("digest.txt"), format!("{digest:#x}\n")) {
                            warn!("ctl: digest.txt: {e}");
                        }
                        info!("ctl: saved {RESTART_SLOT}: {} digest {digest:#x}", leader_summary(&world.0));
                        Some(Job::Save { quit, waited: 0.0, saved: Some((sdir, digest)) })
                    }
                    _ if waited > SAVE_WAIT_SECS => {
                        let why = match attempt {
                            Some(Err(e)) => e,
                            _ if !in_game => "not in game".to_owned(),
                            _ => "busy".to_owned(),
                        };
                        ctl.ack(format!("err save {why} ts={}", unix_now()));
                        if quit {
                            // The caller asked for the game to go away.
                            exit.write(AppExit::Success);
                        }
                        None
                    }
                    _ => Some(Job::Save { quit, waited, saved: None }),
                }
            }
            Job::Save { quit, mut waited, saved: Some((sdir, digest)) } => {
                waited += dt;
                // The thumbnail is cosmetic; do not hold the ack hostage.
                if job.dir.is_none() || waited > THUMB_WAIT_SECS {
                    ctl.ack(format!("ok save slot={RESTART_SLOT} ts={} digest={digest:#x}", unix_now()));
                    if quit {
                        info!("ctl: quit");
                        ctl.no_exit_save = true;
                        exit.write(AppExit::Success);
                    }
                    None
                } else {
                    Some(Job::Save { quit, waited, saved: Some((sdir, digest)) })
                }
            }
            Job::Load { slot, mut waited } => {
                waited += dt;
                if ctl.awaiting.is_none() {
                    // check_loaded consumed it: the load landed.
                    ctl.ack(format!("ok load slot={slot} ts={}", unix_now()));
                    None
                } else if waited > LOAD_WAIT_SECS {
                    ctl.awaiting = None;
                    ctl.ack(format!("err load {slot} did not complete ts={}", unix_now()));
                    None
                } else {
                    Some(Job::Load { slot, waited })
                }
            }
        };
    }

    // Start the next command once the previous one is done.
    while ctl.job.is_none() {
        let Some(line) = ctl.queue.pop_front() else { break };
        let mut words = line.split_whitespace();
        let cmd = words.next().unwrap_or_default().to_ascii_lowercase();
        let arg = words.next().map(str::to_owned);
        info!("ctl: command {line:?}");
        match cmd.as_str() {
            "save" => {
                if !in_game && *state.get() != AppState::Game {
                    ctl.ack(format!("err save not in game ts={}", unix_now()));
                } else {
                    ctl.job = Some(Job::Save { quit: false, waited: 0.0, saved: None });
                }
            }
            "quit" if arg.as_deref().is_some_and(|a| a.eq_ignore_ascii_case("now")) => {
                ctl.no_exit_save = true;
                ctl.ack(format!("ok quit now ts={}", unix_now()));
                exit.write(AppExit::Success);
            }
            "quit" => {
                if *state.get() == AppState::Game {
                    ctl.job = Some(Job::Save { quit: true, waited: 0.0, saved: None });
                } else {
                    ctl.ack(format!("ok quit not in game, nothing saved ts={}", unix_now()));
                    exit.write(AppExit::Success);
                }
            }
            "load" => match arg {
                Some(slot) if valid_slot(&slot) && root.0.join(&slot).join("save.bin").is_file() => {
                    if *state.get() == AppState::Game {
                        load.0 = Some(slot.clone());
                        ctl.awaiting = Some(slot.clone());
                        ctl.job = Some(Job::Load { slot, waited: 0.0 });
                    } else {
                        ctl.ack(format!("err load not in game ts={}", unix_now()));
                    }
                }
                _ => ctl.ack(format!("err load no such slot ts={}", unix_now())),
            },
            other => ctl.ack(format!("err {other} unknown command ts={}", unix_now())),
        }
    }
}

/// When a load this module started has landed: log module and position and
/// compare the digest with the one stored beside the slot.
fn check_loaded(ctl: &mut Ctl, root: &SaveRoot, world: &WorldRes, test: &mut SaveTest) {
    let Some(slot) = ctl.awaiting.clone() else { return };
    let Some(digest) = test.loaded.take() else { return };
    ctl.awaiting = None;
    let summary = leader_summary(&world.0);
    match read_digest(&root.0.join(&slot)) {
        Some(saved) if saved == digest => info!("continue: PASS slot {slot} loaded digest {digest:#x} matches the saved one; {summary}"),
        Some(saved) => warn!("continue: loaded (digest differs, engine changed) slot {slot} loaded digest {digest:#x}, saved {saved:#x}; {summary}"),
        None => info!("continue: slot {slot} loaded digest {digest:#x} (no saved digest to compare); {summary}"),
    }
}

static SIGNALLED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    SIGNALLED.store(true, Ordering::SeqCst);
}

/// SIGTERM, SIGINT and SIGHUP become a normal exit (with the exit save).
fn install_signal_handlers() {
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: the handler only stores to an atomic, which is
        // async-signal-safe.
        unsafe {
            libc::signal(sig, on_signal as *const () as libc::sighandler_t);
        }
    }
}

fn signal_exit(mut exit: MessageWriter<AppExit>) {
    if SIGNALLED.swap(false, Ordering::SeqCst) {
        info!("ctl: signal received, exiting");
        exit.write(AppExit::Success);
    }
}

/// Saves the slot "restart" for an exit: the same save as the `save`
/// command but written at once and without a thumbnail (the window may be
/// gone). Allowed during a conversation (`World::exit_save_blocked`).
/// Returns the state digest.
fn save_on_exit(world: &k2_world::World, root: &Path) -> Result<u64, String> {
    if let Some(why) = world.exit_save_blocked() {
        return Err(why.to_owned());
    }
    let data = world.save();
    let meta = world.save_meta("Restart");
    let dir = save::write_slot(root, RESTART_SLOT, &data, &meta).map_err(|e| e.to_string())?;
    let digest = world.state_digest();
    let _ = std::fs::write(dir.join("digest.txt"), format!("{digest:#x}\n"));
    // The old thumbnail shows an earlier moment.
    let _ = std::fs::remove_file(dir.join("thumb.png"));
    Ok(digest)
}

/// On every exit while in game: save "restart" so `--continue` resumes
/// exactly here.
fn exit_save(mut exits: MessageReader<AppExit>, mut ctl: ResMut<Ctl>, args: Res<Args>, state: Res<State<AppState>>, world: Res<WorldRes>, root: Res<SaveRoot>) {
    if exits.read().next().is_none() || std::mem::replace(&mut ctl.exit_handled, true) {
        return;
    }
    if ctl.no_exit_save || args.screenshot.is_some() || args.edit || *state.get() != AppState::Game {
        return;
    }
    match save_on_exit(&world.0, &root.0) {
        Ok(digest) => info!("ctl: saved restart on exit: {} digest {digest:#x}", leader_summary(&world.0)),
        Err(why) => warn!("ctl: not saved on exit: {why}"),
    }
}

/// Removes the pid file when the game exits.
fn ctl_exit(mut exits: MessageReader<AppExit>, ctl: Res<Ctl>) {
    if exits.read().next().is_none() {
        return;
    }
    let Some(dir) = &ctl.dir else { return };
    let pid = dir.join("pid");
    if std::fs::read_to_string(&pid).is_ok_and(|s| s.trim() == std::process::id().to_string()) {
        let _ = std::fs::remove_file(&pid);
    }
}

#[cfg(test)]
mod tests;
