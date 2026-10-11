//! The MCP surface. Three groups of tools: the supervisor (start, stop,
//! restart, status, logs), the game's eyes and hands (screenshot, keyboard,
//! mouse), and the semantic verbs of the control API. Arguments are typed by
//! their JSON schemas: the verbs' own argument types from k2-world.
//! Resources: the observation and the journal.

use std::collections::BTreeMap;
use std::time::Duration;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use k2_world::control::{
    ChooseReplyArgs, Command, Condition, DebugGiveItemArgs, DebugSetGlobalArgs, DebugTeleportArgs, EquipArgs,
    KeyPressArgs, LevelUpArgs, LoadArgs, MouseClickArgs, MouseDragArgs, MouseMoveArgs, MoveToArgs, ObjectArgs, ObserveArgs,
    PickArgs, SaveArgs, ScreenshotArgs, ScrollArgs, SetLogFilterArgs, SetPartyArgs, TakeItemArgs, TraceStartArgs, TypeTextArgs, UnequipArgs,
    UseItemArgs, UsePowerArgs, UseSkillArgs, WaitUntilArgs,
};
use k2_world::control::Reply;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, InitializeResult, ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::policy;
use crate::render;
use crate::supervisor::{Start, Supervisor};

/// The default radius of observe's nearby list, in metres.
const OBSERVE_RADIUS: f32 = 12.0;

/// The debug_* verbs: they skip the rules, so they are offered only when the
/// server runs with K2_MCP_DEBUG=1.
const DEBUG_TOOLS: &[&str] = &["debug_teleport", "debug_set_global", "debug_give_item"];

const STATE_URI: &str = "k2://state";
const JOURNAL_URI: &str = "k2://journal";

const INSTRUCTIONS: &str = "Plays KotOR 2 (kotor2-rs) in a real window. Start with game_start (it builds the game, snapshots the \
binary and launches it; start chooses the main menu, a new game, character generation, the newest save, a save slot or a module); \
game_status shows what is running and game_stop ends it. Play either by the semantic verbs (observe, move_to, interact, talk_to, \
choose_reply, ...) or by the human-equivalent input (screenshot to see the screen, then mouse_click, key_press, type_text, \
mouse_drag, scroll at window pixel positions). Both act on the same game. Verbs that take game time (move_to, interact, talk_to, \
transition, wait_until) answer when they finish, and one runs at a time. While one runs, observe, inspect, journal, list_saves, \
game_status, project, screenshot, the log and trace calls and stop still answer; keys, pointer input and pick are refused until it \
finishes. Mouse positions are \
logical window pixels; screenshots are physical pixels (see game_status for scale_factor). logs_tail shows the game's output \
(set_log_filter changes the console log level while it runs; trace_start/trace_stop write a JSON span trace with its own filter). \
If the game has exited, every call reports the crash and its last output. The debug_* verbs skip the rules and are for tests only.";

/// game_start arguments.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct GameStartArgs {
    /// How to start: "menu" (the main menu), "new" (a new character through the prologue; default), "chargen" (character generation), "continue" (the newest save), "load:<slot>" or "module:<name>".
    pub start: Option<String>,
    /// Build the game first (cargo build --release -p kotor2) and launch that snapshot. Default true. Set false to relaunch the last snapshot.
    pub build: Option<bool>,
    /// Extra game flags, after the start flag. Only these are accepted. Switches: --control, --newgame, --continue, --chargen, --quickstart, --debug, --autoattack, --mute, --skip-movies, --no-logos. Each with one value: --load-slot, --module, --autowalk, --autouse, --autotalk, --autoattack-door, --cam, --player-appearance, --volume.
    pub args: Option<Vec<String>>,
    /// Extra environment for the game. Names starting with K2_, and WAYLAND_DISPLAY, HOME, XDG_* and LD_*, are refused: the supervisor sets the run's folders and window.
    pub env: Option<BTreeMap<String, String>>,
}

/// game_restart arguments.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct GameRestartArgs {
    /// Build before relaunching. Default true. A failed build leaves the running game as it was.
    pub build: Option<bool>,
}

/// logs_tail arguments.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct LogsTailArgs {
    /// Most lines to return. Default 100, at most 2000.
    pub n: Option<usize>,
    /// Regular expression over the line text.
    pub filter: Option<String>,
    /// The cursor from an earlier reply: returns the lines after it. A cursor belongs to one run; after a new run the reply has dropped true and the newest lines.
    pub since: Option<u64>,
}

/// What wait_until waits for, as offered over MCP. combat_end is not offered.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitFor {
    Idle,
    Dialog,
    AreaLoaded,
}

/// wait_until arguments.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WaitUntilToolArgs {
    /// What to wait for: idle, dialog or area_loaded.
    pub condition: WaitFor,
    /// Give up after this many game seconds. Default 1, most 30.
    pub timeout: Option<f32>,
}

/// move_to arguments: the object to walk in reach of. Walking to a position is not offered.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct MoveToToolArgs {
    /// Object id from observe: the leader walks in reach of it.
    pub object: u32,
    /// Run instead of walk. Default walk.
    pub run: Option<bool>,
    /// Give up after this many game seconds. Default 60.
    pub timeout: Option<f32>,
}

/// game_status arguments.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct StatusArgs {
    /// true adds the log filter, the socket path, the physical window sizes and each build record. Default false.
    pub verbose: Option<bool>,
}

/// observe arguments.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ObserveToolArgs {
    /// Radius in metres, from the party leader, for the nearby list. Default 12.
    pub radius: Option<f32>,
    /// true adds each party member's Force powers and each journal entry's text. Default false.
    pub verbose: Option<bool>,
}

#[derive(Clone)]
pub struct K2 {
    sup: Supervisor,
    tool_router: ToolRouter<Self>,
}

impl K2 {
    pub fn new(sup: Supervisor) -> Self {
        let mut tool_router = Self::tool_router();
        if !policy::debug_tools_enabled() {
            for name in DEBUG_TOOLS {
                tool_router.remove_route(name);
            }
        }
        Self { sup, tool_router }
    }

    /// Runs a blocking supervisor call off the async runtime.
    async fn blocking<F>(&self, f: F) -> Reply
    where
        F: FnOnce(Supervisor) -> Reply + Send + 'static,
    {
        let sup = self.sup.clone();
        tokio::task::spawn_blocking(move || f(sup)).await.unwrap_or_else(|e| Reply::err(format!("internal error: {e}")))
    }

    /// Sends a verb to the game.
    async fn call(&self, cmd: Command) -> CallToolResult {
        to_result(self.blocking(move |sup| sup.forward(cmd)).await)
    }
}

/// The reply as an MCP tool result. A call that succeeded gives its data as one
/// line of JSON (or its message, when it has no data), then the game's messages.
/// A failed call gives its message alone, marked as a tool error.
fn to_result(reply: Reply) -> CallToolResult {
    if !reply.ok {
        return CallToolResult::error(vec![ContentBlock::text(reply.message)]);
    }
    CallToolResult::success(vec![ContentBlock::text(render::plain(&reply))])
}

/// How long a key_press may take in wall-clock time: its hold of game time, at up
/// to ten times slower than real time, plus a minute. A speech hold is added by the
/// socket client when the game's interim frame arrives.
fn key_press_wait(hold_ms: Option<u32>) -> Duration {
    let hold_s = f64::from(hold_ms.unwrap_or(60).min(10_000)) / 1000.0;
    Duration::from_secs_f64(hold_s * 10.0 + 60.0)
}

/// key_press's reply: the game's account of the hold (with any reason for an early
/// release), the leader's position, and the game's messages.
fn key_press_result(reply: Reply) -> CallToolResult {
    if !reply.ok {
        return to_result(reply);
    }
    let position = reply.data.as_ref().and_then(|data| data.get("position")).map(|p| render::round_floats(p.clone()));
    let head = match position {
        Some(p) => format!("{} position={}", reply.message, serde_json::to_string(&p).unwrap_or_default()),
        None => reply.message.clone(),
    };
    CallToolResult::success(vec![ContentBlock::text(render::with_feedback(head, &reply))])
}

/// observe's reply: the compact text view (see render::observe), then the game's messages.
fn observe_result(reply: Reply, verbose: bool) -> CallToolResult {
    if !reply.ok {
        return to_result(reply);
    }
    let body = match reply.data.as_ref().and_then(|data| render::observe(data, verbose)) {
        Some(text) => render::with_feedback(render::waited(&reply, text), &reply),
        None => render::plain(&reply),
    };
    CallToolResult::success(vec![ContentBlock::text(body)])
}

#[tool_router]
impl K2 {
    #[tool(description = "Start the game in a window: build it (cargo build --release -p kotor2), snapshot the binary, launch it with the control socket and the run's own saves and settings, then wait until it answers. A red build is reported and nothing launches. The window size is reported once the game says it has settled.")]
    async fn game_start(&self, Parameters(args): Parameters<GameStartArgs>) -> CallToolResult {
        let reply = match Start::parse(args.start.as_deref().unwrap_or("new")) {
            Ok(start) => {
                let build = args.build.unwrap_or(true);
                let extra = args.args.unwrap_or_default();
                let env: Vec<(String, String)> = args.env.unwrap_or_default().into_iter().collect();
                self.blocking(move |sup| sup.start(start, build, extra, env)).await
            }
            Err(e) => Reply::err(e),
        };
        to_result(reply)
    }

    #[tool(description = "Stop the game: it saves and exits (a quit request, then SIGTERM, then a kill, each only if the game has not exited yet). Waits for a start or restart in progress.")]
    async fn game_stop(&self) -> CallToolResult {
        to_result(self.blocking(|sup| sup.stop()).await)
    }

    #[tool(description = "Restart the game: build (a red build keeps the old game running), save to mcp-restart, stop, and relaunch from that save with the extra flags of the last start. With no game running it saves nothing and starts the newest save (continue).")]
    async fn game_restart(&self, Parameters(args): Parameters<GameRestartArgs>) -> CallToolResult {
        let build = args.build.unwrap_or(true);
        to_result(self.blocking(move |sup| sup.restart(build)).await)
    }

    #[tool(description = "Whether the game runs, its pid, uptime, build, start, log file, what the game reports (phase: menu, movie, loading, dialog, minigame, paused or game; module, clock, window size once settled), and the last exit: stopped (game_stop), exited on its own (code 0), or crashed (with its last output). verbose=true adds the log filter, the socket path, the physical window sizes and the full build records.")]
    async fn game_status(&self, Parameters(args): Parameters<StatusArgs>) -> CallToolResult {
        let verbose = args.verbose.unwrap_or(false);
        let mut reply = self.blocking(|sup| sup.status()).await;
        if let Some(data) = reply.data.take() {
            reply.data = Some(render::status(&data, verbose));
        }
        to_result(reply)
    }

    #[tool(description = "Recent lines of the game's output (stdout and stderr), oldest first. Filter with a regex; page with the cursor from the previous reply. A cursor belongs to one run: after a new run the reply has dropped true and the newest lines.")]
    async fn logs_tail(&self, Parameters(args): Parameters<LogsTailArgs>) -> CallToolResult {
        let n = args.n.unwrap_or(100);
        to_result(self.blocking(move |sup| sup.logs_tail(n, args.filter.as_deref(), args.since)).await)
    }

    #[tool(description = "Change the game's console log filter while it runs (EnvFilter syntax, e.g. \"info,kotor2=debug,wgpu=error\"). The trace has its own filter, set by trace_start.")]
    async fn set_log_filter(&self, Parameters(args): Parameters<SetLogFilterArgs>) -> CallToolResult {
        self.call(Command::SetLogFilter(args)).await
    }

    #[tool(description = "Start writing a JSON trace of the game's spans and events to a file. path is a bare file name under the run's traces folder, or a path inside it; an existing file is refused. filter (EnvFilter syntax) sets what the trace records and does not change the console filter; default \"info,kotor2=trace,k2_world=trace\". Returns the file path.")]
    async fn trace_start(&self, Parameters(args): Parameters<TraceStartArgs>) -> CallToolResult {
        self.call(Command::TraceStart(args)).await
    }

    #[tool(description = "Stop the trace in progress and return its file path.")]
    async fn trace_stop(&self) -> CallToolResult {
        self.call(Command::TraceStop).await
    }

    #[tool(description = "Capture the game's rendered frame as a PNG. Returns the image, its path, and its width, height (physical pixels) and scale_factor: logical = physical / scale_factor. path is a bare file name under the run's shots folder, or a path inside it; an existing file is refused. The game must be rendering: keep its window on a visible workspace.")]
    async fn screenshot(&self, Parameters(args): Parameters<ScreenshotArgs>) -> CallToolResult {
        let reply = self.blocking(move |sup| sup.forward(Command::Screenshot(args))).await;
        let path = reply.data.as_ref().and_then(|d| d.get("path")).and_then(Value::as_str).map(str::to_owned);
        let mut result = to_result(reply.clone());
        if let (true, Some(path)) = (reply.ok, path) {
            // The image is read only from the run's shots folder, whatever path the game names.
            // The file is resolved first; that resolved path is the one checked and read.
            let shots = self.sup.shots_folder();
            let real = std::fs::canonicalize(&path).ok().filter(|real| shots.canonicalize().is_ok_and(|dir| real.starts_with(dir)));
            let Some(real) = real else {
                return CallToolResult::error(vec![ContentBlock::text(format!("the game reported {path}, which is not under {}; not read", shots.display()))]);
            };
            match std::fs::read(&real) {
                Ok(bytes) => result.content.push(ContentBlock::image(STANDARD.encode(bytes), "image/png")),
                Err(e) => result.content.push(ContentBlock::text(format!("cannot read {}: {e}", real.display()))),
            }
        }
        result
    }

    #[tool(description = "Press a key and hold it for hold_ms of game time (default 60), then release it. This is the way to walk: key_press KeyW with hold_ms 1000 walks forward for one game second, about 1.75 m. KeyS walks back. Shift (ShiftLeft, or a shift modifier) toggles running, about 5.4 m/s. Paused, movie, loading and conversation time does not count, and the call answers when the key is up, so it can take longer than hold_ms in wall-clock time. The reply gives the held time, the metres moved and the leader's position, or why the key was refused. A printable key (letter, digit, space) also types its character unless ctrl or super is held. Escape skips a playing movie. Modifiers: shift, ctrl, alt, super. Keys: KeyA, Digit1, Enter, Space, Escape, ArrowUp, F5, ShiftLeft, ...")]
    async fn key_press(&self, Parameters(args): Parameters<KeyPressArgs>) -> CallToolResult {
        let wait = key_press_wait(args.hold_ms);
        let reply = self.blocking(move |sup| sup.forward_with(Command::KeyPress(args), Some(wait))).await;
        key_press_result(reply)
    }

    #[tool(description = "Type text one character at a time, as a keyboard would (letters, digits, space, tab, newline). At most 200 characters.")]
    async fn type_text(&self, Parameters(args): Parameters<TypeTextArgs>) -> CallToolResult {
        self.call(Command::TypeText(args)).await
    }

    #[tool(description = "Move the mouse pointer to a window position in logical pixels (origin top left). Logical = physical screenshot pixel / scale_factor; game_status gives the window size in logical pixels and the scale_factor. A position must be finite and inside the window, else it is refused.")]
    async fn mouse_move(&self, Parameters(args): Parameters<MouseMoveArgs>) -> CallToolResult {
        self.call(Command::MouseMove(args)).await
    }

    #[tool(description = "Move the pointer to a logical window position and click (left by default; clicks 2 for a double click). Positions are logical pixels, as for mouse_move, and must be inside the window.")]
    async fn mouse_click(&self, Parameters(args): Parameters<MouseClickArgs>) -> CallToolResult {
        self.call(Command::MouseClick(args)).await
    }

    #[tool(description = "Press a button at one logical window position, move to another in steps, and release. Positions are logical pixels, as for mouse_move, and must be inside the window.")]
    async fn mouse_drag(&self, Parameters(args): Parameters<MouseDragArgs>) -> CallToolResult {
        self.call(Command::MouseDrag(args)).await
    }

    #[tool(description = "Mouse wheel: lines_y positive scrolls up. Optionally moves to a logical window position first (logical pixels, as for mouse_move, inside the window).")]
    async fn scroll(&self, Parameters(args): Parameters<ScrollArgs>) -> CallToolResult {
        self.call(Command::Scroll(args)).await
    }

    #[tool(description = "Move the pointer to a logical window position and report the object under it: object, name and kind (object is null when nothing is under the pointer). Positions are logical pixels, as for mouse_move, and must be inside the window. Use it to see what a click would hit before clicking.")]
    async fn pick(&self, Parameters(args): Parameters<PickArgs>) -> CallToolResult {
        self.call(Command::Pick(args)).await
    }

    #[tool(description = "Where an object is on the window: x and y in logical pixels, and on_screen. x and y are null when the object is behind the camera. Takes an object id from observe.")]
    async fn project(&self, Parameters(args): Parameters<ObjectArgs>) -> CallToolResult {
        self.call(Command::Project(args)).await
    }

    #[tool(description = "Observe the game, as compact lines: a header (module, area, time, leader, paused), one line per party member (id, name, level, HP, flags), then the interactable objects nearby, nearest first, each as `id name (kind, flags) - distance and direction from the leader`: doors that can be used (open or closed), placeables that can be used or talked to, creatures that are alive or talk, items on the ground, trapped triggers. Scenery and helpers are left out. Directions are relative to where the leader faces (in front of you, to your left, behind you and to your right, and so on); above or below is named when the height differs by more than 1.5 m. Then the conversation with numbered replies, combat, journal, inventory and level-ups, each only when set. Default radius 12 m. When a window, screen or menu is open, the first line names it (open: ...). verbose=true adds raw positions, the powers and the journal text.")]
    async fn observe(&self, Parameters(args): Parameters<ObserveToolArgs>) -> CallToolResult {
        let verbose = args.verbose.unwrap_or(false);
        let cmd = Command::Observe(ObserveArgs { radius: Some(args.radius.unwrap_or(OBSERVE_RADIUS)) });
        observe_result(self.blocking(move |sup| sup.forward(cmd)).await, verbose)
    }

    #[tool(description = "Details of one object by id: a creature's sheet, skills, feats and powers; an item's description and value; a door's or placeable's state.")]
    async fn inspect(&self, Parameters(args): Parameters<ObjectArgs>) -> CallToolResult {
        self.call(Command::Inspect(args)).await
    }

    #[tool(description = "Every journal entry, completed ones included.")]
    async fn journal(&self) -> CallToolResult {
        self.call(Command::Journal).await
    }

    #[tool(description = "Save slots in the save folder, with module, area, play time and party.")]
    async fn list_saves(&self) -> CallToolResult {
        self.call(Command::ListSaves).await
    }

    #[tool(description = "Cancel the running move_to, interact, talk_to, transition or wait_until (or an input job), cancel a queued module change, and clear the leader's queued actions.")]
    async fn stop(&self) -> CallToolResult {
        self.call(Command::Stop).await
    }

    #[tool(description = "Walk the party leader in reach of an object id. It stops within the object's reach, which is 1.8 m plus the object's radius: doors 1.2 (3.0 m in all), placeables 0.8 (2.6 m), creatures max(radius, 0.4) (2.2 m at least), other objects 0.5 (2.3 m). Answers on arrival, on timeout, or when the way is blocked.")]
    async fn move_to(&self, Parameters(args): Parameters<MoveToToolArgs>) -> CallToolResult {
        self.call(Command::MoveTo(MoveToArgs { point: None, object: Some(args.object), run: args.run, timeout: args.timeout })).await
    }

    #[tool(description = "Use a door, container or placeable, walking there first. A static door, or a placeable that cannot be used, is refused ('cannot be used'). A locked door answers 'is locked': try use_skill unlock. Opening a container lists its items; then take_item or take_all.")]
    async fn interact(&self, Parameters(args): Parameters<ObjectArgs>) -> CallToolResult {
        self.call(Command::Interact(args)).await
    }

    #[tool(description = "Start a conversation with a non-hostile creature, walking there first. Then read the conversation in observe and use choose_reply or continue_dialog.")]
    async fn talk_to(&self, Parameters(args): Parameters<ObjectArgs>) -> CallToolResult {
        self.call(Command::TalkTo(args)).await
    }

    #[tool(description = "Pick a numbered reply of the running conversation (the numbers are in observe's conversation.replies). Answers once the NPC's next line is up with its replies, or the conversation has ended; the answer carries the conversation as it stands then.")]
    async fn choose_reply(&self, Parameters(args): Parameters<ChooseReplyArgs>) -> CallToolResult {
        self.call(Command::ChooseReply(args)).await
    }

    #[tool(description = "Continue past a line that offers only [Continue]. Refused when real replies are offered. Answers once the next line is up, or the conversation has ended.")]
    async fn continue_dialog(&self) -> CallToolResult {
        self.call(Command::ContinueDialog).await
    }

    #[tool(description = "Attack a hostile creature, or bash a locked door or container. Answers at once; use observe to see the outcome.")]
    async fn attack(&self, Parameters(args): Parameters<ObjectArgs>) -> CallToolResult {
        self.call(Command::Attack(args)).await
    }

    #[tool(description = "Cast a known Force power (its id from a party member's powers) at a target. Refused when the power cannot be used on that target.")]
    async fn use_power(&self, Parameters(args): Parameters<UsePowerArgs>) -> CallToolResult {
        self.call(Command::UsePower(args)).await
    }

    #[tool(description = "Use an inventory item, such as a medpac or stim, on a target (default: the party leader).")]
    async fn use_item(&self, Parameters(args): Parameters<UseItemArgs>) -> CallToolResult {
        self.call(Command::UseItem(args)).await
    }

    #[tool(description = "Skill checks: unlock a locked door, container or placeable with Security (or a key), or disarm a mine with Demolitions.")]
    async fn use_skill(&self, Parameters(args): Parameters<UseSkillArgs>) -> CallToolResult {
        self.call(Command::UseSkill(args)).await
    }

    #[tool(description = "Equip an inventory item of the party leader, in the given slot or the first slot it fits.")]
    async fn equip(&self, Parameters(args): Parameters<EquipArgs>) -> CallToolResult {
        self.call(Command::Equip(args)).await
    }

    #[tool(description = "Empty an equipment slot of the party leader back into the inventory.")]
    async fn unequip(&self, Parameters(args): Parameters<UnequipArgs>) -> CallToolResult {
        self.call(Command::Unequip(args)).await
    }

    #[tool(description = "Take one item out of the open container (interact with it first).")]
    async fn take_item(&self, Parameters(args): Parameters<TakeItemArgs>) -> CallToolResult {
        self.call(Command::TakeItem(args)).await
    }

    #[tool(description = "Take everything out of the open container with this object id.")]
    async fn take_all(&self, Parameters(args): Parameters<ObjectArgs>) -> CallToolResult {
        self.call(Command::TakeAll(args)).await
    }

    #[tool(description = "Give control to a living party member in the field (its object id).")]
    async fn switch_leader(&self, Parameters(args): Parameters<ObjectArgs>) -> CallToolResult {
        self.call(Command::SwitchLeader(args)).await
    }

    #[tool(description = "Set the followers by NPC slot (K2 slots: 0 Atton, 1 Bao-Dur, 2 Mandalore, 3 G0-T0, 4 Handmaiden, 5 HK-47, 6 Kreia, 7 Mira, 8 T3-M4, 9 Visas, 10 Hanharr, 11 Disciple). At most two.")]
    async fn set_party(&self, Parameters(args): Parameters<SetPartyArgs>) -> CallToolResult {
        self.call(Command::SetParty(args)).await
    }

    #[tool(description = "Apply one level up to a party member once it has the experience: automatic (default without picks) or explicit attribute, skills, feats and powers.")]
    async fn level_up(&self, Parameters(args): Parameters<LevelUpArgs>) -> CallToolResult {
        self.call(Command::LevelUp(args)).await
    }

    #[tool(description = "Leave the area: walk through an exit trigger, or walk to a door that leads elsewhere and use it. Answers once the module has changed (or on failure).")]
    async fn transition(&self, Parameters(args): Parameters<ObjectArgs>) -> CallToolResult {
        self.call(Command::Transition(args)).await
    }

    #[tool(description = "Wait for a condition: idle (you have control, nothing is queued, and the party has been settled for 0.5 game seconds with no pending script resume, transition, movie or queued action), dialog (a conversation is running), or area_loaded (the module changed). A dialog wait answers at once while a conversation runs. Refuses at once, with the reason, if nothing in progress could make the condition hold. Otherwise waits for timeout game seconds: 1 by default, at most 30.")]
    async fn wait_until(&self, Parameters(args): Parameters<WaitUntilToolArgs>) -> CallToolResult {
        let condition = match args.condition {
            WaitFor::Idle => Condition::Idle,
            WaitFor::Dialog => Condition::Dialog,
            WaitFor::AreaLoaded => Condition::AreaLoaded,
        };
        self.call(Command::WaitUntil(WaitUntilArgs { condition, timeout: args.timeout })).await
    }

    #[tool(description = "Save the game into a slot named mcp-<name> (letters, digits, '-' or '_').")]
    async fn save(&self, Parameters(args): Parameters<SaveArgs>) -> CallToolResult {
        self.call(Command::Save(args)).await
    }

    #[tool(description = "Load a save slot: the game loads it on its next frame; observe to confirm.")]
    async fn load(&self, Parameters(args): Parameters<LoadArgs>) -> CallToolResult {
        self.call(Command::Load(args)).await
    }

    #[tool(description = "DEBUG, not human-equivalent: move an object (default the party leader) to a point, skipping the walk.")]
    async fn debug_teleport(&self, Parameters(args): Parameters<DebugTeleportArgs>) -> CallToolResult {
        self.call(Command::DebugTeleport(args)).await
    }

    #[tool(description = "DEBUG, not human-equivalent: set a module global (number, boolean or string) without running scripts.")]
    async fn debug_set_global(&self, Parameters(args): Parameters<DebugSetGlobalArgs>) -> CallToolResult {
        self.call(Command::DebugSetGlobal(args)).await
    }

    #[tool(description = "DEBUG, not human-equivalent: create an item from a blueprint in a creature's inventory (default the party leader).")]
    async fn debug_give_item(&self, Parameters(args): Parameters<DebugGiveItemArgs>) -> CallToolResult {
        self.call(Command::DebugGiveItem(args)).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for K2 {
    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().enable_resources().build())
            .with_server_info(Implementation::new("k2-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_resources(&self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new(STATE_URI, "state").with_description("The observation: party, nearby objects, conversation, combat, journal, inventory.").with_mime_type("application/json"),
            Resource::new(JOURNAL_URI, "journal").with_description("Every journal entry.").with_mime_type("application/json"),
        ]))
    }

    async fn read_resource(&self, request: ReadResourceRequestParams, _context: RequestContext<RoleServer>) -> Result<ReadResourceResponse, ErrorData> {
        let cmd = match request.uri.as_str() {
            STATE_URI => Command::Observe(ObserveArgs { radius: Some(OBSERVE_RADIUS) }),
            JOURNAL_URI => Command::Journal,
            other => return Err(ErrorData::resource_not_found(format!("no resource {other}"), None)),
        };
        let reply = self.blocking(move |sup| sup.forward(cmd)).await;
        let body = if reply.ok { serde_json::to_string(&render::round_floats(reply.data.unwrap_or(Value::Null))).unwrap_or_default() } else { reply.message };
        Ok(ReadResourceResult::new(vec![ResourceContents::text(body, request.uri)]).into())
    }
}
