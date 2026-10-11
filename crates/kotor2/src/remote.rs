//! Remote control socket (`--control` or K2_CONTROL=1): the control API of
//! k2-world (control.rs) and the host verbs of this process, served on
//! `<control dir>/control.sock` for the MCP supervisor (crates/k2-mcp).
//! Off by default.
//!
//! Each frame the socket is read and the requests run on the main thread.
//! Semantic verbs (move_to, interact, ...) go to the control API; host verbs
//! are answered here: input (remote_input.rs), screenshot, pick and project,
//! log filter, trace file, info, save and load (through the game's own save
//! path), quit. A request that takes time answers its client when it finishes:
//! a goal (one at a time), or one job (an input plan or a screenshot). A goal
//! and a screenshot can run together; input waits for a goal to finish. Input
//! goes through the window's input messages, so it reaches the game the way a
//! player's does.
//!
//! Files the requests name (screenshots, traces) stay under K2_SHOT_DIR and
//! K2_TRACE_DIR. A name that resolves elsewhere, or that exists already, is
//! refused.

use std::collections::VecDeque;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bevy::ecs::system::SystemParam;
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::window::PrimaryWindow;
use k2_script::ObjectId;
use k2_world::control::socket::{self, Server};
use k2_world::control::{Command, Control, Env, Reply};
use k2_world::World;
use serde_json::json;

use crate::area::Interactable;
use crate::game::{AppState, MovieCards, NeedAreaLoad, ObjEntities, Paused, WorldRes};
use crate::hover;
use crate::logging;
use crate::movie;
use crate::remote_input::{self, InputQueue, Step};
use crate::bark::Bubble;
use crate::dialog_ui::LineState;
use crate::gameover::GameOver;
use crate::gui::menus::{MenuScreen, OpenMenu, close_menu};
use crate::progress_ui::LevelUpUi;
use crate::gui::screens::ScreenState;
use crate::saves::{PendingLoad, SaveJob, SaveRoot, save_now};

mod screen;
pub(crate) mod speech;

/// Flags owned by this module (no value).
pub const SWITCHES: [&str; 1] = ["--control"];

/// Seconds a screenshot may take before it is reported as failed.
const SHOT_TIMEOUT: f64 = 15.0;
/// Seconds input delivery may take before it is reported as failed. A longer
/// plan gets its own length on top (INPUT_SLACK).
const INPUT_TIMEOUT: f64 = 30.0;
/// Seconds a plan gets past its last event before its job is reported as timed out.
const INPUT_SLACK: f64 = 5.0;
/// Frames the window must keep its size and scale before info reports it settled.
const SETTLED_FRAMES: u32 = 10;

pub struct RemotePlugin;

impl Plugin for RemotePlugin {
    fn build(&self, app: &mut App) {
        if !enabled() {
            return;
        }
        app.init_resource::<Remote>()
            .init_resource::<InputQueue>()
            .init_resource::<ShotDone>()
            .add_systems(Startup, bind)
            .add_systems(Update, (serve, remote_input::deliver_input).chain());
    }
}

/// A request that takes time: its client waits for it.
struct Job {
    client: u64,
    work: Work,
}

enum Work {
    /// Input is being delivered; the reply goes out when the queue is empty.
    Input { reply: Reply, deadline: f64 },
    /// A pick: the pointer has moved, and the object under it is read once the queue is empty.
    Pick { deadline: f64 },
    /// A screenshot: its observer reports the save through [`ShotDone`].
    Shot { path: PathBuf, scale: f32, started: f64 },
    /// A key_press: the key is held until the game clock has run its hold.
    Hold(Box<KeyHold>),
}

#[derive(Resource, Default)]
struct Remote {
    server: Option<Server>,
    control: Control,
    /// The client waiting for the running goal.
    waiter: Option<u64>,
    job: Option<Job>,
    /// Physical size and scale factor of the window, and how many frames they have held.
    window: Option<(u32, u32, f32)>,
    held_frames: u32,
    /// Calls waiting in order behind the speech or dialog ahead of them (speech.rs).
    queue: VecDeque<Queued>,
    /// Wait notes for the replies of calls that waited, by client.
    notes: Vec<(u64, Waited)>,
}

/// A call waiting in the queue: one that has not run yet, or a dialog call that has
/// run and answers once the conversation is ready for input again.
enum Queued {
    Call { client: u64, command: Command, wait: Option<speech::Wait> },
    /// The dialog call's message, the line count when it ran (a new line changes it),
    /// and its wait.
    Answer { client: u64, message: String, serial: u64, wait: speech::Wait },
}

impl Queued {
    fn call(incoming: socket::Incoming) -> Self {
        Self::Call { client: incoming.client, command: incoming.command, wait: None }
    }

    fn wait(&self) -> Option<&speech::Wait> {
        match self {
            Self::Call { wait, .. } => wait.as_ref(),
            Self::Answer { wait, .. } => Some(wait),
        }
    }
}

/// The outcome of the screenshot in flight: its size, or why it was not saved.
#[derive(Resource, Default)]
struct ShotDone(Option<Result<(u32, u32), String>>);

/// Read-only state the replies report on.
#[derive(SystemParam)]
struct Frame<'w, 's> {
    paused: Res<'w, Paused>,
    need: Res<'w, NeedAreaLoad>,
    movie: Res<'w, MovieCards>,
    screens: Res<'w, ScreenState>,
    game_over: Res<'w, GameOver>,
    level: Res<'w, LevelUpUi>,
    line: Res<'w, LineState>,
    bubble: Res<'w, Bubble>,
    hover: Res<'w, hover::Target>,
    ents: Res<'w, ObjEntities>,
    cams: Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<Camera3d>>,
    bodies: Query<'w, 's, (&'static GlobalTransform, Option<&'static Interactable>)>,
}

/// The open menu (the container, inventory and other windows, the pause menu),
/// which closes through `close_menu` like Escape does.
#[derive(SystemParam)]
struct MenuState<'w, 's> {
    open: ResMut<'w, OpenMenu>,
    existing: Query<'w, 's, Entity, With<MenuScreen>>,
}

/// Whether this run serves the control socket (--control or K2_CONTROL=1).
pub fn enabled() -> bool {
    std::env::args().any(|a| a == "--control") || std::env::var("K2_CONTROL").is_ok_and(|v| v == "1")
}

/// Binds the socket in the control directory. Screenshot runs have no control
/// directory unless K2_CTL_DIR is set (as for the restart channel).
fn bind(mut remote: ResMut<Remote>) {
    let screenshot = std::env::args().any(|a| a == "--screenshot");
    if screenshot && std::env::var_os("K2_CTL_DIR").is_none_or(|v| v.is_empty()) {
        warn!("control: no control socket in screenshot runs (set K2_CTL_DIR)");
        return;
    }
    let dir = socket::default_dir();
    // Private: the socket is owner-only, and its folder is too.
    if let Err(e) = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir) {
        warn!("control: cannot create {}: {e}", dir.display());
        return;
    }
    match Server::bind(&socket::socket_path(&dir)) {
        Ok(server) => {
            info!("control: listening on {}", server.path().display());
            remote.server = Some(server);
        }
        Err(e) => warn!("control: cannot bind the socket in {}: {e}", dir.display()),
    }
}

/// Answers finished work, then runs the requests that arrived this frame.
#[allow(clippy::too_many_arguments)]
fn serve(
    mut remote: ResMut<Remote>,
    mut world: ResMut<WorldRes>,
    mut input: ResMut<InputQueue>,
    mut shot: ResMut<ShotDone>,
    state: Res<State<AppState>>,
    frame: Frame,
    mut menus: MenuState,
    root: Res<SaveRoot>,
    mut save_job: ResMut<SaveJob>,
    mut load: ResMut<PendingLoad>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    let _span = bevy::log::info_span!("remote_serve").entered();
    let Remote { server, control, waiter, job, window, held_frames, queue, notes } = &mut *remote;
    let Some(server) = server.as_mut() else { return };
    let w = &mut world.0;
    let now = time.elapsed_secs_f64();
    let win = windows.single().ok();
    let size = win.map(|v| Vec2::new(v.width(), v.height()));
    if let Some(v) = win {
        let seen = (v.physical_width(), v.physical_height(), v.scale_factor());
        if *window == Some(seen) {
            *held_frames = held_frames.saturating_add(1);
        } else {
            *window = Some(seen);
            *held_frames = 0;
        }
    }
    let env = Env { save_root: &root.0, paused: frame.paused.0, movie: movie::movie_up(&frame.movie) };
    // Why the game clock stands still now, if it does: a held key cannot run on it.
    let stopped = stopped_reason(*state.get() == AppState::Game, frame.need.0 || load.0.is_some(), frame.paused.0, w.conversation.is_some(), movie::movie_up(&frame.movie));

    if let Some(reply) = control.poll(w, &env)
        && let Some(client) = waiter.take()
    {
        server.reply(client, &with_note(reply, take_note(notes, client).as_ref()));
    }
    if let Some(current) = job.take() {
        match job_done(current, &mut input, &mut shot, now, w, &frame, stopped) {
            Ok((client, reply)) => server.reply(client, &with_note(reply, take_note(notes, client).as_ref())),
            Err(still_running) => *job = Some(still_running),
        }
    }

    // Calls that arrive queue in order behind the ones waiting ahead of them. Stop, quit and
    // game_status jump the queue: they run at once.
    let (jump, behind): (Vec<_>, Vec<_>) = server.poll().into_iter().partition(|incoming| speech::exempt(&incoming.command));
    for incoming in jump.into_iter().rev() {
        queue.push_front(Queued::call(incoming));
    }
    queue.extend(behind.into_iter().map(Queued::call));
    // Runs the queue in order. A call that must wait goes back to the front, and the calls
    // behind it keep their place.
    while let Some(item) = queue.pop_front() {
        let (client, cmd, wait) = match item {
            Queued::Answer { client, message, serial, mut wait } => {
                wait.track(now, speech_now(w, &frame).as_ref());
                let ready = dialog_ready(w, &frame, serial);
                if !ready && !wait.overdue(now) {
                    queue.push_front(Queued::Answer { client, message, serial, wait });
                    break;
                }
                let waited = now - wait.started;
                let mut reply = dialog_answer(w, &frame, &message);
                if !ready {
                    reply.message = format!("{}; {}", reply.message, speech::unsettled(waited));
                }
                server.reply(client, &with_note(reply, Some(&Waited { text: speech::wait_note(&wait.speaker, waited), secs: waited })));
                continue;
            }
            Queued::Call { client, command, wait } => (client, command, wait),
        };
        // The wait so far, for the reply of a call that was held.
        let note = wait.as_ref().map(|wait| Waited { text: speech::wait_note(&wait.speaker, now - wait.started), secs: now - wait.started });
        debug!("remote: {} from client {client}", cmd.verb());
        if let Some(message) = busy(&cmd, job, control) {
            server.reply(client, &with_note(Reply::err(message), note.as_ref()));
            continue;
        }
        // What is on screen takes the input: a verb it does not take is refused now, never queued.
        let in_game = *state.get() == AppState::Game;
        let loading = frame.need.0 || load.0.is_some();
        let here = screen_here(w, &frame, &menus, in_game, loading);
        if let Some(message) = screen::refusal(&here, &cmd) {
            server.reply(client, &with_note(Reply::err(message), note.as_ref()));
            continue;
        }
        // While someone speaks, the call waits for the speech to end (never refused for it).
        if let Some(s) = speech_now(w, &frame).filter(|s| speech::must_wait(&cmd, Some(s))) {
            let fresh = wait.is_none();
            let mut wait = wait.unwrap_or_else(|| speech::Wait::new(now, &s.speaker, now));
            wait.track(now, Some(&s));
            if wait.overdue(now) {
                server.reply(client, &Reply::err(speech::overran(&s.speaker, now - wait.started)));
                continue;
            }
            // The client learns at once how long the call may wait; the answer follows when it runs.
            if fresh {
                let may_wait = wait.cap - now;
                server.reply(client, &held_notice(speech::notice(&s.speaker, may_wait), &s.speaker, may_wait));
            }
            queue.push_front(Queued::Call { client, command: cmd, wait: Some(wait) });
            break;
        }
        let raw = screen::is_raw_input(&cmd);
        let observing = matches!(cmd, Command::Observe(_));
        let dialog = matches!(cmd, Command::ChooseReply(_) | Command::ContinueDialog);
        // take_all closes the container window; take_item closes it once the container is empty.
        let emptied = match &cmd {
            Command::TakeAll(_) => Some(None),
            Command::TakeItem(a) => Some(Some(a.container)),
            _ => None,
        };
        let pending = match cmd {
            Command::Stop => {
                if let Some(waiting) = stop_input(&mut input, job, now) {
                    server.reply(waiting, &Reply::err("cancelled: the input job was stopped"));
                }
                control.submit(w, &env, Command::Stop)
            }
            Command::Save(args) => Some(save_slot(w, &root, &mut save_job, &args.slot, args.name.as_deref())),
            Command::Load(args) => Some(load_slot(&root, &mut load, *state.get() == AppState::Game, &args.slot)),
            Command::Info => Some(info(w, &frame, &state, &load, win, *held_frames >= SETTLED_FRAMES)),
            Command::Quit => {
                exit.write(AppExit::Success);
                Some(Reply::ok("quitting: the normal exit save runs"))
            }
            Command::SetLogFilter(args) => Some(match logging::set_filter(&args.filter) {
                Ok(()) => Reply::ok_with(format!("log filter {}", args.filter), json!({ "filter": logging::filter_text() })),
                Err(e) => Reply::err(e),
            }),
            Command::TraceStart(args) => Some(trace_start(args.path.as_deref(), args.filter.as_deref())),
            Command::TraceStop => Some(match logging::stop_trace() {
                Ok(Some(path)) => Reply::ok_with(format!("trace written to {}", path.display()), json!({ "path": path })),
                Ok(None) => Reply::ok("no trace was running"),
                Err(e) => Reply::err(e),
            }),
            Command::Screenshot(args) => match confine(Output::Shot, args.path.as_deref()) {
                Ok(path) => {
                    let scale = win.map_or(1.0, |v| v.scale_factor());
                    let target = path.clone();
                    // A capture that came late from an earlier, timed-out shot must not answer this one.
                    shot.0 = None;
                    commands.spawn(Screenshot::primary_window()).observe(move |captured: On<ScreenshotCaptured>, mut done: ResMut<ShotDone>| {
                        done.0 = Some(save_capture(&captured.image, &target));
                    });
                    *job = Some(Job { client, work: Work::Shot { path, scale, started: now } });
                    None
                }
                Err(e) => Some(Reply::err(e)),
            },
            Command::KeyPress(a) => match stopped {
                Some(why) => Some(Reply::err(format!("key_press refused: the game {why}; nothing was pressed"))),
                None => start_hold(&mut input, job, client, now, w, &a.key, &a.modifiers.clone().unwrap_or_default(), a.hold_ms),
            },
            Command::KeyDown(a) => {
                let plan = remote_input::plan_key_down(&a.key, &a.modifiers.clone().unwrap_or_default());
                start_input(&mut input, job, client, now, plan, Some("key down"))
            }
            Command::KeyUp(a) => {
                let plan = remote_input::plan_key_up(&a.key, &a.modifiers.clone().unwrap_or_default());
                start_input(&mut input, job, client, now, plan, Some("key up"))
            }
            Command::TypeText(a) => {
                let plan = remote_input::plan_type_text(&a.text, a.delay_ms);
                start_input(&mut input, job, client, now, plan, Some("typing"))
            }
            Command::MouseMove(a) => {
                let plan = remote_input::check_point([a.x, a.y], size).map(|()| remote_input::plan_mouse_move(a.x, a.y));
                start_input(&mut input, job, client, now, plan, Some("pointer move"))
            }
            Command::MouseClick(a) => {
                let plan = remote_input::check_point([a.x, a.y], size).map(|()| remote_input::plan_mouse_click(a.x, a.y, a.button, a.clicks, a.hold_ms));
                start_input(&mut input, job, client, now, plan, Some("click"))
            }
            Command::MouseDrag(a) => {
                let plan = check_points(&[a.from, a.to], size).map(|()| remote_input::plan_mouse_drag(a.from, a.to, a.button, a.steps));
                start_input(&mut input, job, client, now, plan, Some("drag"))
            }
            Command::Scroll(a) => {
                let at = a.x.zip(a.y).map(|(x, y)| [x, y]);
                let plan = check_points(&at.into_iter().collect::<Vec<_>>(), size).map(|()| remote_input::plan_scroll(at, a.lines_x, a.lines_y));
                start_input(&mut input, job, client, now, plan, Some("scroll"))
            }
            Command::Pick(a) => {
                let plan = remote_input::check_point([a.x, a.y], size).map(|()| remote_input::plan_mouse_move(a.x, a.y));
                start_input(&mut input, job, client, now, plan, None)
            }
            Command::Project(a) => Some(project_reply(w, &frame, size, a.object)),
            cmd => {
                let reply = control.submit(w, &env, cmd);
                if reply.is_none() {
                    *waiter = Some(client);
                }
                reply
            }
        };
        // Observe says what is open first.
        let pending = match pending {
            Some(reply) if observing => Some(match screen::open_line(&here) {
                Some(line) => with_open(reply, &line),
                None => reply,
            }),
            other => other,
        };
        // A take that empties the container closes its window, as KotOR does after take all.
        if let (Some(reply), Some(check)) = (&pending, emptied)
            && reply.ok
            && menus.open.0.as_deref() == Some("container")
        {
            let empty = match check {
                None => true,
                Some(c) => w.container_views(c).is_empty(),
            };
            if empty {
                close_menu(&mut commands, &mut menus.open, &menus.existing);
            }
        }
        // Raw input that started a job says which window has focus.
        if pending.is_none() && raw && let Some(label) = screen::focus(&here) {
            attach_focus(job, &label);
        }
        match pending {
            // A dialog call answers once the conversation is ready for input again (Queued::Answer).
            Some(reply) if dialog && reply.ok && w.conversation.is_some() => {
                let speaker = conversation_speaker(w);
                let may_wait = speech::HOLD_PAD;
                server.reply(client, &held_notice(speech::dialog_notice(&speaker, may_wait), &speaker, may_wait));
                let wait = speech::Wait::new(now, &speaker, now + may_wait);
                queue.push_front(Queued::Answer { client, message: reply.message, serial: frame.line.serial(), wait });
                break;
            }
            Some(reply) => server.reply(client, &with_note(reply, note.as_ref())),
            // A goal or a job answers later; its reply starts with the same note.
            None => {
                if let Some(waited) = note {
                    notes.push((client, waited));
                }
            }
        }
    }
    // Calls queued behind a waiting one are told once how long they may wait.
    if let Some((speaker, cap)) = queue.front().and_then(Queued::wait).map(|wait| (wait.speaker.clone(), wait.cap)) {
        for item in queue.iter_mut() {
            if let Queued::Call { client, wait, .. } = item
                && wait.is_none()
            {
                let may_wait = cap - now;
                server.reply(*client, &held_notice(speech::notice(&speaker, may_wait), &speaker, may_wait));
                *wait = Some(speech::Wait::new(now, &speaker, cap));
            }
        }
    }
}

/// What is being spoken right now: an NPC line still playing, or a bark (a one-liner too).
fn speech_now(w: &World, frame: &Frame) -> Option<speech::Speech> {
    if let (Some(left), Some(total)) = (frame.line.speaking_left(), frame.line.speaking_total()) {
        return Some(speech::Speech { speaker: conversation_speaker(w), left, total });
    }
    let (left, total) = (frame.bubble.speaking_left()?, frame.bubble.speaking_total()?);
    Some(speech::Speech { speaker: "the bark".to_owned(), left, total })
}

/// Who the conversation is with, or "the speaker" when none is running.
fn conversation_speaker(w: &World) -> String {
    w.conversation_view().map_or_else(|| "the speaker".to_owned(), |v| w.name_of(v.speaker))
}

/// Whether a dialog call's answer can go out: the conversation has ended, or the line
/// it moved to (`serial` is the line count before the call ran) is shown and spoken,
/// and offers replies or a message box.
fn dialog_ready(w: &World, frame: &Frame, serial: u64) -> bool {
    if w.conversation.is_none() {
        return true;
    }
    let offers = w.conversation_view().is_some_and(|v| v.boxed || v.replies.iter().any(|r| !r.text.trim().is_empty()));
    frame.line.serial() != serial && speech_now(w, frame).is_none() && offers
}

/// A dialog call's answer: its message, and the conversation as it stands now.
fn dialog_answer(w: &mut World, frame: &Frame, message: &str) -> Reply {
    let ended = w.conversation.is_none();
    let conversation = k2_world::control::observe(w, frame.paused.0, 0.0).conversation;
    let message = if ended { format!("{message}; conversation ended") } else { message.to_owned() };
    Reply::ok_with(message, json!({ "conversation": conversation }))
}

/// The interim frame a held call sends at once: its message, its speaker, and the
/// seconds it may still wait (`interim` "held"). The client reads it and extends its limit.
fn held_notice(message: String, speaker: &str, wait_s: f64) -> Reply {
    Reply::ok_with(message, json!({ "interim": "held", "wait_s": wait_s, "speaker": speaker }))
}

/// The wait a call made before it ran: the text its reply starts with, and the seconds.
struct Waited {
    text: String,
    secs: f64,
}

/// Starts a reply with a call's wait ("waited 3.2 s for Kreia to finish speaking; <message>")
/// and gives it the seconds as `waited_s`.
fn with_note(mut reply: Reply, waited: Option<&Waited>) -> Reply {
    if let Some(waited) = waited {
        reply.message = format!("{}; {}", waited.text, reply.message);
        let data = reply.data.get_or_insert_with(|| json!({}));
        if let Some(map) = data.as_object_mut() {
            map.insert("waited_s".to_owned(), json!(waited.secs));
        }
    }
    reply
}

/// Takes the wait recorded for a client's call.
fn take_note(notes: &mut Vec<(u64, Waited)>, client: u64) -> Option<Waited> {
    let i = notes.iter().position(|(c, _)| *c == client)?;
    Some(notes.swap_remove(i).1)
}

/// What the player's input is going to right now (screen.rs reads the state).
fn screen_here(w: &mut World, frame: &Frame, menus: &MenuState, in_game: bool, loading: bool) -> screen::Screen {
    let menu = menus.open.0.as_deref();
    let container = frame.screens.container.filter(|_| menu == Some("container")).map(|c| (w.name_of(c), w.container_views(c).len()));
    screen::screen_now(&screen::Look {
        in_game,
        loading,
        movie: movie::movie_up(&frame.movie),
        game_over: frame.game_over.is_open(),
        conversation: w.conversation.is_some(),
        box_up: w.conversation_view().is_some_and(|v| v.boxed),
        level_up: frame.level.is_open(),
        menu,
        container,
        paused: frame.paused.0,
    })
}

/// Puts the line for what is open first in observe's answer, and in its data.
fn with_open(mut reply: Reply, line: &str) -> Reply {
    reply.message = format!("{line}; {}", reply.message);
    let data = reply.data.get_or_insert_with(|| json!({}));
    if let Some(map) = data.as_object_mut() {
        map.insert("open".to_owned(), json!(line));
    }
    reply
}

/// Records on a raw-input job which window has focus, so its reply says where the input went.
fn attach_focus(job: &mut Option<Job>, label: &str) {
    match job.as_mut().map(|j| &mut j.work) {
        Some(Work::Input { reply, .. }) => reply.message.push_str(&format!(", input went to the open {label} window")),
        Some(Work::Hold(hold)) => hold.focus = Some(label.to_owned()),
        _ => {}
    }
}

/// Why a request waits or is refused while work runs, if it does. Reads and
/// stop and quit always answer. A screenshot does not stop a goal or the
/// log and trace verbs; input does.
fn busy(cmd: &Command, job: &Option<Job>, control: &Control) -> Option<String> {
    if cmd.is_read_only() || matches!(cmd, Command::Stop | Command::Quit) {
        return None;
    }
    if let Some(job) = job {
        let quiet = matches!(cmd, Command::SetLogFilter(_) | Command::TraceStart(_) | Command::TraceStop);
        if !quiet {
            return Some(match job.work {
                Work::Shot { .. } => "busy: a screenshot is still being written; observe, or stop".to_owned(),
                _ => "busy: an input job is still running; observe, or stop".to_owned(),
            });
        }
    }
    control.refusal(cmd).map(|r| r.message)
}

/// The job's answer, once it is done: its client and reply. Otherwise the job
/// comes back, still running.
fn job_done(job: Job, input: &mut InputQueue, shot: &mut ShotDone, now: f64, w: &World, frame: &Frame, stopped: Option<&'static str>) -> Result<(u64, Reply), Job> {
    let Job { client, work } = job;
    match work {
        Work::Input { reply, deadline } => {
            if input.is_idle() {
                return Ok((client, reply));
            }
            if now > deadline {
                return Ok((client, timed_out("the input", now, input)));
            }
            Err(Job { client, work: Work::Input { reply, deadline } })
        }
        Work::Pick { deadline } => {
            if input.is_idle() {
                return Ok((client, pick_reply(w, frame)));
            }
            if now > deadline {
                return Ok((client, timed_out("the pointer move", now, input)));
            }
            Err(Job { client, work: Work::Pick { deadline } })
        }
        Work::Shot { path, scale, started } => match shot.0.take() {
            Some(Ok((width, height))) => Ok((
                client,
                Reply::ok_with(
                    format!("screenshot {}", path.display()),
                    json!({ "path": path, "width": width, "height": height, "scale_factor": scale }),
                ),
            )),
            Some(Err(e)) => Ok((client, Reply::err(format!("screenshot {} failed: {e}", path.display())))),
            None if now - started > SHOT_TIMEOUT => Ok((client, Reply::err(format!("screenshot {} did not finish", path.display())))),
            None => Err(Job { client, work: Work::Shot { path, scale, started } }),
        },
        Work::Hold(mut hold) => {
            // Released: the reply goes out once the release has been delivered.
            if let Some((why, to, game_s)) = hold.released.clone() {
                if input.is_idle() {
                    return Ok((client, hold_reply(&hold, &why, to, game_s)));
                }
                return Err(Job { client, work: Work::Hold(hold) });
            }
            match hold_step(&hold, w.time, now, stopped) {
                HoldStep::Keep => Err(Job { client, work: Work::Hold(hold) }),
                HoldStep::Release(why) => {
                    input.push(now, remote_input::plan_key_up(&hold.key, &hold.modifiers).unwrap_or_default());
                    hold.released = Some((why, leader_position(w), w.time - hold.start));
                    Err(Job { client, work: Work::Hold(hold) })
                }
            }
        }
    }
}

/// Queues an input plan as the running job for `client`. With a `what`, the
/// client gets "<what>: n input events delivered" once every event has gone
/// out; without one (a pick) it gets the object under the pointer. A plan that
/// cannot be built is refused at once. The job's deadline allows for the plan's
/// own length (its last event) on top of INPUT_TIMEOUT.
fn start_input(input: &mut InputQueue, job: &mut Option<Job>, client: u64, now: f64, plan: Result<Vec<Step>, String>, what: Option<&str>) -> Option<Reply> {
    match plan {
        Ok(steps) => {
            let n = steps.len();
            let last_ms = steps.iter().map(|s| s.at_ms).fold(0.0, f64::max);
            let deadline = now + INPUT_TIMEOUT.max(last_ms / 1000.0 + INPUT_SLACK);
            input.push(now, steps);
            let work = match what {
                Some(what) => Work::Input { reply: Reply::ok(format!("{what}: {n} input events delivered")), deadline },
                None => Work::Pick { deadline },
            };
            *job = Some(Job { client, work });
            None
        }
        Err(e) => Some(Reply::err(e)),
    }
}

/// Wall-clock seconds a key hold may run past its game-time length before it is
/// released anyway (the game clock is not moving: a stalled load, say).
const HOLD_WALL_SLACK: f64 = 10.0;

/// A key held for game time (key_press). The key comes up when the game clock
/// (`World::time`) has run `hold_s` seconds, so a frame over the clamp does not
/// shorten it, and a paused, talking or movie-showing game does not spend it:
/// a stopped game releases the key at once (see [`hold_step`]).
struct KeyHold {
    key: String,
    modifiers: Vec<String>,
    hold_s: f64,
    /// Game time at the press, and the game time the key comes up at.
    start: f64,
    until: f64,
    /// Wall-clock time (seconds) past which the key comes up whatever the game clock says.
    limit: f64,
    /// The leader's position at the press, [x, y, z].
    from: [f32; 3],
    /// Set once the release is queued: why, the leader's position then, and the game seconds held.
    released: Option<(String, [f32; 3], f64)>,
    /// The open window the key went to, when one had focus (named in the reply).
    focus: Option<String>,
}

/// What a held key does next.
#[derive(Debug, PartialEq)]
enum HoldStep {
    Keep,
    /// Release now, with the reason for the reply.
    Release(String),
}

/// The hold rule (see [`KeyHold`]): `game` and `real` are the game and wall-clock
/// times now, and `stopped` is why the game clock stands still, if it does.
fn hold_step(hold: &KeyHold, game: f64, real: f64, stopped: Option<&str>) -> HoldStep {
    if let Some(why) = stopped {
        return HoldStep::Release(format!("released early: the game {why} after {:.2} s of game time", game - hold.start));
    }
    if game >= hold.until {
        return HoldStep::Release(format!("held {:.2} s of game time", hold.hold_s));
    }
    if real > hold.limit {
        return HoldStep::Release(format!(
            "released at the wall-clock limit after {:.2} of {:.2} s of game time: the game clock did not advance",
            game - hold.start,
            hold.hold_s
        ));
    }
    HoldStep::Keep
}

/// Why the game clock stands still, so a key cannot be held on it. None when it runs.
fn stopped_reason(in_game: bool, loading: bool, paused: bool, conversation: bool, movie: bool) -> Option<&'static str> {
    if !in_game {
        Some("is not in the game")
    } else if movie {
        Some("is showing a movie")
    } else if loading {
        Some("is loading")
    } else if paused {
        Some("is paused")
    } else if conversation {
        Some("is in a conversation")
    } else {
        None
    }
}

/// The leader's position, [x, y, z] (the origin with no leader).
fn leader_position(w: &World) -> [f32; 3] {
    w.get(w.party_leader()).map_or([0.0; 3], |o| o.position.to_array())
}

/// Horizontal distance between two positions, metres.
fn moved_m(from: [f32; 3], to: [f32; 3]) -> f32 {
    ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt()
}

/// The answer to a key_press once its key is up.
fn hold_reply(hold: &KeyHold, why: &str, to: [f32; 3], game_s: f64) -> Reply {
    let moved = moved_m(hold.from, to);
    let mut reply = Reply::ok_with(format!("key press {why}; the leader moved {moved:.2} m"), json!({ "held_s": game_s, "moved_m": moved, "position": to }));
    if let Some(label) = &hold.focus {
        reply.message.push_str(&format!(", input went to the open {label} window"));
    }
    reply
}

/// Starts a key_press: the key goes down now, and its job releases it when the
/// game clock has run the hold (see [`hold_step`]).
#[allow(clippy::too_many_arguments)]
fn start_hold(input: &mut InputQueue, job: &mut Option<Job>, client: u64, now: f64, w: &World, key: &str, modifiers: &[String], hold_ms: Option<u32>) -> Option<Reply> {
    match remote_input::plan_key_down(key, modifiers) {
        Ok(press) => {
            input.push(now, press);
            let hold_s = remote_input::hold_seconds(hold_ms);
            let start = w.time;
            let hold = KeyHold {
                key: key.to_owned(),
                modifiers: modifiers.to_vec(),
                hold_s,
                start,
                until: start + hold_s,
                limit: now + HOLD_WALL_SLACK + 3.0 * hold_s,
                from: leader_position(w),
                released: None,
                focus: None,
            };
            *job = Some(Job { client, work: Work::Hold(Box::new(hold)) });
            None
        }
        Err(e) => Some(Reply::err(e)),
    }
}

/// The reply for input that was not delivered in time. Whatever was still
/// pending is dropped, and every key and button the window holds is released.
fn timed_out(what: &str, now: f64, input: &mut InputQueue) -> Reply {
    input.abort(now);
    Reply::err(format!(
        "{what} did not finish within the time allowed: the window took no events in time (pending events dropped, held keys and buttons released)"
    ))
}

/// What a stop does to input: a running plan is dropped, and every key and
/// button the window holds is released, whether or not a plan is running (a
/// key_down holds until its key_up, so a finished key_down is still held). Returns
/// the client waiting on a cancelled input job, to answer.
fn stop_input(input: &mut InputQueue, job: &mut Option<Job>, now: f64) -> Option<u64> {
    input.abort(now);
    match job.take() {
        Some(Job { client, work: Work::Input { .. } | Work::Pick { .. } | Work::Hold(_) }) => Some(client),
        keep => {
            *job = keep;
            None
        }
    }
}

/// Checks several pointer positions; see [`remote_input::check_point`].
fn check_points(points: &[[f32; 2]], size: Option<Vec2>) -> Result<(), String> {
    points.iter().try_for_each(|p| remote_input::check_point(*p, size))
}

/// The object under the pointer, from the hover target of the last frame.
fn pick_reply(w: &World, frame: &Frame) -> Reply {
    let found = frame.hover.0.and_then(|e| frame.ents.0.iter().find(|(_, v)| **v == e).map(|(id, _)| *id));
    match found {
        Some(id) => {
            let name = w.name_of(id);
            let kind = w.get(id).map(|o| format!("{:?}", o.kind).to_ascii_lowercase());
            Reply::ok_with(format!("{name} is under the pointer"), json!({ "object": id, "name": name, "kind": kind }))
        }
        None => Reply::ok_with("nothing is under the pointer", json!({ "object": null, "name": null, "kind": null })),
    }
}

/// Where an object is on the window, in logical pixels. Behind the camera, x
/// and y are null; off the window, on_screen is false.
fn project_reply(w: &World, frame: &Frame, size: Option<Vec2>, id: ObjectId) -> Reply {
    let Some(entity) = frame.ents.0.get(&id) else {
        return Reply::err(format!("object {id} has no body in the game"));
    };
    let Ok((gt, interactable)) = frame.bodies.get(*entity) else {
        return Reply::err(format!("object {id} has no body in the game"));
    };
    let mut at = gt.translation();
    if let Some(it) = interactable {
        at += Vec3::Y * it.height * 0.5;
    }
    let Ok((cam, cam_gt)) = frame.cams.single() else {
        return Reply::err("no camera to project through");
    };
    let name = w.name_of(id);
    match cam.world_to_viewport(cam_gt, at) {
        Ok(p) => {
            let on_screen = size.is_some_and(|s| p.x >= 0.0 && p.y >= 0.0 && p.x < s.x && p.y < s.y);
            Reply::ok_with(format!("{name} at ({:.0}, {:.0})", p.x, p.y), json!({ "object": id, "x": p.x, "y": p.y, "on_screen": on_screen }))
        }
        Err(_) => Reply::ok_with(format!("{name} is behind the camera"), json!({ "object": id, "x": null, "y": null, "on_screen": false })),
    }
}

/// Writes a captured frame as PNG (RGB: the alpha channel can carry brightness
/// when HDR is on). Returns the image size.
fn save_capture(image: &Image, path: &Path) -> Result<(u32, u32), String> {
    let dynamic = image.clone().try_into_dynamic().map_err(|e| e.to_string())?;
    dynamic.to_rgb8().save(path).map_err(|e| e.to_string())?;
    info!("screenshot saved to {}", path.display());
    Ok((image.width(), image.height()))
}

/// What the game is doing, for the supervisor's status.
fn info(w: &World, frame: &Frame, state: &State<AppState>, load: &PendingLoad, win: Option<&Window>, settled: bool) -> Reply {
    let speaking = speech_now(w, frame);
    let phase = movie::phase(
        *state.get() == AppState::Game,
        movie::movie_up(&frame.movie),
        frame.need.0 || load.0.is_some(),
        w.conversation.is_some(),
        w.minigame.is_some(),
        frame.paused.0,
    );
    let module = w.module.as_ref().map_or("no module", |m| m.name.as_str());
    let (width, height, physical_width, physical_height, scale) = win.map_or((0.0, 0.0, 0, 0, 1.0), |v| {
        (v.width(), v.height(), v.physical_width(), v.physical_height(), v.scale_factor())
    });
    Reply::ok_with(
        format!("{phase} in {module}"),
        json!({
            "phase": phase,
            "state": format!("{:?}", state.get()),
            "module": w.module.as_ref().map(|m| m.name.clone()),
            "time": w.time,
            "paused": frame.paused.0,
            "speaking": speaking.is_some(),
            "speaking_left_s": speaking.as_ref().map(|s| s.left),
            "speaking_total_s": speaking.as_ref().map(|s| s.total),
            // The seconds until a call sent now would be refused at its cap (speech.rs).
            "hold_left_s": speaking.as_ref().map(|s| f64::from(s.left) + speech::HOLD_PAD),
            "window": {
                "width": width,
                "height": height,
                "physical_width": physical_width,
                "physical_height": physical_height,
                "scale_factor": scale,
                "settled": settled,
            },
            "log_filter": logging::filter_text(),
            "trace_file": logging::trace_file(),
        }),
    )
}

/// The files the host writes for a request.
#[derive(Clone, Copy)]
enum Output {
    Shot,
    Trace,
}

impl Output {
    fn env(self) -> &'static str {
        match self {
            Output::Shot => "K2_SHOT_DIR",
            Output::Trace => "K2_TRACE_DIR",
        }
    }

    fn folder(self) -> &'static str {
        match self {
            Output::Shot => "kotor2-rs-shots",
            Output::Trace => "kotor2-rs-traces",
        }
    }

    fn stem(self) -> &'static str {
        match self {
            Output::Shot => "shot",
            Output::Trace => "trace",
        }
    }

    fn ext(self) -> &'static str {
        match self {
            Output::Shot => "png",
            Output::Trace => "json",
        }
    }
}

/// The folder a kind of file goes to: K2_SHOT_DIR or K2_TRACE_DIR, else a
/// folder of the temp directory.
fn output_root(kind: Output) -> PathBuf {
    std::env::var_os(kind.env()).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join(kind.folder()))
}

/// The file a request writes, under the kind's folder.
fn confine(kind: Output, requested: Option<&str>) -> Result<PathBuf, String> {
    confine_in(&output_root(kind), kind, requested)
}

/// A new file under `root`. The request names a file: a bare name is taken
/// from `root`, and a path must resolve (symlinks included) inside it. A name
/// that exists already is refused, so no file is ever overwritten.
fn confine_in(root: &Path, kind: Output, requested: Option<&str>) -> Result<PathBuf, String> {
    std::fs::create_dir_all(root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    let root = root.canonicalize().map_err(|e| format!("cannot use {}: {e}", root.display()))?;
    let path = match requested.map(str::trim).filter(|p| !p.is_empty()) {
        None => {
            let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis());
            root.join(format!("{}-{stamp}.{}", kind.stem(), kind.ext()))
        }
        Some(p) => {
            let p = Path::new(p);
            let joined = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
            let name = joined.file_name().ok_or_else(|| format!("{} does not name a file", joined.display()))?.to_owned();
            let parent = joined.parent().unwrap_or(&root);
            let parent = parent.canonicalize().map_err(|_| format!("no folder {}", parent.display()))?;
            if !parent.starts_with(&root) {
                return Err(format!("{} is outside {}", joined.display(), root.display()));
            }
            parent.join(name)
        }
    };
    if std::fs::symlink_metadata(&path).is_ok() {
        return Err(format!("{} already exists", path.display()));
    }
    Ok(path)
}

/// Starts a trace with `filter` (the default when it is blank) into a new file.
fn trace_start(requested: Option<&str>, filter: Option<&str>) -> Reply {
    let path = match confine(Output::Trace, requested) {
        Ok(path) => path,
        Err(e) => return Reply::err(e),
    };
    let filter = filter.filter(|f| !f.trim().is_empty()).unwrap_or(logging::TRACE_FILTER);
    match logging::start_trace(&path, filter) {
        Ok(()) => Reply::ok_with(format!("tracing to {}", path.display()), json!({ "path": path, "filter": filter })),
        Err(e) => Reply::err(e),
    }
}

/// Save through the game's own path, so the slot gets a thumbnail.
fn save_slot(w: &World, root: &SaveRoot, job: &mut SaveJob, slot: &str, name: Option<&str>) -> Reply {
    if let Err(e) = k2_world::control::check_save_slot(slot) {
        return Reply::err(e);
    }
    if job.dir.is_some() {
        return Reply::err("busy: the previous save is still writing its thumbnail; try again");
    }
    match save_now(w, root, slot, name.unwrap_or(slot), job) {
        Ok(_) => Reply::ok(format!("saved {slot}")),
        Err(e) => Reply::err(format!("cannot save now: {e}")),
    }
}

/// Queues a load for the frame after; the game swaps the world in (saves.rs).
fn load_slot(root: &SaveRoot, load: &mut PendingLoad, in_game: bool, slot: &str) -> Reply {
    let valid = !slot.is_empty() && slot.len() <= 64 && slot.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid || !Path::new(&root.0).join(slot).join("save.bin").is_file() {
        return Reply::err(format!("no save slot {slot}"));
    }
    if !in_game {
        return Reply::err("not in game: load from the main menu");
    }
    load.0 = Some(slot.to_owned());
    Reply::ok(format!("loading {slot}; observe to see the result"))
}

#[cfg(test)]
mod tests;
