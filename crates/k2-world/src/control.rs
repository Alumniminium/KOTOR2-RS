//! Remote control API: one serde command set over [`World`], used by the MCP
//! server (crates/k2-mcp, which drives the windowed game through its socket)
//! and by the game's control socket (crates/kotor2 remote.rs). It is the single
//! source of truth for what an agent can see and do.
//!
//! Player verbs take the same World paths a player takes: action queues
//! (MoveTo, Attack, Cast), [`World::use_object`], dialog, inventory, level up.
//! Only the `debug_*` verbs write state directly.
//!
//! Verbs that take game time (move_to, interact, talk_to, take_item, take_all,
//! use_skill, transition, wait_until) start a goal. Object verbs walk the leader into use
//! reach first, as a click does, and act once it stands there. [`Control::submit`] returns None while it runs,
//! and the caller polls [`Control::poll`] once per world step until it answers.
//! One goal runs at a time: other verbs are refused until it finishes (or
//! `stop` cancels it). Read-only verbs always answer, and so do a few host
//! verbs that leave the player alone (see [`Command::allowed_during_goal`]).
//!
//! Host verbs (input, screenshot, logs, trace, info, pick, project, quit) are
//! answered by the process that owns the window, not by the World.
//!
//! Coordinates are game space (Z-up, metres). Object ids are stable for the
//! life of the object; ids seen in an observation stay valid until the object
//! is destroyed or its module is left.

pub mod socket;

use std::path::{Path, PathBuf};

use glam::Vec3;
use k2_script::ObjectId;
use k2_walk::SpotSearch;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, info_span};

use crate::inventory::ItemError;
use crate::nav::UseStep;
use crate::object::{Detail, ObjectKind, Target};
use crate::powers::TALENT_SPELL;
use crate::progression::LevelUpChoice;
use crate::save;
use crate::{World, WorldEvent};

/// Limit for walk and use verbs that do not name one (game seconds).
const DEFAULT_TIMEOUT: f32 = 60.0;
/// How long wait_until waits when the call names no timeout (game seconds).
const WAIT_DEFAULT: f32 = 1.0;
/// The longest wait_until waits: a wait always has a limit.
const WAIT_CAP: f32 = 30.0;
/// Upper bound for any timeout (game seconds).
const MAX_TIMEOUT: f32 = 3600.0;
/// Reach beyond the object's own radius (the client's use check uses it too).
const REACH: f32 = 1.8;
/// A walk to a point is over within this distance of it (metres).
const POINT_ARRIVED: f32 = 0.5;
/// A walk to a point that stopped short still counts as arrived within this
/// distance: the verdict's tolerance (metres).
const POINT_TOLERANCE: f32 = 1.0;
/// Seconds without progress before a walk gives up.
const WALK_STUCK: f32 = 1.5;
/// A walk is nearer its aim by this much (metres) or it makes no progress.
const PROGRESS: f32 = 0.02;
/// A stand is searched for this far inside use reach. A walk that stops within
/// this distance of its stand is in reach, so the walk's arrival is in_use_reach.
const STAND_CLEARANCE: f32 = 0.5;
/// The engine's message when the leader cannot get within use reach.
const UNREACHABLE: &str = "Unable to reach target!";
/// Seconds the leader has to be in an exit trigger without a change of module
/// before the exit verb gives up.
const EXIT_GRACE: f32 = 2.0;
/// Message log lines kept between replies.
const MESSAGE_CAP: usize = 200;
/// Nearby objects listed by observe.
const NEARBY_CAP: usize = 150;
/// Default observe radius (metres).
const NEARBY_RADIUS: f32 = 12.0;
/// Save slots written through the control API carry this prefix, so an
/// agent can never overwrite the player's own slots.
pub const SLOT_PREFIX: &str = "mcp-";
/// Game seconds the party must stay settled before `wait_until idle` answers.
const IDLE_HOLD: f64 = 0.5;

// ---------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------

/// One request. On the wire: a JSON object with the verb in "cmd", e.g.
/// `{"cmd":"move_to","point":[1.0,2.0,0.0]}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Party, nearby objects, dialog, combat, journal, inventory, feedback.
    Observe(ObserveArgs),
    /// Details of one object: character sheet, item, door or placeable state.
    Inspect(ObjectArgs),
    /// Every journal entry, completed ones included.
    Journal,
    /// Save slots under the save directory.
    ListSaves,
    /// Cancels a running goal and clears the leader's action queue.
    Stop,
    /// Walks the party leader to a point or to an object.
    MoveTo(MoveToArgs),
    /// Uses a door, container or placeable, or walks to it first.
    Interact(ObjectArgs),
    /// Starts a conversation with a non-hostile creature (walks to it first).
    TalkTo(ObjectArgs),
    /// Picks one of the offered replies by its index.
    ChooseReply(ChooseReplyArgs),
    /// Advances past a line that offers only [Continue].
    ContinueDialog,
    /// Ends the running conversation.
    EndDialog(EndDialogArgs),
    /// Attacks a hostile creature or bashes a locked door or container.
    Attack(ObjectArgs),
    /// Casts a known Force power at a target (must be usable on it).
    UsePower(UsePowerArgs),
    /// Uses an inventory item (medpac, stim, ...) on a target.
    UseItem(UseItemArgs),
    /// Walks to the target, then unlocks with the Security skill or disables a mine.
    UseSkill(UseSkillArgs),
    /// Equips an inventory item, in the given slot or the first that fits.
    Equip(EquipArgs),
    /// Empties an equipment slot back into the inventory.
    Unequip(UnequipArgs),
    /// Walks to a container, opens it if needed, and takes one item out.
    TakeItem(TakeItemArgs),
    /// Walks to a container, opens it if needed, and takes everything out.
    TakeAll(ObjectArgs),
    /// Gives control to a living party member in the field.
    SwitchLeader(ObjectArgs),
    /// Sets the followers by NPC slot (the party selection screen).
    SetParty(SetPartyArgs),
    /// Applies one level up, automatic or with explicit picks.
    LevelUp(LevelUpArgs),
    /// Leaves the area through a door or exit trigger.
    Transition(ObjectArgs),
    /// Waits until the party is idle, or a conversation runs, or combat ends,
    /// or the area changes.
    WaitUntil(WaitUntilArgs),
    /// Saves the game into an mcp- slot.
    Save(SaveArgs),
    /// Loads a save slot. The game answers it (it replaces the running world).
    Load(LoadArgs),
    /// DEBUG: moves an object (default the party leader) to a point. Skips the walk.
    DebugTeleport(DebugTeleportArgs),
    /// DEBUG: sets a module global. Skips scripts.
    DebugSetGlobal(DebugSetGlobalArgs),
    /// DEBUG: creates an item from a blueprint in a creature's inventory.
    DebugGiveItem(DebugGiveItemArgs),
    /// Host verb: a key press (down, hold, up) through the game's input path.
    KeyPress(KeyPressArgs),
    /// Host verb: holds a key down until key_up.
    KeyDown(KeyDownArgs),
    /// Host verb: releases a key held by key_down.
    KeyUp(KeyUpArgs),
    /// Host verb: types text, one character at a time.
    TypeText(TypeTextArgs),
    /// Host verb: moves the mouse pointer to a window position.
    MouseMove(MouseMoveArgs),
    /// Host verb: moves to a position and clicks a mouse button.
    MouseClick(MouseClickArgs),
    /// Host verb: presses a button at one position and releases it at another.
    MouseDrag(MouseDragArgs),
    /// Host verb: mouse wheel scroll, optionally at a position.
    Scroll(ScrollArgs),
    /// Host verb: captures the game's rendered frame to a PNG file.
    Screenshot(ScreenshotArgs),
    /// Host verb: changes the console log filter (EnvFilter syntax) while the game runs.
    SetLogFilter(SetLogFilterArgs),
    /// Host verb: starts writing tracing spans and events to a JSON file.
    TraceStart(TraceStartArgs),
    /// Host verb: stops the trace file and returns its path.
    TraceStop,
    /// Host verb: what the game is doing: phase, state, module, clock, window, filters.
    Info,
    /// Host verb: moves the pointer to a window position and reports the object under it.
    Pick(PickArgs),
    /// Host verb: where an object is on the window, in logical pixels.
    Project(ObjectArgs),
    /// Host verb: exits the game cleanly (the normal exit save runs).
    Quit,
}

impl Command {
    /// The verb's name on the wire (the "cmd" field), for logs and messages.
    pub fn verb(&self) -> &'static str {
        match self {
            Command::Observe(_) => "observe",
            Command::Inspect(_) => "inspect",
            Command::Journal => "journal",
            Command::ListSaves => "list_saves",
            Command::Stop => "stop",
            Command::MoveTo(_) => "move_to",
            Command::Interact(_) => "interact",
            Command::TalkTo(_) => "talk_to",
            Command::ChooseReply(_) => "choose_reply",
            Command::ContinueDialog => "continue_dialog",
            Command::EndDialog(_) => "end_dialog",
            Command::Attack(_) => "attack",
            Command::UsePower(_) => "use_power",
            Command::UseItem(_) => "use_item",
            Command::UseSkill(_) => "use_skill",
            Command::Equip(_) => "equip",
            Command::Unequip(_) => "unequip",
            Command::TakeItem(_) => "take_item",
            Command::TakeAll(_) => "take_all",
            Command::SwitchLeader(_) => "switch_leader",
            Command::SetParty(_) => "set_party",
            Command::LevelUp(_) => "level_up",
            Command::Transition(_) => "transition",
            Command::WaitUntil(_) => "wait_until",
            Command::Save(_) => "save",
            Command::Load(_) => "load",
            Command::DebugTeleport(_) => "debug_teleport",
            Command::DebugSetGlobal(_) => "debug_set_global",
            Command::DebugGiveItem(_) => "debug_give_item",
            Command::KeyPress(_) => "key_press",
            Command::KeyDown(_) => "key_down",
            Command::KeyUp(_) => "key_up",
            Command::TypeText(_) => "type_text",
            Command::MouseMove(_) => "mouse_move",
            Command::MouseClick(_) => "mouse_click",
            Command::MouseDrag(_) => "mouse_drag",
            Command::Scroll(_) => "scroll",
            Command::Screenshot(_) => "screenshot",
            Command::SetLogFilter(_) => "set_log_filter",
            Command::TraceStart(_) => "trace_start",
            Command::TraceStop => "trace_stop",
            Command::Info => "info",
            Command::Pick(_) => "pick",
            Command::Project(_) => "project",
            Command::Quit => "quit",
        }
    }

    /// Commands that only read state.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Command::Observe(_) | Command::Inspect(_) | Command::Journal | Command::ListSaves | Command::Info | Command::Project(_))
    }

    /// Commands that answer while a goal runs: the read-only ones, stop, quit,
    /// and the host verbs that leave the player alone (screenshot, log filter,
    /// trace). Keys, pointer and pick would act on the player's input, so they wait.
    pub fn allowed_during_goal(&self) -> bool {
        self.is_read_only() || matches!(self, Command::Stop | Command::Quit | Command::Screenshot(_) | Command::SetLogFilter(_) | Command::TraceStart(_) | Command::TraceStop)
    }

    /// Verbs that send the leader somewhere or make it act on the world (walk, use,
    /// talk, take, skill, transition). A cutscene refuses them.
    pub fn walks_leader(&self) -> bool {
        matches!(
            self,
            Command::MoveTo(_) | Command::Interact(_) | Command::TalkTo(_) | Command::UseSkill(_) | Command::TakeItem(_) | Command::TakeAll(_) | Command::Transition(_)
        )
    }

    /// Host verbs act on the game process itself (input, screen, logs, exit).
    /// The control layer refuses them; the game answers them.
    pub fn is_host(&self) -> bool {
        matches!(
            self,
            Command::KeyPress(_)
                | Command::KeyDown(_)
                | Command::KeyUp(_)
                | Command::TypeText(_)
                | Command::MouseMove(_)
                | Command::MouseClick(_)
                | Command::MouseDrag(_)
                | Command::Scroll(_)
                | Command::Screenshot(_)
                | Command::SetLogFilter(_)
                | Command::TraceStart(_)
                | Command::TraceStop
                | Command::Info
                | Command::Pick(_)
                | Command::Project(_)
                | Command::Quit
        )
    }
}

/// Keys are named as in Bevy's KeyCode: "KeyW", "Digit1", "Enter", "Space",
/// "ArrowUp", "ShiftLeft", "F5"; single letters and digits also work.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KeyPressArgs {
    pub key: String,
    /// Held while the key is pressed: "shift", "ctrl", "alt", "super".
    pub modifiers: Option<Vec<String>>,
    /// Milliseconds between press and release. Default 60.
    pub hold_ms: Option<u32>,
}

/// Arguments of `key_down`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KeyDownArgs {
    pub key: String,
    /// Modifiers pressed first and held with the key.
    pub modifiers: Option<Vec<String>>,
}

/// Arguments of `key_up`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KeyUpArgs {
    pub key: String,
    /// Modifiers released after the key.
    pub modifiers: Option<Vec<String>>,
}

/// Arguments of `type_text`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TypeTextArgs {
    /// Letters, digits, space, tab and newline. At most 200 characters.
    pub text: String,
    /// Milliseconds between characters. Default 30.
    pub delay_ms: Option<u32>,
}

/// Arguments of `mouse_move`: window position in logical pixels, origin top left.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MouseMoveArgs {
    pub x: f32,
    pub y: f32,
}

/// Arguments of `pick`: window position in logical pixels, origin top left.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PickArgs {
    pub x: f32,
    pub y: f32,
}

/// A mouse button.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MouseButtonName {
    #[default]
    Left,
    Right,
    Middle,
}

/// Arguments of `mouse_click`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MouseClickArgs {
    pub x: f32,
    pub y: f32,
    /// Default left.
    pub button: Option<MouseButtonName>,
    /// Number of clicks in a row, 1 or 2. Default 1.
    pub clicks: Option<u32>,
    /// Milliseconds the button is held. Default 50.
    pub hold_ms: Option<u32>,
}

/// Arguments of `mouse_drag`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MouseDragArgs {
    /// Start position [x, y] in logical pixels.
    pub from: [f32; 2],
    /// End position [x, y] in logical pixels.
    pub to: [f32; 2],
    /// Default left.
    pub button: Option<MouseButtonName>,
    /// Pointer positions in between. Default 12.
    pub steps: Option<u32>,
}

/// Arguments of `scroll`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScrollArgs {
    /// Position to scroll at (moves the pointer there first). Default: where the pointer is.
    pub x: Option<f32>,
    pub y: Option<f32>,
    /// Vertical lines: positive scrolls up (content moves down).
    pub lines_y: f32,
    /// Horizontal lines: positive scrolls right.
    pub lines_x: Option<f32>,
}

/// Arguments of `screenshot`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ScreenshotArgs {
    /// PNG file name, or a path under the run's shots folder. Must not exist yet.
    /// Default: a new file under the shots folder.
    pub path: Option<String>,
}

/// Arguments of `set_log_filter`. It changes the console output only.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SetLogFilterArgs {
    /// EnvFilter directives, e.g. "info,kotor2=debug,wgpu=error".
    pub filter: String,
}

/// Arguments of `trace_start`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TraceStartArgs {
    /// JSON file name, or a path under the run's traces folder. Must not exist yet.
    /// Default: a new file under the traces folder.
    pub path: Option<String>,
    /// EnvFilter directives for the trace alone (the console filter is not
    /// changed). Default "info,kotor2=trace,k2_world=trace".
    pub filter: Option<String>,
}

/// Arguments of `observe`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ObserveArgs {
    /// Radius in metres, measured from the party leader, for the nearby list. Default 12.
    pub radius: Option<f32>,
}

/// An object id, as the single argument of most verbs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ObjectArgs {
    /// Object id from an observation.
    pub object: ObjectId,
}

/// Arguments of `move_to`. Give exactly one of point or object.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct MoveToArgs {
    /// Destination [x, y, z] in game coordinates (metres, Z-up).
    pub point: Option<[f32; 3]>,
    /// Destination object id: the leader stops in reach of it.
    pub object: Option<ObjectId>,
    /// Run instead of walk. Default walk.
    pub run: Option<bool>,
    /// Give up after this many game seconds. Default 60.
    pub timeout: Option<f32>,
}

/// Arguments of `choose_reply`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChooseReplyArgs {
    /// Index of the reply, as numbered in the observed conversation.
    pub index: usize,
}

/// Arguments of `end_dialog`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct EndDialogArgs {
    /// Ends it as aborted (runs the abort script). Default false.
    pub aborted: Option<bool>,
}

/// Arguments of `use_power`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UsePowerArgs {
    /// Power id (spells.2da row), from the party member's `powers` list.
    pub power: i32,
    /// Target object id. Default: the caster.
    pub target: Option<ObjectId>,
    /// Caster object id. Default: the party leader.
    pub caster: Option<ObjectId>,
}

/// Arguments of `use_item`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UseItemArgs {
    /// Inventory item id.
    pub item: ObjectId,
    /// Target object id. Default: the party leader.
    pub target: Option<ObjectId>,
}

/// Skill-driven actions of `use_skill`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillAction {
    /// Security check on a locked door, container or placeable (or a key).
    Unlock,
    /// Demolitions check to disable a mine.
    Disarm,
}

/// Arguments of `use_skill`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UseSkillArgs {
    /// What to attempt.
    pub skill: SkillAction,
    /// Object id of the door, container, placeable or mine.
    pub target: ObjectId,
}

/// Arguments of `equip`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EquipArgs {
    /// Inventory item id.
    pub item: ObjectId,
    /// Equipment slot (0 head, 1 armour, 4 right hand, 5 left hand, ...).
    /// Default: the first slot the item fits.
    pub slot: Option<i32>,
}

/// Arguments of `unequip`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UnequipArgs {
    /// Equipment slot to empty.
    pub slot: i32,
}

/// Arguments of `take_item`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TakeItemArgs {
    /// The open container.
    pub container: ObjectId,
    /// Item inside it.
    pub item: ObjectId,
}

/// Arguments of `set_party`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SetPartyArgs {
    /// NPC slots to bring along (K2 slots: 0 Atton, 1 Bao-Dur, 2 Mandalore,
    /// 3 G0-T0, 4 Handmaiden, 5 HK-47, 6 Kreia, 7 Mira, 8 T3-M4, 9 Visas,
    /// 10 Hanharr, 11 Disciple). At most two.
    pub npcs: Vec<i32>,
}

/// A skill and the ranks bought with it at this level up.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillRank {
    /// SKILL_* index (skills.2da row).
    pub skill: usize,
    /// Ranks to add.
    pub ranks: i32,
}

/// Arguments of `level_up`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct LevelUpArgs {
    /// Creature object id (the party member).
    pub creature: ObjectId,
    /// Automatic picks (recommended feats, powers, skills). Default true
    /// unless explicit picks are given.
    pub auto: Option<bool>,
    /// Attribute to raise (0 STR, 1 DEX, 2 CON, 3 INT, 4 WIS, 5 CHA), when
    /// the level grants one.
    pub attribute: Option<usize>,
    /// Skill ranks to buy.
    pub skills: Option<Vec<SkillRank>>,
    /// Feat ids to take.
    pub feats: Option<Vec<u16>>,
    /// Power ids to take.
    pub powers: Option<Vec<i32>>,
}

/// The conditions `wait_until` can wait for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    /// The party is in control and settled, for about half a second of game
    /// time: no conversation, no movie, no queued event, no module change in
    /// progress, leader alive. A party member's queued actions block it, except
    /// follow actions (a companion keeps a standing follow queued and still
    /// counts as idle). A pending script resume (DelayCommand) blocks it, except
    /// an area loop: an area script that has re-armed itself on this visit. A
    /// one-shot delay from an area script blocks like any other. See
    /// [`World::settled`] for the exact rule.
    Idle,
    /// A conversation is running.
    Dialog,
    /// No creature is in combat.
    CombatEnd,
    /// The module has changed since the wait began (a transition completed).
    AreaLoaded,
}

/// Arguments of `wait_until`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WaitUntilArgs {
    /// What to wait for.
    pub condition: Condition,
    /// Give up after this many game seconds. Default 1, most 30.
    pub timeout: Option<f32>,
}

/// Arguments of `save`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SaveArgs {
    /// Slot name: "mcp-" followed by letters, digits, '-' or '_' (at most 60 more).
    pub slot: String,
    /// Name shown in the save list. Default: the slot name.
    pub name: Option<String>,
}

/// Arguments of `load`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LoadArgs {
    /// Slot name from list_saves.
    pub slot: String,
}

/// Arguments of `debug_teleport`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DebugTeleportArgs {
    /// Object to move. Default: the party leader.
    pub object: Option<ObjectId>,
    /// Destination [x, y, z]; z snaps to the walkmesh.
    pub point: [f32; 3],
}

/// Value kind of a global.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GlobalKind {
    Number,
    Boolean,
    String,
}

/// Arguments of `debug_set_global`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DebugSetGlobalArgs {
    /// Global name, e.g. "K_TEST_FLAG".
    pub name: String,
    /// How to read `value`.
    pub kind: GlobalKind,
    /// The new value: an integer, "true"/"false", or text.
    pub value: String,
}

/// Arguments of `debug_give_item`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DebugGiveItemArgs {
    /// Item blueprint resref, e.g. "g_i_medpac001".
    pub template: String,
    /// Receiving creature. Default: the party leader.
    pub owner: Option<ObjectId>,
    /// Stack size. Default 1.
    pub stack: Option<i32>,
}

// ---------------------------------------------------------------------
// Replies and observations
// ---------------------------------------------------------------------

/// The answer to one command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Reply {
    pub ok: bool,
    /// One line: what happened, or why it did not.
    pub message: String,
    /// Message log lines since the previous reply, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feedback: Vec<String>,
    /// Verb-specific structured result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl Reply {
    pub fn ok(message: impl Into<String>) -> Self {
        Self { ok: true, message: message.into(), feedback: Vec::new(), data: None }
    }

    pub fn ok_with(message: impl Into<String>, data: Value) -> Self {
        Self { ok: true, message: message.into(), feedback: Vec::new(), data: Some(data) }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self { ok: false, message: message.into(), feedback: Vec::new(), data: None }
    }
}

/// A named object reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObjRef {
    pub id: ObjectId,
    pub name: String,
}

/// A numbered thing (power, feat): id and name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Named {
    pub id: i32,
    pub name: String,
}

/// An equipped item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Equipped {
    pub slot: i32,
    pub item: ObjectId,
    pub name: String,
}

/// A party member.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Member {
    pub id: ObjectId,
    pub tag: String,
    pub name: String,
    pub leader: bool,
    pub level: i32,
    pub xp: i32,
    pub next_level_xp: i32,
    pub hp: i32,
    pub max_hp: i32,
    pub fp: i32,
    pub max_fp: i32,
    pub dead: bool,
    pub in_combat: bool,
    pub position: [f32; 3],
    /// Facing in radians (0 = +X, counter-clockwise). Directions are measured from it.
    pub facing: f32,
    /// Active effects by name ("EffectAttackIncrease", ...).
    pub conditions: Vec<String>,
    pub equipment: Vec<Equipped>,
    /// Known Force powers.
    pub powers: Vec<Named>,
}

/// An object near the party leader.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Nearby {
    pub id: ObjectId,
    pub tag: String,
    pub name: String,
    /// creature, door, placeable, trigger or item.
    pub kind: String,
    pub position: [f32; 3],
    /// Horizontal distance from the leader, metres.
    pub distance: f32,
    /// Creatures: hostile to the party.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostile: Option<bool>,
    /// Creatures: dead (corpses can be looted).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dead: Option<bool>,
    /// Doors and placeables.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    /// Doors and placeables.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
    /// Can be used by interact, with the pointer's rules: living creatures,
    /// doors that are not static, placeables flagged useable.
    pub useable: bool,
    /// Runs a script when used (OnUsed): using it has effects beyond the object itself.
    pub scripted_use: bool,
    /// Has a conversation (talk_to).
    pub conversation: bool,
    /// An armed trap or set mine.
    pub trapped: bool,
    /// Triggers: the module an exit leads to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_to: Option<String>,
}

/// One line of a conversation the player can pick.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplyInfo {
    pub index: usize,
    /// "[Continue]" for a bare continue.
    pub text: String,
}

/// The running conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationInfo {
    pub dialog: String,
    pub speaker: ObjRef,
    pub text: String,
    pub replies: Vec<ReplyInfo>,
    /// A door's message box (see `ConversationView::boxed`): OK closes it.
    #[serde(default)]
    pub boxed: bool,
}

/// Combat state of the party leader.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Combat {
    pub in_combat: bool,
    /// Living hostile creatures in combat.
    pub engaged: Vec<ObjRef>,
}

/// A journal entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Quest {
    pub tag: String,
    pub name: String,
    pub text: String,
    pub state: i32,
    pub completed: bool,
}

/// An inventory item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub id: ObjectId,
    pub name: String,
    pub stack: i32,
    pub value: i32,
    pub plot: bool,
    /// baseitems equipableslots bit mask (bit n = slot n).
    pub equip_slots: u32,
}

/// A party member with an available level up.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LevelUp {
    pub creature: ObjectId,
    pub name: String,
    pub new_level: i32,
}

/// Everything an agent needs to decide its next command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Observation {
    pub module: String,
    pub area: String,
    /// Game time, seconds.
    pub time: f64,
    pub paused: bool,
    /// The party is down (no living member).
    pub game_over: bool,
    pub leader: Option<ObjectId>,
    pub party: Vec<Member>,
    pub nearby: Vec<Nearby>,
    pub conversation: Option<ConversationInfo>,
    pub combat: Combat,
    /// Journal entries not yet completed.
    pub journal: Vec<Quest>,
    pub credits: i32,
    pub inventory: Vec<Item>,
    pub level_ups: Vec<LevelUp>,
}

// ---------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------

/// What the caller adds beyond the World.
pub struct Env<'a> {
    /// Directory of save slots.
    pub save_root: &'a Path,
    /// The game is paused (Esc menu); observe reports it.
    pub paused: bool,
    /// A movie owns the screen (the presentation layer's state, not the World's).
    pub movie: bool,
}

/// The goal a long verb is running.
#[derive(Debug, Clone, PartialEq)]
enum Goal {
    /// wait_until: the condition holds. `from` is the module when it began.
    Until { cond: Condition, from: String },
    /// move_to: the leader walks there; `target` judges arrival.
    Move { target: Target, run: bool },
    /// interact / talk_to / take: the leader walks into reach, then acts once.
    Act { target: ObjectId, act: Act },
    /// transition: the module changes. Doors are used once in reach;
    /// exit triggers fire when the leader walks through them.
    Exit { target: ObjectId, from: String, used: bool },
}

/// What a walking verb does once the leader is in reach.
#[derive(Debug, Clone, PartialEq)]
enum Act {
    /// interact: use the object (or open the container).
    Interact,
    /// talk_to: talk to the creature.
    Talk,
    /// take / take_all: take one item from the container, or everything.
    Take(Option<ObjectId>),
    /// use_skill: the skill check on the target.
    Skill(SkillAction),
}

impl Goal {
    fn name(&self) -> &'static str {
        match self {
            Goal::Until { .. } => "wait_until",
            Goal::Move { .. } => "move_to",
            Goal::Act { act: Act::Talk, .. } => "talk_to",
            Goal::Act { act: Act::Interact, .. } => "interact",
            Goal::Act { act: Act::Take(_), .. } => "take",
            Goal::Act { act: Act::Skill(_), .. } => "use_skill",
            Goal::Exit { .. } => "transition",
        }
    }
}

/// Where a walking verb walks to.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Dest {
    /// Within use reach of an object: a stand is searched for as ClickWalk does.
    Object(ObjectId),
    /// A point, walked to within POINT_ARRIVED.
    Point(Vec3),
}

/// A walking verb's walk, kept between polls the way ClickWalk keeps its own.
#[derive(Debug, Clone, Default)]
struct Walk {
    /// The stand search toward an object, stepped a frame at a time.
    search: Option<SpotSearch>,
    /// The stand found for an object, walked to by steering.
    stand: Option<Vec3>,
    /// Seconds without progress along a steered walk (a running search does not count).
    stuck: f32,
    /// The aim the walk is steering to, and how near the leader has got to it.
    aim: Option<Vec3>,
    best: f32,
}

/// The in-flight goal of one control channel.
#[derive(Debug, Clone, Default)]
pub struct Control {
    goal: Option<Goal>,
    /// World time at which the goal gives up.
    deadline: f64,
    /// Game-second limit recorded for the timeout message.
    limit: f32,
    /// World time at which a wait_until condition first held in a row.
    held: Option<f64>,
    /// Answer for a goal that `stop` cancelled; the next poll returns it so
    /// whoever waits on the goal gets a reply.
    cancelled: Option<Reply>,
    /// The walk of a walking verb.
    walk: Walk,
    /// World time at the previous poll: the step of a walk is the time since.
    clock: f64,
}

impl Control {
    /// Whether a goal is running.
    pub fn busy(&self) -> bool {
        self.goal.is_some()
    }

    /// Name of the running goal, if any.
    pub fn goal_name(&self) -> Option<&'static str> {
        self.goal.as_ref().map(Goal::name)
    }

    /// Runs `cmd`. None means the command started a goal: call [`Control::poll`]
    /// after each step until it answers.
    pub fn submit(&mut self, w: &mut World, env: &Env, cmd: Command) -> Option<Reply> {
        let _span = info_span!("control_submit", verb = cmd.verb()).entered();
        debug!("control: {} received", cmd.verb());
        if let Some(refusal) = self.refusal(&cmd) {
            debug!("control: {} refused: {}", cmd.verb(), refusal.message);
            return Some(finish(w, refusal));
        }
        if cmd.walks_leader() && held(w, env) {
            debug!("control: {} refused: a cutscene is running", cmd.verb());
            return Some(finish(w, Reply::err("refused: a cutscene is running")));
        }
        let out = match cmd {
            Command::Observe(a) => {
                let radius = a.radius.unwrap_or(NEARBY_RADIUS).max(0.0);
                let obs = observe(w, env.paused, radius);
                let msg = match (&obs.conversation, obs.game_over) {
                    _ if obs.module.is_empty() => "no module loaded".to_owned(),
                    (_, true) => format!("{}: the party is down", obs.module),
                    (Some(c), _) => format!("{}: talking to {}", obs.module, c.speaker.name),
                    _ => format!("{} at {:.1} s", obs.module, obs.time),
                };
                Some(Reply::ok_with(msg, json_value(&obs)))
            }
            Command::Inspect(a) => Some(inspect(w, a.object)),
            Command::Journal => {
                let entries: Vec<Quest> = w.journal_view().into_iter().map(quest).collect();
                Some(Reply::ok_with(format!("{} journal entries", entries.len()), json!({ "entries": entries })))
            }
            Command::ListSaves => Some(list_saves(env.save_root)),
            Command::Stop => Some(self.stop(w)),
            Command::MoveTo(a) => self.move_to(w, env, a),
            Command::Interact(a) => self.use_or_talk(w, env, a.object, false),
            Command::TalkTo(a) => self.use_or_talk(w, env, a.object, true),
            Command::ChooseReply(a) => Some(choose_reply(w, a.index)),
            Command::ContinueDialog => Some(continue_dialog(w)),
            Command::EndDialog(a) => Some(end_dialog(w, a.aborted.unwrap_or(false))),
            Command::Attack(a) => Some(attack(w, a.object)),
            Command::UsePower(a) => Some(use_power(w, a)),
            Command::UseItem(a) => Some(use_item(w, a)),
            Command::UseSkill(a) => self.skill(w, env, a),
            Command::Equip(a) => Some(equip(w, a)),
            Command::Unequip(a) => Some(unequip(w, a.slot)),
            Command::TakeItem(a) => self.take(w, env, a.container, Some(a.item)),
            Command::TakeAll(a) => self.take(w, env, a.object, None),
            Command::SwitchLeader(a) => Some(switch_leader(w, a.object)),
            Command::SetParty(a) => Some(set_party(w, &a.npcs)),
            Command::LevelUp(a) => Some(level_up(w, a)),
            Command::Transition(a) => self.transition(w, env, a.object),
            Command::WaitUntil(a) => {
                let from = w.module.as_ref().map(|m| m.name.clone()).unwrap_or_default();
                let cond = a.condition;
                // Idle must hold for IDLE_HOLD before it answers, so its goal waits that out.
                if cond != Condition::Idle && condition_holds(w, cond, &from, env.movie) {
                    Some(Reply::ok(format!("condition met: {cond:?}")))
                } else if let Some(why) = wait_refusal(w, env, cond) {
                    Some(Reply::err(why))
                } else {
                    self.begin(w, env, Goal::Until { cond, from }, a.timeout.unwrap_or(WAIT_DEFAULT).min(WAIT_CAP))
                }
            }
            Command::Save(a) => Some(save_game(w, env.save_root, &a)),
            Command::Load(_) => Some(Reply::err("load is handled by the host process (the game)")),
            Command::DebugTeleport(a) => Some(debug_teleport(w, a)),
            Command::DebugSetGlobal(a) => Some(debug_set_global(w, a)),
            Command::DebugGiveItem(a) => Some(debug_give_item(w, a)),
            host => Some(Reply::err(format!("{} is a host verb: the game answers it, not the world", host.verb()))),
        };
        out.map(|r| finish(w, r))
    }

    /// The reply that refuses `cmd` because a goal is running, if any. Hosts
    /// that handle a verb themselves (save, load) call this first.
    pub fn refusal(&self, cmd: &Command) -> Option<Reply> {
        let name = self.goal_name()?;
        if cmd.allowed_during_goal() {
            return None;
        }
        Some(Reply::err(format!("busy: {name} is still running; observe, or stop")))
    }

    /// Advances the running goal. Some(reply) when it finished (or was cancelled
    /// by stop). Call after every world step, and once after [`Control::submit`]
    /// returned None.
    pub fn poll(&mut self, w: &mut World, env: &Env) -> Option<Reply> {
        if let Some(r) = self.cancelled.take() {
            return Some(finish(w, r));
        }
        let goal = self.goal.clone()?;
        let dt = (w.time - self.clock).max(0.0) as f32;
        self.clock = w.time;
        let out = {
            let _span = info_span!("control_goal", goal = goal.name()).entered();
            self.check(w, env, goal, dt)
        };
        if let Some(reply) = &out {
            debug!("control: {} finished: {}", self.goal_name().unwrap_or("goal"), reply.message);
            self.goal = None;
        }
        out.map(|r| finish(w, r))
    }

    fn check(&mut self, w: &mut World, env: &Env, goal: Goal, dt: f32) -> Option<Reply> {
        // A cutscene that starts during a walk stops the leader where it stands. An exit that
        // already took the leader to another module is done, whatever starts there.
        let left_module = matches!(&goal, Goal::Exit { from, .. } if w.module.as_ref().is_some_and(|m| m.name != *from));
        if !matches!(goal, Goal::Until { .. }) && !left_module && held(w, env) {
            self.end_walk(w);
            return Some(Reply::err("interrupted: a cutscene started"));
        }
        let leader = w.party_leader();
        let timed_out = w.time >= self.deadline;
        match goal {
            Goal::Until { cond, from } => {
                if condition_holds(w, cond, &from, env.movie) {
                    // A condition must hold for a while: the start of a wait
                    // is not enough, the party may be between two beats.
                    let since = *self.held.get_or_insert(w.time);
                    let hold = if cond == Condition::Idle { IDLE_HOLD } else { 0.0 };
                    if w.time - since >= hold {
                        return Some(Reply::ok(format!("condition met: {cond:?}")));
                    }
                } else {
                    self.held = None;
                }
                if timed_out {
                    Some(Reply::err(format!("timed out after {:.0} s waiting for {cond:?}", self.limit)))
                } else {
                    None
                }
            }
            Goal::Move { target, run } => {
                let (dest, what) = match target {
                    Target::Point(p) => (Dest::Point(p), format!("point {}", at(p))),
                    Target::Object(o) => (Dest::Object(o), format!("object {o}")),
                };
                match self.walk_frame(w, dest, run, dt) {
                    Ok(true) => {
                        let pos = leader_pos(w);
                        self.end_walk(w);
                        Some(Reply::ok_with(format!("arrived near {what}"), json!({ "position": fmt_pos(pos) })))
                    }
                    Ok(false) if timed_out => {
                        let stopped = at(leader_pos(w));
                        let state = walk_state(w, &self.walk);
                        self.end_walk(w);
                        Some(Reply::err(format!("timed out walking to {what}; stopped at {stopped} ({state})")))
                    }
                    Ok(false) => None,
                    Err(why) => {
                        self.end_walk(w);
                        Some(Reply::err(why))
                    }
                }
            }
            Goal::Act { target, act } => {
                if !w.is_valid(target) {
                    self.end_walk(w);
                    return Some(Reply::err(format!("object {target} is gone")));
                }
                match self.walk_frame(w, Dest::Object(target), false, dt) {
                    Ok(true) => {
                        self.end_walk(w);
                        Some(perform_act(w, leader, target, &act))
                    }
                    Ok(false) if timed_out => {
                        let state = walk_state(w, &self.walk);
                        self.end_walk(w);
                        Some(Reply::err(format!("timed out walking to {}; {state}", w.describe(target))))
                    }
                    Ok(false) => None,
                    Err(why) => {
                        self.end_walk(w);
                        Some(Reply::err(why))
                    }
                }
            }
            Goal::Exit { target, from, used } => {
                let module = w.module.as_ref().map(|m| m.name.clone()).unwrap_or_default();
                if module != from {
                    self.end_walk(w);
                    return Some(Reply::ok_with(format!("entered {module}"), json!({ "module": module })));
                }
                if !w.is_valid(target) {
                    self.end_walk(w);
                    return Some(Reply::err(format!("exit object {target} is gone")));
                }
                let is_door = w.get(target).is_some_and(|o| o.kind == ObjectKind::Door);
                if !used {
                    let dest = if is_door {
                        Dest::Object(target)
                    } else {
                        match trigger_centre(w, target, leader_pos(w).z) {
                            Some(c) => Dest::Point(c),
                            None => {
                                self.end_walk(w);
                                return Some(Reply::err(format!("{} is not an exit", w.describe(target))));
                            }
                        }
                    };
                    return match self.walk_frame(w, dest, false, dt) {
                        Ok(true) => {
                            self.end_walk(w);
                            if is_door {
                                if w.get(target).and_then(|o| o.door()).is_some_and(|d| d.locked) {
                                    return Some(Reply::err(format!("{} is locked", w.describe(target))));
                                }
                                w.use_object(leader, target);
                            }
                            self.goal = Some(Goal::Exit { target, from, used: true });
                            None
                        }
                        Ok(false) if timed_out => {
                            self.end_walk(w);
                            Some(Reply::err(format!("timed out before leaving {from}")))
                        }
                        Ok(false) => None,
                        Err(why) => {
                            self.end_walk(w);
                            Some(Reply::err(format!("did not leave {from}: {why}")))
                        }
                    };
                }
                // Used: a door's change, or a trigger the leader stands in, is on its way.
                if timed_out {
                    return Some(Reply::err(format!("timed out before leaving {from}")));
                }
                if !is_door {
                    self.walk.stuck += dt;
                    if self.walk.stuck > EXIT_GRACE {
                        self.end_walk(w);
                        return Some(Reply::err(format!("did not leave {from}: the exit did not fire")));
                    }
                }
                None
            }
        }
    }

    /// Starts `goal` with a limit in game seconds, then polls it once.
    fn begin(&mut self, w: &mut World, env: &Env, goal: Goal, limit: f32) -> Option<Reply> {
        let limit = limit.clamp(0.0, MAX_TIMEOUT);
        self.deadline = w.time + f64::from(limit);
        self.limit = limit;
        self.held = None;
        self.walk = Walk::default();
        w.forget_leader_path();
        self.clock = w.time;
        debug!("control: {} started (limit {limit:.0} s)", goal.name());
        self.goal = Some(goal);
        self.poll(w, env)
    }

    fn stop(&mut self, w: &mut World) -> Reply {
        let was = self.goal_name();
        if let Some(name) = was {
            self.cancelled = Some(Reply::err(format!("cancelled: {name} was stopped")));
        }
        self.goal = None;
        self.walk = Walk::default();
        w.forget_leader_path();
        clear_leader_actions(w, w.party_leader());
        // A module change queued by a door or trigger has not happened yet: stop cancels it.
        let queued = w.pending_transition.take();
        let mut msg = match was {
            Some(name) => format!("stopped {name}"),
            None => "stopped; nothing was running".to_owned(),
        };
        if let Some(t) = queued {
            msg.push_str(&format!("; cancelled the queued change to {}", t.module));
        }
        Reply::ok(msg)
    }

    /// One frame of a walk to `dest`. Ok(true) once the leader has arrived,
    /// Ok(false) while it walks or searches, Err when the walk cannot go on.
    /// An object is reached within use reach: a stand is searched for one step
    /// a frame, then the leader steers to it, as a click walk does.
    fn walk_frame(&mut self, w: &mut World, dest: Dest, run: bool, dt: f32) -> Result<bool, String> {
        let from = leader_pos(w);
        let goal = match dest {
            Dest::Point(p) if from.truncate().distance(p.truncate()) <= POINT_ARRIVED => return Ok(true),
            Dest::Point(p) => p,
            Dest::Object(target) => {
                if w.in_use_reach(from, target) {
                    return Ok(true);
                }
                // At the stand and out of reach: the object has moved, so a new stand is searched for.
                if self.walk.stand.is_some_and(|s| from.truncate().distance(s.truncate()) <= POINT_ARRIVED) {
                    self.walk.stand = None;
                }
                if let Some(stand) = self.walk.stand {
                    stand
                } else {
                    let mut search = match self.walk.search.take() {
                        Some(search) => search,
                        None => match w.begin_use_search_within(from, target, STAND_CLEARANCE) {
                            Some(search) => search,
                            None => return give_up(w, dest),
                        },
                    };
                    match w.step_use_search(&mut search) {
                        UseStep::Pending => self.walk.search = Some(search),
                        UseStep::Stand(at) => self.walk.stand = Some(at),
                        UseStep::NoStand => return give_up(w, dest),
                    }
                    return Ok(false);
                }
            }
        };
        let aim = match w.steer_leader(from, goal) {
            Some(aim) => aim,
            None => return give_up(w, dest),
        };
        // The route is still being searched: the leader stands still, which is not walking.
        if w.leader_route_pending() {
            return Ok(false);
        }
        // A closed door the leg runs into: once the leader is within reach it opens the door
        // (the use path; a door that talks says so and the walk ends there), then waits for the swing.
        if let Some(door) = w.door_across(from, aim)
            && w.in_use_reach(from, door)
        {
            let leader = w.party_leader();
            if w.get(door).and_then(|o| o.door()).is_some_and(|d| d.locked) {
                // A locked door stops the walk: the use gives the locked-door feedback and runs OnFailToOpen.
                w.use_object(leader, door);
                return Err(format!("{} is locked", w.describe(door)));
            }
            if !w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open) {
                w.use_object(leader, door);
                if w.conversation.is_some() {
                    return Err(format!("{} talks instead of opening", w.describe(door)));
                }
            }
            // Waits for the swing only: a door that is open and has swung is walked through.
            let (open, swinging) = w.get(door).and_then(|o| o.door()).map_or((false, false), |d| (d.open, d.opening > 0.0));
            if !open {
                return Err(format!("{} did not open", w.describe(door)));
            }
            if swinging {
                return Ok(false);
            }
        }
        w.walk_leader_toward(aim, dt, run);
        // Arrival is judged after the step too: a step that brings the leader into reach ends the walk.
        if let Dest::Object(target) = dest
            && w.in_use_reach(leader_pos(w), target)
        {
            return Ok(true);
        }
        if let Dest::Point(p) = dest
            && leader_pos(w).truncate().distance(p.truncate()) <= POINT_ARRIVED
        {
            return Ok(true);
        }
        // Progress is getting nearer the aim. A new aim is a new leg of the route, so it counts.
        let left = (aim - leader_pos(w)).truncate().length();
        if self.walk.aim != Some(aim) || left < self.walk.best - PROGRESS {
            self.walk.aim = Some(aim);
            self.walk.best = left;
            self.walk.stuck = 0.0;
        } else {
            self.walk.stuck += dt;
        }
        if self.walk.stuck > WALK_STUCK {
            return give_up(w, dest);
        }
        Ok(false)
    }

    /// Ends a walk: its state is dropped and the leader's cached route with it.
    fn end_walk(&mut self, w: &mut World) {
        self.walk = Walk::default();
        w.forget_leader_path();
    }

    /// take_item / take_all: the leader walks into reach of the container, then takes.
    fn take(&mut self, w: &mut World, env: &Env, container: ObjectId, item: Option<ObjectId>) -> Option<Reply> {
        if w.conversation.is_some() {
            return Some(Reply::err("in conversation: finish it first"));
        }
        if !w.is_valid(container) {
            return Some(Reply::err(format!("no object {container}")));
        }
        if !w.is_container(container) {
            return Some(Reply::err(format!("{} is not a container", w.describe(container))));
        }
        let leader = w.party_leader();
        if !w.in_use_reach(leader_pos(w), container) {
            clear_leader_actions(w, leader);
        }
        self.begin(w, env, Goal::Act { target: container, act: Act::Take(item) }, DEFAULT_TIMEOUT)
    }

    /// use_skill: the leader walks into reach of the target, then tries the skill.
    /// A key in the party's inventory has no place to walk to: it is used here.
    fn skill(&mut self, w: &mut World, env: &Env, a: UseSkillArgs) -> Option<Reply> {
        if !w.is_valid(a.target) {
            return Some(Reply::err(format!("no object {}", a.target)));
        }
        if w.get(a.target).is_some_and(|o| o.kind == ObjectKind::Item) {
            return Some(use_skill(w, a));
        }
        let leader = w.party_leader();
        if !w.in_use_reach(leader_pos(w), a.target) {
            clear_leader_actions(w, leader);
        }
        self.begin(w, env, Goal::Act { target: a.target, act: Act::Skill(a.skill) }, DEFAULT_TIMEOUT)
    }

    fn move_to(&mut self, w: &mut World, env: &Env, a: MoveToArgs) -> Option<Reply> {
        if w.conversation.is_some() {
            return Some(Reply::err("in conversation: finish it first"));
        }
        let leader = w.party_leader();
        if !w.is_valid(leader) {
            return Some(Reply::err("no party leader"));
        }
        let target = match (a.point, a.object) {
            (Some(p), None) => Target::Point(Vec3::from(p)),
            (None, Some(o)) if w.is_valid(o) => Target::Object(o),
            (None, Some(o)) => return Some(Reply::err(format!("no object {o}"))),
            _ => return Some(Reply::err("give exactly one of point or object")),
        };
        let timeout = a.timeout.unwrap_or(DEFAULT_TIMEOUT);
        clear_leader_actions(w, leader);
        self.begin(w, env, Goal::Move { target, run: a.run.unwrap_or(false) }, timeout)
    }

    fn use_or_talk(&mut self, w: &mut World, env: &Env, target: ObjectId, talk: bool) -> Option<Reply> {
        if w.conversation.is_some() {
            return Some(Reply::err("in conversation: finish it first"));
        }
        let leader = w.party_leader();
        let Some(o) = w.get(target) else { return Some(Reply::err(format!("no object {target}"))) };
        match o.kind {
            ObjectKind::Creature => {
                if target == leader {
                    return Some(Reply::err("that is the party leader"));
                }
                // A corpse with loot is a container: interact opens it.
                let dead = o.creature().is_some_and(|c| c.dead);
                if dead && (talk || !w.is_container(target)) {
                    return Some(Reply::err(format!("{} is dead", w.describe(target))));
                }
                if !dead && w.is_enemy(leader, target) {
                    return Some(Reply::err(format!("{} is hostile: use attack", w.describe(target))));
                }
            }
            // The pointer's rules (game.rs interactable): static doors and
            // placeables not flagged useable cannot be used.
            ObjectKind::Door if !talk => {
                if o.door().is_some_and(|d| d.is_static) {
                    return Some(Reply::err(format!("{} cannot be used", w.describe(target))));
                }
            }
            ObjectKind::Placeable if !talk => {
                if !o.placeable().is_some_and(|p| p.useable) {
                    return Some(Reply::err(format!("{} cannot be used", w.describe(target))));
                }
            }
            _ if talk => return Some(Reply::err(format!("talk_to needs a creature, not a {:?}", o.kind))),
            ObjectKind::Trigger => return Some(Reply::err("exit triggers are walked through: use transition")),
            _ => return Some(Reply::err(format!("cannot interact with a {:?}", o.kind))),
        }
        if !w.in_use_reach(leader_pos(w), target) {
            clear_leader_actions(w, leader);
        }
        let act = if talk { Act::Talk } else { Act::Interact };
        self.begin(w, env, Goal::Act { target, act }, DEFAULT_TIMEOUT)
    }

    fn transition(&mut self, w: &mut World, env: &Env, target: ObjectId) -> Option<Reply> {
        if w.conversation.is_some() {
            return Some(Reply::err("in conversation: finish it first"));
        }
        let leader = w.party_leader();
        let Some(o) = w.get(target) else { return Some(Reply::err(format!("no object {target}"))) };
        let from = w.module.as_ref().map(|m| m.name.clone()).unwrap_or_default();
        let exit = match (o.kind, o.door(), o.trigger()) {
            (ObjectKind::Door, Some(d), _) => d.linked_to_module.as_ref().is_some_and(|m| !m.is_empty()),
            (ObjectKind::Trigger, _, Some(t)) => !t.polygon.is_empty() && t.linked_to_module.as_ref().is_some_and(|m| !m.is_empty()),
            _ => false,
        };
        if !exit {
            return Some(Reply::err(format!("{} is not an exit", w.describe(target))));
        }
        clear_leader_actions(w, leader);
        self.begin(w, env, Goal::Exit { target, from, used: false }, DEFAULT_TIMEOUT)
    }
}

/// Attaches the message log to a reply.
fn finish(w: &mut World, mut reply: Reply) -> Reply {
    reply.feedback.extend(w.messages.drain(..));
    reply
}

// ---------------------------------------------------------------------
// Verbs
// ---------------------------------------------------------------------

fn perform_use(w: &mut World, leader: ObjectId, target: ObjectId, talk: bool) -> Reply {
    let name = w.name_of(target);
    if talk {
        w.talk_to(leader, target);
        return match w.conversation_view() {
            Some(_) => Reply::ok_with(format!("talking to {name}"), json!({ "conversation": conversation(w) })),
            None => Reply::err(format!("{name} has nothing to say")),
        };
    }
    let kind = w.get(target).map(|o| o.kind);
    let locked = w.get(target).is_some_and(|o| o.door().is_some_and(|d| d.locked) || o.placeable().is_some_and(|p| p.locked));
    if locked && kind != Some(ObjectKind::Creature) {
        w.use_object(leader, target);
        return Reply::err(format!("{name} is locked: use_skill unlock, or a key"));
    }
    w.use_object(leader, target);
    if w.inventory.open_container == Some(target) {
        let items: Vec<Value> = w.container_views(target).into_iter().map(|v| json!({ "id": v.id, "name": v.name, "stack": v.stack })).collect();
        return Reply::ok_with(format!("opened {name}: {} items", items.len()), json!({ "items": items }));
    }
    let open = w.get(target).and_then(|o| o.door().map(|d| d.open).or(o.placeable().map(|p| p.open)));
    let data = json!({ "open": open, "conversation": conversation(w) });
    Reply::ok_with(format!("used {name}"), data)
}

/// What a walking verb does once the leader is in reach.
fn perform_act(w: &mut World, leader: ObjectId, target: ObjectId, act: &Act) -> Reply {
    match act {
        Act::Interact => perform_use(w, leader, target, false),
        Act::Talk => perform_use(w, leader, target, true),
        Act::Take(item) => perform_take(w, leader, target, *item),
        Act::Skill(skill) => use_skill(w, UseSkillArgs { skill: *skill, target }),
    }
}

/// Takes from a container in reach. It is opened first when it is not open
/// (the use opens it); then one item, or everything, is taken.
fn perform_take(w: &mut World, leader: ObjectId, container: ObjectId, item: Option<ObjectId>) -> Reply {
    if w.inventory.open_container != Some(container) {
        let used = perform_use(w, leader, container, false);
        if w.inventory.open_container != Some(container) {
            let name = w.name_of(container);
            return if used.ok { Reply::err(format!("{name} did not open")) } else { used };
        }
    }
    match item {
        Some(item) => {
            let name = w.name_of(item);
            match w.take_item(container, item, leader) {
                Ok(_) => Reply::ok(format!("took {name}")),
                Err(e) => Reply::err(e.to_string()),
            }
        }
        None => {
            let taken = w.take_all(container, leader);
            Reply::ok(format!("took {} items", taken.len()))
        }
    }
}

fn choose_reply(w: &mut World, index: usize) -> Reply {
    let Some(view) = w.conversation_view() else { return Reply::err("no conversation running") };
    let Some(picked) = view.replies.get(index).cloned() else {
        return Reply::err(format!("no reply {index}; {} offered", view.replies.len()));
    };
    w.choose_reply(index);
    let msg = if picked.text.trim().is_empty() { "continued".to_owned() } else { format!("chose: {}", picked.text) };
    let msg = if w.conversation.is_none() { format!("{msg}; conversation ended") } else { msg };
    Reply::ok_with(msg, json!({ "conversation": conversation(w) }))
}

fn continue_dialog(w: &mut World) -> Reply {
    let Some(view) = w.conversation_view() else { return Reply::err("no conversation running") };
    let real = view.replies.len() > 1 || view.replies.iter().any(|r| !r.text.trim().is_empty());
    if real {
        return Reply::err(format!("{} replies offered: use choose_reply", view.replies.len()));
    }
    w.advance_conversation();
    let msg = if w.conversation.is_none() { "conversation ended" } else { "continued" };
    Reply::ok_with(msg, json!({ "conversation": conversation(w) }))
}

fn end_dialog(w: &mut World, aborted: bool) -> Reply {
    if w.conversation.is_none() {
        return Reply::err("no conversation running");
    }
    w.end_conversation(aborted);
    Reply::ok(if aborted { "conversation aborted" } else { "conversation ended" })
}

fn attack(w: &mut World, target: ObjectId) -> Reply {
    let leader = w.party_leader();
    let name = w.name_of(target);
    let Some(o) = w.get(target) else { return Reply::err(format!("no object {target}")) };
    if o.kind == ObjectKind::Creature {
        if o.creature().is_some_and(|c| c.dead) {
            return Reply::err(format!("{name} is dead"));
        }
        if !w.is_enemy(leader, target) {
            return Reply::err(format!("{name} is not hostile"));
        }
    } else if !w.can_bash(target) {
        return Reply::err(format!("{name} cannot be attacked"));
    }
    w.player_attack(target);
    Reply::ok(format!("attacking {name}"))
}

fn use_power(w: &mut World, a: UsePowerArgs) -> Reply {
    let caster = a.caster.unwrap_or_else(|| w.party_leader());
    let target = a.target.unwrap_or(caster);
    if !w.is_valid(caster) {
        return Reply::err(format!("no caster {caster}"));
    }
    let usable = w.usable_talents(caster, target);
    if !usable.iter().any(|t| t.ty == TALENT_SPELL && t.id == a.power) {
        let names: Vec<String> = usable.iter().filter(|t| t.ty == TALENT_SPELL).map(|t| format!("{} {}", t.id, t.name)).collect();
        return Reply::err(format!("power {} is not usable on {}; usable: [{}]", a.power, w.name_of(target), names.join(", ")));
    }
    w.use_talent(caster, TALENT_SPELL, a.power, target);
    Reply::ok(format!("{} casts power {} at {}", w.name_of(caster), a.power, w.name_of(target)))
}

fn use_item(w: &mut World, a: UseItemArgs) -> Reply {
    let user = w.party_leader();
    let target = a.target.unwrap_or(user);
    let name = w.name_of(a.item);
    match w.use_item(user, a.item, target) {
        Ok(spells) => Reply::ok_with(format!("used {name} on {}", w.name_of(target)), json!({ "spells": spells })),
        Err(e) => Reply::err(e),
    }
}

fn use_skill(w: &mut World, a: UseSkillArgs) -> Reply {
    let leader = w.party_leader();
    let name = w.name_of(a.target);
    match a.skill {
        SkillAction::Unlock => match w.try_unlock(leader, a.target) {
            Ok(()) => Reply::ok(format!("unlocked {name}")),
            Err(e) => Reply::err(format!("unlock {name} failed: {e}")),
        },
        SkillAction::Disarm => match w.disarm_trap(leader, a.target, false) {
            Ok(true) => Reply::ok(format!("disarmed {name}")),
            Ok(false) => Reply::err(format!("could not disarm {name}")),
            Err(e) => Reply::err(e),
        },
    }
}

fn equip(w: &mut World, a: EquipArgs) -> Reply {
    let leader = w.party_leader();
    let name = w.name_of(a.item);
    let slots: Vec<i32> = match a.slot {
        Some(s) => vec![s],
        None => {
            let mask = w.item_view(a.item).map_or(0, |v| v.equip_slots);
            (0..32).filter(|n| mask & (1 << n) != 0).collect()
        }
    };
    if slots.is_empty() {
        return Reply::err(format!("{name} does not fit any slot"));
    }
    let mut last = None;
    for slot in slots {
        match w.can_equip(leader, a.item, slot).and_then(|()| w.equip(leader, a.item, slot)) {
            Ok(()) => return Reply::ok(format!("equipped {name} in slot {slot}")),
            Err(e) => last = Some(e),
        }
    }
    Reply::err(last.map_or_else(|| format!("cannot equip {name}"), |e: ItemError| e.to_string()))
}

fn unequip(w: &mut World, slot: i32) -> Reply {
    let leader = w.party_leader();
    match w.unequip(leader, slot) {
        Some(item) => Reply::ok(format!("unequipped {} from slot {slot}", w.name_of(item))),
        None => Reply::err(format!("nothing in slot {slot}")),
    }
}

fn switch_leader(w: &mut World, creature: ObjectId) -> Reply {
    if w.set_party_leader(creature) {
        Reply::ok(format!("{} leads the party", w.name_of(creature)))
    } else {
        Reply::err(format!("{creature} cannot lead: not a living party member in the field, or in conversation"))
    }
}

fn set_party(w: &mut World, npcs: &[i32]) -> Reply {
    match w.apply_party_selection(npcs) {
        Ok(()) => Reply::ok(format!("party set: {} follower(s)", w.party.len().saturating_sub(1))),
        Err(e) => Reply::err(e),
    }
}

fn level_up(w: &mut World, a: LevelUpArgs) -> Reply {
    let Some(opts) = w.level_up_options(a.creature) else { return Reply::err(format!("{} is not a creature", a.creature)) };
    let explicit = a.attribute.is_some() || a.skills.is_some() || a.feats.is_some() || a.powers.is_some();
    let choice = if a.auto.unwrap_or(!explicit) {
        w.auto_choice(&opts)
    } else {
        let mut skills = [0; crate::craft::SKILL_COUNT];
        for s in a.skills.unwrap_or_default() {
            match skills.get_mut(s.skill) {
                Some(slot) => *slot += s.ranks,
                None => return Reply::err(format!("no skill {}", s.skill)),
            }
        }
        LevelUpChoice { attribute: a.attribute, skills, feats: a.feats.unwrap_or_default(), powers: a.powers.unwrap_or_default() }
    };
    let name = w.name_of(a.creature);
    match w.apply_level_up(a.creature, &choice) {
        Ok(()) => Reply::ok(format!("{name} reached level {}", w.level_of(a.creature))),
        Err(e) => Reply::err(e),
    }
}

fn save_game(w: &World, root: &Path, a: &SaveArgs) -> Reply {
    if let Err(e) = check_save_slot(&a.slot) {
        return Reply::err(e);
    }
    match write_save(w, root, &a.slot, a.name.as_deref().unwrap_or(&a.slot)) {
        Ok(_) => Reply::ok(format!("saved {}: {}", a.slot, leader_summary(w))),
        Err(e) => Reply::err(e),
    }
}

/// Writes a slot after the usual checks. Shared with the game's own save.
pub fn write_save(w: &World, root: &Path, slot: &str, name: &str) -> Result<PathBuf, String> {
    if let Some(why) = w.save_blocked() {
        return Err(format!("cannot save now: {why}"));
    }
    let data = w.save();
    let meta = w.save_meta(name);
    save::write_slot(root, slot, &data, &meta).map_err(|e| format!("cannot write {slot}: {e}"))
}

fn list_saves(root: &Path) -> Reply {
    let slots: Vec<Value> = save::list_slots(root)
        .into_iter()
        .map(|s| {
            json!({
                "slot": s.slot,
                "name": s.meta.name,
                "module": s.meta.module,
                "area": s.meta.area_name,
                "play_time": s.meta.play_time,
                "timestamp": s.meta.timestamp,
                "party": s.meta.party.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
                "incompatible": s.incompatible,
            })
        })
        .collect();
    Reply::ok_with(format!("{} save slots", slots.len()), json!({ "slots": slots, "root": root.display().to_string() }))
}

fn debug_teleport(w: &mut World, a: DebugTeleportArgs) -> Reply {
    let id = a.object.unwrap_or_else(|| w.party_leader());
    if !w.is_valid(id) {
        return Reply::err(format!("no object {id}"));
    }
    w.place(id, Vec3::from(a.point));
    let here = w.get(id).map(|o| at(o.position)).unwrap_or_default();
    Reply::ok(format!("DEBUG: moved {} to {here}", w.name_of(id)))
}

fn debug_set_global(w: &mut World, a: DebugSetGlobalArgs) -> Reply {
    match a.kind {
        GlobalKind::Number => match a.value.trim().parse::<i32>() {
            Ok(v) => w.globals.set_number(&a.name, v),
            Err(_) => return Reply::err("number globals need an integer value"),
        },
        GlobalKind::Boolean => match a.value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => w.globals.set_boolean(&a.name, true),
            "false" | "0" => w.globals.set_boolean(&a.name, false),
            _ => return Reply::err("boolean globals need true or false"),
        },
        GlobalKind::String => w.globals.set_string(&a.name, a.value.clone()),
    }
    Reply::ok(format!("DEBUG: set {} to {}", a.name, a.value))
}

fn debug_give_item(w: &mut World, a: DebugGiveItemArgs) -> Reply {
    let owner = a.owner.unwrap_or_else(|| w.party_leader());
    match w.give_new_item(&a.template, owner, a.stack.unwrap_or(1).max(1), false) {
        Some(item) => Reply::ok(format!("DEBUG: gave {} ({item}) to {}", a.template, w.name_of(owner))),
        None => Reply::err(format!("no item blueprint {}", a.template)),
    }
}

// ---------------------------------------------------------------------
// Goal helpers
// ---------------------------------------------------------------------

/// Work in progress that can still change what a wait waits for: a movie, a queued
/// transition or script event, a script resume that is not an area loop, a queued
/// action on a party member other than a follow, or input blocked for a moment.
fn progress_pending(w: &World, env: &Env) -> bool {
    env.movie
        || w.pending_transition.is_some()
        || !w.pending.is_empty()
        || w.input_blocked_for > 0.0
        || w.timers.iter().any(|t| matches!(&t.kind, crate::TimerKind::Resume(s) if !w.is_ambient(s)))
        || w.party.iter().any(|&id| w.get(id).is_some_and(|o| o.actions.iter().any(|a| !matches!(a, crate::object::Action::Follow { .. }))))
}

/// Why a wait_until cannot end: the game cannot move into the state it waits for.
/// None when something in progress could bring it about, so the wait may run.
fn wait_refusal(w: &World, env: &Env, cond: Condition) -> Option<String> {
    if env.paused {
        return Some(format!("refused: the game is paused, so nothing can make {cond:?} hold"));
    }
    match cond {
        Condition::Idle if w.conversation.is_some() => Some("refused: a conversation is running; choose a reply or continue it first".to_owned()),
        Condition::Idle if !progress_pending(w, env) && !w.settled() => Some("refused: the game is not idle, and nothing is pending that would make it idle".to_owned()),
        Condition::Dialog if !progress_pending(w, env) => Some("refused: nothing is pending that would start a conversation".to_owned()),
        Condition::AreaLoaded if !progress_pending(w, env) => Some("refused: nothing is pending that would change the module".to_owned()),
        _ => None,
    }
}

fn condition_holds(w: &World, cond: Condition, from: &str, movie: bool) -> bool {
    match cond {
        Condition::Idle => !movie && w.settled(),
        Condition::Dialog => w.conversation.is_some(),
        Condition::CombatEnd => !w.iter().any(|o| o.kind == ObjectKind::Creature && w.in_combat(o.id)),
        Condition::AreaLoaded => {
            w.pending_transition.is_none() && w.module.as_ref().is_some_and(|m| m.name != from)
        }
    }
}

/// Whether a cutscene holds the party for a walk: the world's (World::walk_held, a door's message box
/// excepted) or a movie the presentation is showing.
fn held(w: &World, env: &Env) -> bool {
    env.movie || w.walk_held()
}

fn leader_pos(w: &World) -> Vec3 {
    w.get(w.party_leader()).map_or(Vec3::ZERO, |o| o.position)
}

/// What a timed-out walk was doing: its stand, the aim it steers to and how near
/// it has got, whether its route is still being searched, and how long it has
/// made no progress.
fn walk_state(w: &World, walk: &Walk) -> String {
    let stand = walk.stand.map_or_else(|| "none".to_owned(), at);
    let aim = walk.aim.map_or_else(|| "none".to_owned(), at);
    let route = if w.leader_route_pending() { "searching" } else { "set" };
    format!("stand {stand}, aim {aim}, nearest {:.2} m to it, route {route}, no progress for {:.1} s", walk.best, walk.stuck)
}

/// The end of a walk that cannot go on. A point within POINT_TOLERANCE of the
/// leader counts as reached; any other point fails with where the leader
/// stopped, and an object fails with the engine's message.
fn give_up(w: &mut World, dest: Dest) -> Result<bool, String> {
    let pos = leader_pos(w);
    match dest {
        Dest::Point(p) if pos.truncate().distance(p.truncate()) <= POINT_TOLERANCE => Ok(true),
        Dest::Point(p) => Err(format!("could not reach point {}; stopped at {}", at(p), at(pos))),
        Dest::Object(_) => {
            w.report_unreachable();
            Err(UNREACHABLE.to_owned())
        }
    }
}

/// The centre of an exit trigger's polygon, at height `z`. None when the
/// object is not a trigger with a polygon.
fn trigger_centre(w: &World, target: ObjectId, z: f32) -> Option<Vec3> {
    let t = w.get(target)?.trigger()?;
    if t.polygon.is_empty() {
        return None;
    }
    let n = t.polygon.len() as f32;
    let (x, y) = t.polygon.iter().fold((0.0, 0.0), |(x, y), p| (x + p[0], y + p[1]));
    Some(Vec3::new(x / n, y / n, z))
}

fn clear_leader_actions(w: &mut World, leader: ObjectId) {
    if let Some(o) = w.get_mut(leader) {
        o.actions.clear();
        o.action_epoch += 1;
    }
}

/// Distance at which an object can be used from: REACH plus a radius by object
/// kind, the radii the client gives what the player can point at (kotor2
/// `interactable`): a living creature's body radius (at least 0.4; the leader
/// excepted, and a dead or downed one only when it is a container), a usable
/// door 1.2 (a static door only with a conversation), a usable placeable 0.8.
/// Everything else (static doors without one, unusable placeables, the leader)
/// gets 0.5.
pub(crate) fn reach_of(w: &World, id: ObjectId) -> f32 {
    let r = match w.get(id) {
        Some(o) if o.kind == ObjectKind::Creature => match o.creature() {
            Some(c) if id != w.party_leader() && (!(c.dead || c.down) || w.is_container(id)) => c.radius.max(0.4),
            _ => 0.5,
        },
        // A static door talks on click when it has a conversation (game.rs interactable), so it has the door radius too.
        Some(o) if o.kind == ObjectKind::Door && o.door().is_some_and(|d| !d.is_static || o.conversation.as_deref().is_some_and(|c| !c.is_empty())) => 1.2,
        Some(o) if o.kind == ObjectKind::Placeable && o.placeable().is_some_and(|p| p.useable) => 0.8,
        _ => 0.5,
    };
    REACH + r
}

/// Slots written by the control API: "mcp-" plus up to 60 safe characters.
pub fn check_save_slot(slot: &str) -> Result<(), String> {
    let rest = slot.strip_prefix(SLOT_PREFIX).ok_or_else(|| format!("save slots written by the control API start with {SLOT_PREFIX}"))?;
    if rest.is_empty() || rest.len() > 60 || !rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("bad slot name {slot:?}: {SLOT_PREFIX} then 1 to 60 letters, digits, '-' or '_'"));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Observation
// ---------------------------------------------------------------------

fn json_value<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn fmt_pos(p: Vec3) -> [f32; 3] {
    [p.x, p.y, p.z]
}

/// Position as text for messages.
fn at(p: Vec3) -> String {
    format!("({:.1}, {:.1}, {:.1})", p.x, p.y, p.z)
}

fn leader_summary(w: &World) -> String {
    let module = w.module.as_ref().map_or("no module", |m| m.name.as_str());
    match w.get(w.party_leader()) {
        Some(o) => format!("{module}, leader {} at ({:.1}, {:.1}, {:.1})", w.substitute(&o.name), o.position.x, o.position.y, o.position.z),
        None => format!("{module}, no leader"),
    }
}

fn quest(q: crate::sheet::QuestView) -> Quest {
    Quest { tag: q.tag, name: q.name, text: q.text, state: q.state, completed: q.completed }
}

fn conversation(w: &World) -> Option<ConversationInfo> {
    let v = w.conversation_view()?;
    let dialog = w.conversation.as_ref().map(|c| c.dialog.resref.clone()).unwrap_or_default();
    let boxed = v.boxed;
    Some(ConversationInfo {
        dialog,
        speaker: ObjRef { id: v.speaker, name: w.name_of(v.speaker) },
        text: v.text,
        replies: v
            .replies
            .into_iter()
            .enumerate()
            .map(|(index, r)| ReplyInfo { index, text: if r.text.trim().is_empty() { "[Continue]".to_owned() } else { r.text } })
            .collect(),
        boxed,
    })
}

/// Everything observe reports. `radius` bounds the nearby list (metres).
pub fn observe(w: &mut World, paused: bool, radius: f32) -> Observation {
    let mut out = Observation {
        module: w.module.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
        area: w.get(w.area_id).map(|a| a.name.clone()).unwrap_or_default(),
        time: w.time,
        paused,
        ..Default::default()
    };
    if w.module.is_none() {
        return out;
    }
    let leader = w.party_leader();
    out.leader = w.get(leader).map(|_| leader);
    let members: Vec<ObjectId> = w.party.iter().copied().filter(|&id| w.is_valid(id)).collect();
    out.game_over = !members.iter().any(|&id| w.is_standing(id));
    for &id in &members {
        out.party.push(member(w, id, id == leader));
        if w.level_up_available(id) {
            out.level_ups.push(LevelUp { creature: id, name: w.name_of(id), new_level: w.level_of(id) + 1 });
        }
    }
    out.nearby = nearby(w, leader, radius, &members);
    out.conversation = conversation(w);
    out.combat = combat(w, leader);
    out.journal = w.journal_view().into_iter().filter(|q| !q.completed).map(quest).collect();
    out.credits = w.credits();
    out.inventory = w
        .party_inventory_views()
        .into_iter()
        .map(|v| Item { id: v.id, name: v.name, stack: v.stack, value: v.value, plot: v.plot, equip_slots: v.equip_slots })
        .collect();
    out
}

fn member(w: &mut World, id: ObjectId, leader: bool) -> Member {
    let name = w.name_of(id);
    let (tag, pos) = w.get(id).map_or((String::new(), Vec3::ZERO), |o| (o.tag.clone(), o.position));
    let (hp, max_hp, fp, max_fp, dead, xp) = w.get(id).and_then(|o| o.creature()).map_or((0, 0, 0, 0, true, 0), |c| {
        (c.hp, c.max_hp, c.force, c.max_force, c.dead, c.xp)
    });
    let level = w.level_of(id);
    let equipment = w
        .equipment(id)
        .into_iter()
        .map(|(slot, item)| Equipped { slot, item, name: w.name_of(item) })
        .collect();
    let powers = w.power_views(id).into_iter().map(|p| Named { id: i32::from(p.id), name: p.name }).collect();
    let conditions = w.effect_views(id).into_iter().map(|(n, _)| n).collect();
    Member {
        id,
        tag,
        name,
        leader,
        level,
        xp,
        next_level_xp: w.xp_for_level(level + 1),
        hp,
        max_hp,
        fp,
        max_fp,
        dead,
        in_combat: w.in_combat(id),
        position: fmt_pos(pos),
        facing: w.get(id).map_or(0.0, |o| o.facing_deg().to_radians()),
        conditions,
        equipment,
        powers,
    }
}

fn nearby(w: &mut World, leader: ObjectId, radius: f32, members: &[ObjectId]) -> Vec<Nearby> {
    let Some(origin) = w.get(leader).map(|o| o.position) else { return Vec::new() };
    let mut candidates: Vec<(f32, ObjectId)> = w
        .iter()
        .filter(|o| !members.contains(&o.id))
        .filter(|o| match o.kind {
            ObjectKind::Creature | ObjectKind::Door | ObjectKind::Placeable => true,
            ObjectKind::Trigger => o.trigger().is_some_and(|t| t.linked_to_module.is_some()) || o.trap.is_some(),
            ObjectKind::Item => o.item().is_some_and(|i| i.possessor.is_none()),
            _ => false,
        })
        .map(|o| (o.position.truncate().distance(origin.truncate()), o.id))
        .filter(|&(d, _)| d <= radius)
        .collect();
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    candidates.truncate(NEARBY_CAP);
    let mut out = Vec::with_capacity(candidates.len());
    for (distance, id) in candidates {
        let is_creature = w.get(id).is_some_and(|o| o.kind == ObjectKind::Creature);
        let hostile = is_creature.then(|| w.is_enemy(leader, id));
        let Some(o) = w.get(id) else { continue };
        let (dead, locked, open, useable) = match (&o.detail, o.kind) {
            (Detail::Creature(c), _) => (Some(c.dead), None, None, !c.dead),
            (Detail::Door(d), _) => (None, Some(d.locked), Some(d.open), !d.is_static),
            (Detail::Placeable(p), _) => (None, Some(p.locked), Some(p.open), p.useable),
            _ => (None, None, None, false),
        };
        let exit_to = o.trigger().and_then(|t| t.linked_to_module.clone()).filter(|m| !m.is_empty());
        let scripted_use = o.script("OnUsed").is_some_and(|s| !s.is_empty());
        out.push(Nearby {
            id,
            tag: o.tag.clone(),
            name: w.substitute(&o.name),
            kind: format!("{:?}", o.kind).to_ascii_lowercase(),
            position: fmt_pos(o.position),
            distance,
            hostile,
            dead,
            locked,
            open,
            useable,
            scripted_use,
            conversation: o.conversation.is_some(),
            trapped: o.trap.is_some(),
            exit_to,
        });
    }
    for n in &mut out {
        if n.name.is_empty() {
            n.name = n.tag.clone();
        }
    }
    out
}

fn combat(w: &mut World, leader: ObjectId) -> Combat {
    let in_combat = w.in_combat(leader);
    let ids: Vec<ObjectId> = w.iter().filter(|o| o.kind == ObjectKind::Creature).map(|o| o.id).collect();
    let mut engaged = Vec::new();
    for id in ids {
        if w.get(id).and_then(|o| o.creature()).is_some_and(|c| !c.dead) && w.in_combat(id) && w.is_enemy(leader, id) {
            engaged.push(ObjRef { id, name: w.name_of(id) });
        }
    }
    Combat { in_combat, engaged }
}

fn inspect(w: &mut World, id: ObjectId) -> Reply {
    let Some(kind) = w.get(id).map(|o| o.kind) else { return Reply::err(format!("no object {id}")) };
    let base = json!({ "id": id, "kind": format!("{kind:?}").to_ascii_lowercase(), "tag": w.get(id).map(|o| o.tag.clone()), "name": w.name_of(id) });
    let data = match kind {
        ObjectKind::Creature => {
            let sheet = w.character_sheet(id).map(|s| {
                json!({
                    "level": s.level,
                    "classes": s.classes,
                    "abilities": s.abilities,
                    "defense": s.defense,
                    "hp": s.hp, "max_hp": s.max_hp, "fp": s.force, "max_fp": s.max_force,
                    "xp": s.xp, "next_level_xp": s.next_level_xp,
                    "fortitude": s.fortitude, "reflex": s.reflex, "will": s.will,
                    "attack": s.attack_main, "damage": s.damage_main,
                })
            });
            let skills: Vec<Value> = w.skill_views(id).into_iter().map(|s| json!({ "id": s.id, "name": s.name, "rank": s.rank, "bonus": s.bonus })).collect();
            let feats: Vec<Named> = w.feat_views(id).into_iter().map(|f| Named { id: i32::from(f.id), name: f.name }).collect();
            json!({ "sheet": sheet, "skills": skills, "feats": feats, "powers": w.power_views(id).into_iter().map(|p| Named { id: i32::from(p.id), name: p.name }).collect::<Vec<_>>() })
        }
        ObjectKind::Item => match w.item_view(id) {
            Some(v) => {
                let desc = w.item_def_of(id).map(|d| d.description.clone()).unwrap_or_default();
                json!({ "name": v.name, "description": desc, "stack": v.stack, "value": v.value, "plot": v.plot, "charges": v.charges, "equip_slots": v.equip_slots, "upgrade_level": v.upgrade_level })
            }
            None => json!({}),
        },
        ObjectKind::Door => {
            let d = w.get(id).and_then(|o| o.door().cloned());
            json!({ "open": d.as_ref().map(|d| d.open), "locked": d.as_ref().map(|d| d.locked), "key_required": d.as_ref().map(|d| d.key_required), "linked_module": d.and_then(|d| d.linked_to_module) })
        }
        ObjectKind::Placeable => {
            let p = w.get(id).and_then(|o| o.placeable().cloned());
            json!({ "open": p.as_ref().map(|p| p.open), "locked": p.as_ref().map(|p| p.locked), "useable": p.as_ref().map(|p| p.useable), "has_inventory": p.as_ref().map(|p| p.has_inventory) })
        }
        _ => json!({}),
    };
    let position = w.get(id).map(|o| fmt_pos(o.position));
    let mut out = base;
    if let (Value::Object(map), Value::Object(extra)) = (&mut out, data) {
        map.extend(extra);
    }
    if let Some(p) = position {
        out["position"] = json!(p);
    }
    Reply::ok_with(format!("{} {id}", w.name_of(id)), out)
}

impl World {
    /// Display name of an object: its name, else its tag, else its id.
    pub fn name_of(&self, id: ObjectId) -> String {
        match self.get(id) {
            Some(o) => {
                let n = self.substitute(&o.name);
                if !n.is_empty() {
                    n
                } else if !o.tag.is_empty() {
                    o.tag.clone()
                } else {
                    format!("object {id}")
                }
            }
            None => format!("object {id}"),
        }
    }

    /// Keeps the message log for replies (see [`World::emit`]).
    pub(crate) fn note_message(&mut self, e: &WorldEvent) {
        let line = match e {
            WorldEvent::Feedback { text, .. } => text.clone(),
            WorldEvent::Bark { object, text } | WorldEvent::Speak { object, text } => format!("{}: {text}", self.name_of(*object)),
            WorldEvent::ItemReceived { name, stack, .. } if *stack > 1 => format!("Received {name} (x{stack})"),
            WorldEvent::ItemReceived { name, .. } => format!("Received {name}"),
            WorldEvent::Damaged { object, amount, .. } => format!("{} takes {amount} damage", self.name_of(*object)),
            WorldEvent::Died { object, .. } => format!("{} dies", self.name_of(*object)),
            WorldEvent::KnockedDown { object, .. } => format!("{} is knocked down", self.name_of(*object)),
            WorldEvent::GotUp { object } => format!("{} gets up", self.name_of(*object)),
            WorldEvent::AttackHit { attacker, target, damage, critical } => {
                let crit = if *critical { " (critical)" } else { "" };
                format!("{} hits {} for {damage}{crit}", self.name_of(*attacker), self.name_of(*target))
            }
            WorldEvent::AttackMissed { attacker, target } => format!("{} misses {}", self.name_of(*attacker), self.name_of(*target)),
            WorldEvent::PartyDefeated => "The party is down.".to_owned(),
            WorldEvent::XpGained { creature, amount } => format!("{} gains {amount} XP", self.name_of(*creature)),
            WorldEvent::LevelUpAvailable { creature, level } => format!("{} can reach level {level}", self.name_of(*creature)),
            WorldEvent::LevelledUp { creature, level, .. } => format!("{} reaches level {level}", self.name_of(*creature)),
            WorldEvent::ConversationStarted { owner, dialog } => format!("Conversation with {} ({dialog})", self.name_of(*owner)),
            WorldEvent::ConversationEnded { aborted: true, .. } => "Conversation aborted".to_owned(),
            WorldEvent::ConversationEnded { .. } => "Conversation ended".to_owned(),
            WorldEvent::BarkLine { speaker, text, .. } => format!("{}: {text}", self.name_of(*speaker)),
            WorldEvent::Journal { plot, state } => format!("Journal: {plot} is now {state}"),
            WorldEvent::DoorChanged { door, open } => format!("{} {}", self.name_of(*door), if *open { "opens" } else { "closes" }),
            WorldEvent::PlaceableChanged { placeable, open } => format!("{} {}", self.name_of(*placeable), if *open { "opens" } else { "closes" }),
            WorldEvent::ContainerOpened { container, items } => format!("{} opened ({} items)", self.name_of(*container), items.len()),
            WorldEvent::SkillCheck { target, dc, total, success, .. } => {
                format!("Check on {}: {total} against DC {dc} {}", self.name_of(*target), if *success { "passed" } else { "failed" })
            }
            WorldEvent::LeaderChanged { new, .. } => format!("{} leads the party", self.name_of(*new)),
            WorldEvent::TrapTriggered { .. } => "A trap goes off.".to_owned(),
            WorldEvent::StealthChanged { creature, stealthed } => {
                format!("{} {}", self.name_of(*creature), if *stealthed { "hides" } else { "is seen" })
            }
            WorldEvent::ModuleEntered { module } => format!("Entered {module}"),
            WorldEvent::TransitionRequested(t) => format!("Leaving for {}", t.module),
            WorldEvent::Movie(m) => format!("Movie: {m}"),
            _ => return,
        };
        self.messages.push_back(line);
        while self.messages.len() > MESSAGE_CAP {
            self.messages.pop_front();
        }
    }
}

#[cfg(test)]
mod tests;
