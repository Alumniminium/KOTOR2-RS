//! The supervisor: starts the game as a child process, keeps its output, and
//! forwards commands to it over the control socket. It holds no game logic:
//! that lives in the child, so this process can stay up while the game is
//! rebuilt and relaunched.
//!
//! Every launch is a windowed game: WAYLAND_DISPLAY is empty (XWayland, which
//! the NVIDIA present mode needs), K2_TEST_WINDOW=1 gives the test title and
//! class that the Hyprland rule sends to workspace 4, and the game's
//! directories are under the run root, never the player's. The caller's
//! arguments and environment are checked first (policy.rs).
//!
//! One lifecycle guard covers start, stop and restart, so a stop cannot run
//! into a launch. The game is spawned by a launcher thread that lives as long
//! as the process, and it gets SIGTERM if this process dies (PR_SET_PDEATHSIG).

use std::io;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use k2_world::control::socket;
use k2_world::control::{Command as Verb, Reply, SaveArgs};
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};

use crate::game_build::{Build, build_game, kill_build};
use crate::logs::{self, Line, LogStore};
use crate::paths::{Paths, stamp, unix_millis};
use crate::policy;

/// Save slot a restart writes and reloads.
pub const RESTART_SLOT: &str = "mcp-restart";
/// How long a launch may take to open the control socket.
const SOCKET_WAIT: Duration = Duration::from_secs(90);
/// How long a launch waits for the game to report its window settled.
const SETTLE_WAIT: Duration = Duration::from_secs(20);
/// Lines of output in a crash report.
const CRASH_LINES: usize = 40;
/// Time the game gets to exit after the quit request.
const QUIT_WAIT: Duration = Duration::from_secs(20);
const TERM_WAIT: Duration = Duration::from_secs(10);
/// Low bits of a log cursor count lines; the bits above name the run.
const SEQ_BITS: u32 = 40;
const SEQ_MASK: u64 = (1 << SEQ_BITS) - 1;
/// Wall-clock limit of one goal call (move_to, interact, talk_to, transition, wait_until).
const GOAL_CAP: Duration = Duration::from_secs(300);
/// Slack added to each limited call's wait. A call held for speech gets its own wait
/// from the game's interim frame (wait_s), which the socket client adds to the limit.
const SPEECH_SLACK: Duration = Duration::from_secs(5);
/// How long a goal may run while the game's world time stands still before the goal is stopped.
const STALL_LIMIT: Duration = Duration::from_secs(20);
/// How often the watch reads the game's world time.
const WATCH_EVERY: Duration = Duration::from_secs(1);

/// How a game starts.
#[derive(Debug, Clone, PartialEq)]
pub enum Start {
    /// The main menu (no start flag).
    Menu,
    /// A new character, through the prologue (--newgame).
    New,
    /// Character generation (--chargen).
    Chargen,
    /// The newest save (--continue).
    Continue,
    /// A named save slot (--load-slot).
    Load(String),
    /// A module, skipping the menu (--module).
    Module(String),
}

impl Start {
    /// Parses "menu", "new", "chargen", "continue", "load:<slot>" or "module:<name>".
    pub fn parse(text: &str) -> Result<Start, String> {
        let t = text.trim();
        let name = |s: &str| -> Result<String, String> {
            if !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                Ok(s.to_owned())
            } else {
                Err(format!("bad name {s:?}: letters, digits, '-' and '_' only"))
            }
        };
        match t {
            "menu" => Ok(Start::Menu),
            "new" => Ok(Start::New),
            "chargen" => Ok(Start::Chargen),
            "continue" => Ok(Start::Continue),
            _ => {
                if let Some(slot) = t.strip_prefix("load:") {
                    Ok(Start::Load(name(slot)?))
                } else if let Some(module) = t.strip_prefix("module:") {
                    Ok(Start::Module(name(module)?))
                } else {
                    Err(format!("unknown start {t:?}: use menu, new, chargen, continue, load:<slot> or module:<name>"))
                }
            }
        }
    }

    fn args(&self) -> Vec<String> {
        match self {
            Start::Menu => Vec::new(),
            Start::New => vec!["--newgame".into()],
            Start::Chargen => vec!["--chargen".into()],
            Start::Continue => vec!["--continue".into()],
            Start::Load(slot) => vec!["--load-slot".into(), slot.clone()],
            Start::Module(m) => vec!["--module".into(), m.clone()],
        }
    }

    fn describe(&self) -> String {
        match self {
            Start::Menu => "main menu".into(),
            Start::New => "new game".into(),
            Start::Chargen => "character generation".into(),
            Start::Continue => "continue".into(),
            Start::Load(s) => format!("load {s}"),
            Start::Module(m) => format!("module {m}"),
        }
    }
}

/// The caller's extra game arguments and environment for a launch. A restart
/// reuses the last launch's.
#[derive(Debug, Clone, Default)]
pub struct Extra {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// One child game process.
struct Run {
    child: Child,
    run_id: String,
    started: Instant,
    build: Build,
    start: Start,
    socket: PathBuf,
    log_path: PathBuf,
    logs: Arc<LogStore>,
}

/// The output of the latest run, kept after the run ends for logs_tail.
#[derive(Clone)]
struct Output {
    /// The run's number. Its log cursors carry it in the bits above SEQ_BITS.
    epoch: u64,
    run_id: String,
    logs: Arc<LogStore>,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitKind {
    /// game_stop (or shutdown) asked it to end.
    Stopped,
    /// It ended with code 0 and nobody asked: the player quit it, say.
    ExitedOnItsOwn,
    /// It exited with a non-zero code or a signal.
    Crashed,
}

/// How a run ended.
#[derive(Debug, Clone, Serialize)]
pub struct Exit {
    pub run_id: String,
    pub reason: String,
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub kind: ExitKind,
    pub log_path: PathBuf,
    pub at_unix_ms: u128,
    /// The game's last output, for crash reports (not in the status data).
    #[serde(skip)]
    pub last_lines: Vec<Line>,
}

impl Exit {
    fn new(run: &Run, status: Option<ExitStatus>, kind: ExitKind) -> Self {
        let (reason, code, signal) = status.map_or_else(|| ("ended without a status".to_owned(), None, None), describe);
        Self {
            run_id: run.run_id.clone(),
            reason,
            code,
            signal,
            kind,
            log_path: run.log_path.clone(),
            at_unix_ms: unix_millis(),
            last_lines: run.logs.last(CRASH_LINES),
        }
    }

    /// The text of a crash report: how it ended and the last lines of output.
    fn report(&self) -> String {
        let mut text = format!(
            "The game exited unexpectedly ({}, run {}). Full output: {}\nLast {} lines:",
            self.reason,
            self.run_id,
            self.log_path.display(),
            self.last_lines.len()
        );
        for line in &self.last_lines {
            text.push_str(&format!("\n[{}] {}", line.stream, line.text));
        }
        text
    }
}

fn describe(status: ExitStatus) -> (String, Option<i32>, Option<i32>) {
    if let Some(code) = status.code() {
        (format!("exited with code {code}"), Some(code), None)
    } else if let Some(sig) = status.signal() {
        (format!("killed by signal {sig} ({})", signal_name(sig)), None, Some(sig))
    } else {
        ("ended".to_owned(), None, None)
    }
}

fn signal_name(sig: i32) -> &'static str {
    match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        15 => "SIGTERM",
        _ => "signal",
    }
}

#[derive(Default)]
struct Inner {
    paths: Option<Paths>,
    override_bin: Option<PathBuf>,
    run: Option<Run>,
    last_build: Option<Build>,
    /// Output of the latest run; kept after it ends for logs_tail.
    output: Option<Output>,
    /// Runs launched so far; the latest run's number is its log cursor epoch.
    epoch: u64,
    /// Extras of the latest launch; a restart reuses them.
    last_extra: Extra,
    /// How the latest run ended, once it has.
    last_exit: Option<Exit>,
    /// The server is ending: a launch still waiting on its build does not start a game.
    closing: bool,
}

/// The game's supervisor. Cheap to clone; all clones share one game.
#[derive(Clone)]
pub struct Supervisor {
    inner: Arc<Mutex<Inner>>,
    /// Held by start, stop and restart, so a launch and a stop never overlap.
    lifecycle: Arc<Mutex<()>>,
    launcher: Launcher,
    /// The cargo process group of a build in progress, so shutdown can stop it.
    building: Arc<Mutex<Option<u32>>>,
}

impl Supervisor {
    /// The game binary to run instead of a fresh build, when the policy allows
    /// it (tests only; see policy::game_bin_override).
    pub fn new(paths: Paths) -> Self {
        let override_bin = policy::game_bin_override();
        Self {
            inner: Arc::new(Mutex::new(Inner { paths: Some(paths), override_bin, ..Default::default() })),
            lifecycle: Arc::new(Mutex::new(())),
            launcher: Launcher::new(),
            building: Arc::new(Mutex::new(None)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn paths(&self) -> Paths {
        self.lock().paths.clone().expect("paths are set at construction")
    }

    /// The guard for start and restart: refused while another lifecycle call runs.
    fn try_lifecycle(&self) -> Result<MutexGuard<'_, ()>, Reply> {
        match self.lifecycle.try_lock() {
            Ok(guard) => Ok(guard),
            Err(TryLockError::Poisoned(p)) => Ok(p.into_inner()),
            Err(TryLockError::WouldBlock) => Err(Reply::err("another game_start, game_restart or game_stop is in progress; try again when it finishes")),
        }
    }

    /// The guard for stop and shutdown: waits for a launch in progress.
    fn wait_lifecycle(&self) -> MutexGuard<'_, ()> {
        self.lifecycle.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Builds (unless told not to or overridden), then launches and waits for
    /// the control socket.
    pub fn start(&self, start: Start, build: bool, args: Vec<String>, env: Vec<(String, String)>) -> Reply {
        if let Err(e) = policy::check_launch(&args, &env) {
            return Reply::err(e);
        }
        let _life = match self.try_lifecycle() {
            Ok(guard) => guard,
            Err(busy) => return busy,
        };
        {
            let mut g = self.lock();
            reap(&mut g);
            if let Some(run) = &g.run {
                return Reply::err(format!("a game is already running (pid {}); game_stop or game_restart first", run.child.id()));
            }
        }
        match self.build_for(build) {
            Ok(b) => self.launch_with(b, start, Extra { args, env }),
            Err(e) => Reply::err(e),
        }
    }

    /// The build a launch runs: the override, a fresh snapshot, or the last one.
    /// A fresh snapshot never removes the folder of a game that is running.
    fn build_for(&self, build: bool) -> Result<Build, String> {
        let (override_bin, last, in_use) = {
            let mut g = self.lock();
            reap(&mut g);
            (g.override_bin.clone(), g.last_build.clone(), g.run.as_ref().map(|r| r.build.binary.clone()))
        };
        if let Some(bin) = override_bin {
            return Ok(Build { stamp: "override".into(), describe: "K2_MCP_GAME_BIN".into(), binary: bin, warnings: 0, bytes: 0 });
        }
        if build {
            return build_game(&self.paths(), in_use.as_deref(), &self.building);
        }
        last.ok_or_else(|| "no build yet: call game_start with build true".to_owned())
    }

    fn launch_with(&self, build: Build, start: Start, extra: Extra) -> Reply {
        match self.try_launch(build, start, extra) {
            Ok(reply) => reply,
            Err(e) => Reply::err(e),
        }
    }

    fn try_launch(&self, build: Build, start: Start, extra: Extra) -> Result<Reply, String> {
        if self.lock().closing {
            return Err("the server is shutting down; no game was started".to_owned());
        }
        let paths = self.paths();
        paths.create_all().map_err(|e| format!("cannot create the run folders under {}: {e}", paths.root.display()))?;
        ensure_settings(&paths)?;
        let epoch = {
            let mut g = self.lock();
            g.epoch += 1;
            g.epoch
        };
        let run = launch(&paths, &self.launcher, &build, &start, &extra)?;
        let (pid, run_id, log_path) = (run.child.id(), run.run_id.clone(), run.log_path.clone());
        {
            let mut g = self.lock();
            g.output = Some(Output { epoch, run_id: run_id.clone(), logs: run.logs.clone() });
            g.last_build = Some(build.clone());
            g.last_exit = None;
            g.last_extra = extra;
            g.run = Some(run);
        }
        let socket_path = paths.socket();
        let deadline = Instant::now() + SOCKET_WAIT;
        loop {
            {
                let mut g = self.lock();
                reap(&mut g);
                if g.run.is_none() {
                    return Err(early_exit(&g));
                }
            }
            if socket_path.exists() {
                break;
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "the game (pid {pid}) did not open {} within {} s; it is still running. Output: {}",
                    socket_path.display(),
                    SOCKET_WAIT.as_secs(),
                    log_path.display()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let (info, settled) = settled_info(&socket_path, SETTLE_WAIT);
        let mut game = info.unwrap_or(Value::Null);
        let note = if settled {
            String::new()
        } else {
            // The window is still changing size: its size is not reported until it settles.
            if let Some(fields) = game.as_object_mut() {
                fields.remove("window");
            }
            "; the window is not settled yet, so its size is withheld (game_status reports it once settled)".to_owned()
        };
        Ok(Reply::ok_with(
            format!("game started (pid {pid}, build {}, {}){note}", build.stamp, start.describe()),
            json!({
                "pid": pid,
                "run_id": run_id,
                "mode": "windowed",
                "start": start.describe(),
                "settled": settled,
                "build": build,
                "log_file": log_path,
                "socket": socket_path,
                "game": game,
            }),
        ))
    }

    /// Asks the game to quit (its normal exit save runs), then ends the process
    /// if it does not go: SIGTERM, which the game also saves on, then a kill.
    pub fn stop(&self) -> Reply {
        let _life = self.wait_lifecycle();
        self.stop_locked()
    }

    /// [`Supervisor::stop`] for a caller that holds the lifecycle guard.
    fn stop_locked(&self) -> Reply {
        let run = {
            let mut g = self.lock();
            reap(&mut g);
            g.run.take()
        };
        let Some(mut run) = run else {
            let g = self.lock();
            return match &g.last_exit {
                Some(e) if e.kind == ExitKind::Crashed => not_running(&g),
                _ => Reply::ok("no game is running"),
            };
        };
        let pid = run.child.id();
        let _ = socket::request_timeout(&run.socket, &Verb::Quit, Some(Duration::from_secs(5)));
        let status = match wait_for_exit(&mut run.child, QUIT_WAIT) {
            Some(status) => Some(status),
            None => {
                send_signal(pid, libc::SIGTERM);
                match wait_for_exit(&mut run.child, TERM_WAIT) {
                    Some(status) => Some(status),
                    None => {
                        let _ = run.child.kill();
                        run.child.wait().ok()
                    }
                }
            }
        };
        let exit = Exit::new(&run, status, ExitKind::Stopped);
        let reason = exit.reason.clone();
        self.lock().last_exit = Some(exit);
        Reply::ok(format!("game stopped (pid {pid}, {reason})"))
    }

    /// Builds first (a red build leaves the running game alone; the game keeps
    /// playing while it builds), then saves right before the stop so the
    /// progress made during the build is kept, stops, and relaunches from the
    /// save with the last launch's extras. Without a running game it simply
    /// starts one from the newest save.
    pub fn restart(&self, build: bool) -> Reply {
        let _life = match self.try_lifecycle() {
            Ok(guard) => guard,
            Err(busy) => return busy,
        };
        let was_running = {
            let mut g = self.lock();
            reap(&mut g);
            g.run.is_some()
        };
        let built = match self.build_for(build) {
            Ok(b) => b,
            Err(e) if was_running => return Reply::err(format!("restart aborted, the game keeps running: {e}")),
            Err(e) => return Reply::err(e),
        };
        let running = {
            let mut g = self.lock();
            reap(&mut g);
            g.run.is_some()
        };
        let start = if running {
            let saved = self.forward(Verb::Save(SaveArgs { slot: RESTART_SLOT.into(), name: Some("restart".into()) }));
            if !saved.ok {
                return Reply::err(format!("restart aborted: the game did not save ({}); it keeps running", saved.message));
            }
            let stopped = self.stop_locked();
            if !stopped.ok {
                return Reply::err(format!("restart aborted: {}", stopped.message));
            }
            Start::Load(RESTART_SLOT.into())
        } else {
            Start::Continue
        };
        let extra = self.lock().last_extra.clone();
        self.launch_with(built, start, extra)
    }

    /// What is running, how it was built, and what the game says it is doing.
    pub fn status(&self) -> Reply {
        let (run, last_exit, last_build, socket_path) = {
            let mut g = self.lock();
            reap(&mut g);
            let run = g.run.as_ref().map(|r| {
                json!({
                    "pid": r.child.id(),
                    "run_id": r.run_id,
                    "uptime_s": r.started.elapsed().as_secs_f64(),
                    "start": r.start.describe(),
                    "build": r.build,
                    "log_file": r.log_path,
                    "socket": r.socket,
                })
            });
            (run, g.last_exit.clone(), g.last_build.clone(), g.run.as_ref().map(|r| r.socket.clone()))
        };
        let running = run.is_some();
        let game = socket_path.and_then(|s| socket::request_timeout(&s, &Verb::Info, Some(Duration::from_secs(3))).ok()).and_then(|r| r.data);
        let message = match (running, &last_exit) {
            (true, _) => "game running".to_owned(),
            (false, Some(e)) if e.kind == ExitKind::Crashed => format!("no game running; the last one crashed ({})", e.reason),
            (false, Some(e)) if e.kind == ExitKind::ExitedOnItsOwn => format!("no game running; the last one exited on its own ({})", e.reason),
            (false, _) => "no game running".to_owned(),
        };
        Reply::ok_with(
            message,
            json!({
                "running": running,
                "mode": "windowed",
                "run": run,
                "game": game,
                "last_build": last_build,
                "last_exit": last_exit,
            }),
        )
    }

    /// Sends a command to the game and returns its reply. If the game is not
    /// there, says so, with the crash report when it crashed.
    pub fn forward(&self, cmd: Verb) -> Reply {
        self.forward_with(cmd, None)
    }

    /// [`Supervisor::forward`] with a limit on how long to wait, plus SPEECH_SLACK. A
    /// speech hold the game reports is added to the limit by the socket client.
    pub fn forward_with(&self, cmd: Verb, timeout: Option<Duration>) -> Reply {
        let socket_path = {
            let mut g = self.lock();
            reap(&mut g);
            match &g.run {
                Some(r) => r.socket.clone(),
                None => return not_running(&g),
            }
        };
        if timeout.is_none() && takes_game_time(&cmd) {
            return self.forward_goal(&socket_path, &cmd);
        }
        let timeout = timeout.map(|limit| limit + SPEECH_SLACK);
        match socket::request_timeout(&socket_path, &cmd, timeout) {
            Ok(reply) => reply,
            Err(e) => {
                let mut g = self.lock();
                reap(&mut g);
                if g.run.is_none() {
                    return not_running(&g);
                }
                Reply::err(format!("the game did not answer on {}: {e}", socket_path.display()))
            }
        }
    }

    /// Recent game output, filtered and paged (see LogStore::tail). The cursor
    /// belongs to one run: a cursor from an earlier run returns the newest lines
    /// with dropped set.
    pub fn logs_tail(&self, n: usize, filter: Option<&str>, since: Option<u64>) -> Reply {
        let output = self.lock().output.clone();
        let Some(out) = output else {
            return Reply::ok_with("no game output yet", json!({ "run_id": null, "lines": [], "cursor": null, "dropped": false }));
        };
        let re: Option<Regex> = match filter.filter(|f| !f.is_empty()) {
            Some(f) => match Regex::new(f) {
                Ok(r) => Some(r),
                Err(e) => return Reply::err(format!("bad filter regex: {e}")),
            },
            None => None,
        };
        let (from, stale) = match since {
            None => (None, false),
            Some(c) if c >> SEQ_BITS == out.epoch => (Some(c & SEQ_MASK), false),
            Some(_) => (None, true),
        };
        let tail = out.logs.tail(n.clamp(1, 2000), re.as_ref(), from);
        let cursor = (out.epoch << SEQ_BITS) | tail.cursor;
        let count = tail.lines.len();
        let mut data = json!({ "run_id": out.run_id, "lines": tail.lines, "cursor": cursor, "dropped": tail.dropped || stale });
        if stale {
            data["note"] = json!(format!("the cursor was from an earlier run, so these are the newest lines of {}", out.run_id));
        }
        Reply::ok_with(format!("{count} lines"), data)
    }

    /// The folder the game's screenshots go to.
    pub fn shots_folder(&self) -> PathBuf {
        self.paths().shots()
    }

    /// Ends the game when the server ends, so no window outlives its session.
    pub fn shutdown(&self) {
        self.lock().closing = true;
        kill_build(&self.building);
        let _life = self.wait_lifecycle();
        if self.lock().run.is_some() {
            let _ = self.stop_locked();
        }
    }
}

/// Spawns children on one thread that lives as long as the process. Linux ties
/// PR_SET_PDEATHSIG to the thread that forked the child, and a pool thread can
/// exit while the game is still running, which would kill the game.
#[derive(Clone)]
struct Launcher {
    jobs: mpsc::Sender<(Command, mpsc::Sender<io::Result<Child>>)>,
}

impl Launcher {
    fn new() -> Self {
        let (jobs, requests) = mpsc::channel::<(Command, mpsc::Sender<io::Result<Child>>)>();
        std::thread::Builder::new()
            .name("k2-launcher".into())
            .spawn(move || {
                for (mut cmd, reply) in requests {
                    let _ = reply.send(cmd.spawn());
                }
            })
            .expect("start the launcher thread");
        Self { jobs }
    }

    fn spawn(&self, cmd: Command) -> io::Result<Child> {
        let (reply, child) = mpsc::channel();
        self.jobs.send((cmd, reply)).map_err(|_| io::Error::other("the launcher thread has stopped"))?;
        child.recv().map_err(|_| io::Error::other("the launcher thread has stopped"))?
    }
}

/// Notices a run that ended on its own and records how it ended.
fn reap(g: &mut Inner) {
    let ended = match g.run.as_mut() {
        Some(run) => run.child.try_wait().ok().flatten(),
        None => None,
    };
    if let Some(status) = ended {
        if let Some(run) = g.run.take() {
            // The output pumps may still be behind the exit; let them catch up.
            std::thread::sleep(Duration::from_millis(150));
            let kind = if status.code() == Some(0) { ExitKind::ExitedOnItsOwn } else { ExitKind::Crashed };
            g.last_exit = Some(Exit::new(&run, Some(status), kind));
        }
    }
}

/// The reply for a call made while no game runs: the crash report if the last
/// game crashed, a note if it exited on its own, else a plain "not running".
fn not_running(g: &Inner) -> Reply {
    match &g.last_exit {
        Some(e) if e.kind == ExitKind::Crashed => Reply {
            ok: false,
            message: e.report(),
            feedback: Vec::new(),
            data: Some(json!({ "crash": e })),
        },
        Some(e) if e.kind == ExitKind::ExitedOnItsOwn => Reply::err(format!(
            "no game is running: the last one exited on its own ({}, run {}). Call game_start to start one.",
            e.reason, e.run_id
        )),
        _ => Reply::err("no game is running: call game_start first"),
    }
}

/// The message for a launch whose game ended before it opened the socket.
fn early_exit(g: &Inner) -> String {
    match &g.last_exit {
        Some(e) if e.kind == ExitKind::Crashed => e.report(),
        Some(e) => format!("the game exited before it opened the control socket ({}, run {}). Output: {}", e.reason, e.run_id, e.log_path.display()),
        None => "the game is gone".to_owned(),
    }
}

/// Starts the game process under the windowed launch environment. The caller's
/// environment is applied first; the isolation variables come last and win.
fn launch(paths: &Paths, launcher: &Launcher, build: &Build, start: &Start, extra: &Extra) -> Result<Run, String> {
    let run_id = format!("run-{}", stamp(unix_millis()));
    let log_path = paths.logs().join(format!("{run_id}.log"));
    let logs = LogStore::new(Some(&log_path)).map_err(|e| format!("cannot open {}: {e}", log_path.display()))?;
    clear_stale_socket(&paths.socket())?;
    let mut cmd = Command::new(&build.binary);
    cmd.arg("--install")
        .arg(&paths.install)
        .arg("--control")
        .args(start.args())
        .args(&extra.args)
        .envs(extra.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .env("WAYLAND_DISPLAY", "")
        .env("K2_TEST_WINDOW", "1")
        .env("K2_INSTALL", &paths.install)
        .env("K2_SAVE_DIR", paths.saves())
        .env("K2_SETTINGS_DIR", paths.settings())
        .env("K2_CTL_DIR", paths.ctl())
        .env("K2_SHOT_DIR", paths.shots())
        .env("K2_TRACE_DIR", paths.traces())
        .env("K2_MOD_DIR", paths.mods())
        .env("NO_COLOR", "1")
        .current_dir(&paths.install)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: the closure runs in the forked child before exec and calls only
    // prctl, getppid and _exit, which are async-signal-safe. The game gets
    // SIGTERM when this process dies, and exits at once if it already has.
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                libc::_exit(1);
            }
            Ok(())
        });
    }
    let mut child = launcher.spawn(cmd).map_err(|e| format!("cannot start {}: {e}", build.binary.display()))?;
    if let Some(out) = child.stdout.take() {
        logs::pump(out, "stdout", logs.clone());
    }
    if let Some(err) = child.stderr.take() {
        logs::pump(err, "stderr", logs.clone());
    }
    Ok(Run {
        child,
        run_id,
        started: Instant::now(),
        build: build.clone(),
        start: start.clone(),
        socket: paths.socket(),
        log_path,
        logs,
    })
}

/// The game's settings for supervised runs: vsync off, so frames keep coming
/// while the window is on another workspace. An existing file is left alone.
fn ensure_settings(paths: &Paths) -> Result<(), String> {
    let file = paths.settings().join("settings.json");
    if file.exists() {
        return Ok(());
    }
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"{\"vsync\": false}\n"));
    written.map_err(|e| format!("cannot write {}: {e}", file.display()))
}

/// Polls the game's Info until it reports the window settled, or the limit
/// passes. Returns the latest Info and whether it was settled.
fn settled_info(socket_path: &Path, limit: Duration) -> (Option<Value>, bool) {
    let deadline = Instant::now() + limit;
    loop {
        let info = socket::request_timeout(socket_path, &Verb::Info, Some(Duration::from_secs(5))).ok().and_then(|r| r.data);
        let settled = info.as_ref().is_some_and(is_settled);
        if settled || Instant::now() >= deadline {
            return (info, settled);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether an Info reply says the window has settled (window.settled).
fn is_settled(info: &Value) -> bool {
    info.pointer("/window/settled").and_then(Value::as_bool).unwrap_or(false)
}


/// Whether a verb runs in game time: it starts a goal that the game finishes
/// later, as the world advances.
fn takes_game_time(cmd: &Verb) -> bool {
    matches!(cmd, Verb::MoveTo(_) | Verb::Interact(_) | Verb::TalkTo(_) | Verb::Transition(_) | Verb::WaitUntil(_))
}

/// Clears the way for a new game's control socket. A socket that still answers
/// belongs to a game that is running (an orphan of an earlier server), so it is
/// left alone; a stale one is removed. A path that is not a socket is refused.
fn clear_stale_socket(path: &Path) -> Result<(), String> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if !meta.file_type().is_socket() {
        return Err(format!("{} exists and is not a socket; not removing it", path.display()));
    }
    if UnixStream::connect(path).is_ok() {
        return Err(format!("a game still answers on {}; stop it first", path.display()));
    }
    std::fs::remove_file(path).map_err(|e| format!("cannot remove the stale socket {}: {e}", path.display()))
}

impl Supervisor {
    /// A goal call, watched. When the game's world time stands still for
    /// STALL_LIMIT (paused, in a movie, loading) the goal is stopped and the
    /// answer says so. A call that passes GOAL_CAP plus SPEECH_SLACK is stopped too.
    fn forward_goal(&self, socket_path: &Path, cmd: &Verb) -> Reply {
        let limit = GOAL_CAP + SPEECH_SLACK;
        let done = AtomicBool::new(false);
        let stalled: Mutex<Option<String>> = Mutex::new(None);
        let reply = std::thread::scope(|scope| {
            let watch = scope.spawn(|| watch_progress(socket_path, &done, &stalled));
            let reply = socket::request_timeout(socket_path, cmd, Some(limit));
            done.store(true, Ordering::SeqCst);
            let _ = watch.join();
            reply
        });
        if let Some(reason) = stalled.into_inner().unwrap_or_else(|e| e.into_inner()) {
            return Reply::err(reason);
        }
        match reply {
            Ok(reply) => reply,
            Err(e) if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => {
                let _ = socket::request_timeout(socket_path, &Verb::Stop, Some(Duration::from_secs(5)));
                Reply::err(format!("the goal did not finish within {} s of wall clock time and was stopped", limit.as_secs()))
            }
            Err(e) => Reply::err(format!("the game did not answer on {}: {e}", socket_path.display())),
        }
    }
}

/// Reads the game's world time while a goal runs. Time that stands still for
/// STALL_LIMIT stops the goal and records why.
fn watch_progress(socket_path: &Path, done: &AtomicBool, stalled: &Mutex<Option<String>>) {
    let mut last: Option<f64> = None;
    let mut since = Instant::now();
    while !done.load(Ordering::SeqCst) {
        std::thread::sleep(WATCH_EVERY);
        if done.load(Ordering::SeqCst) {
            return;
        }
        let info = socket::request_timeout(socket_path, &Verb::Info, Some(Duration::from_secs(3))).ok().and_then(|r| r.data);
        let Some(info) = info else { continue };
        let time = info.get("time").and_then(Value::as_f64);
        if time.is_some() && time != last {
            last = time;
            since = Instant::now();
            continue;
        }
        if since.elapsed() >= STALL_LIMIT {
            let phase = info.get("phase").and_then(Value::as_str).unwrap_or("unknown");
            let reason = format!(
                "game not advancing (phase {phase}): its game time has stood still for {} s, so the goal was stopped. A paused game, a movie or a loading screen does not advance; resume it and retry.",
                STALL_LIMIT.as_secs()
            );
            *stalled.lock().unwrap_or_else(|e| e.into_inner()) = Some(reason);
            let _ = socket::request_timeout(socket_path, &Verb::Stop, Some(Duration::from_secs(5)));
            return;
        }
    }
}

/// Waits for the child to exit, up to `limit`.
fn wait_for_exit(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => return None,
        }
    }
}

/// Sends a signal to a process this supervisor started.
fn send_signal(pid: u32, sig: i32) {
    // SAFETY: kill(2) only reads its arguments. The pid is the child this
    // supervisor spawned and still holds; a reused pid cannot occur while the
    // Child handle is alive, because it has not been reaped.
    unsafe {
        libc::kill(pid as i32, sig);
    }
}

#[cfg(test)]
mod tests;
