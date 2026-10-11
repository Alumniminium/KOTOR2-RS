//! Headless KotOR2 game simulation.
//!
//! [`World`] owns the authoritative game state: objects spawned from a
//! k2-data module, globals, action queues, script events, timers, the
//! dialog runtime and area transitions. It implements the NWScript engine
//! routines ([`k2_script::ScriptHost`]) on top of that state. The Bevy layer
//! mirrors it: it reads object positions, drains [`WorldEvent`]s and reports
//! player input back.
//!
//! Game coordinates throughout (Z-up, meters). See docs/notes/world.md.

pub mod bash;
pub mod chargen;
pub mod combat;
pub mod control;
pub mod craft;
pub mod crowd;
pub mod dialog;
mod doors;
pub mod encounter;
pub mod engine;
pub mod factions;
pub mod globals;
pub mod host;
pub mod inventory;
pub mod items;
pub mod nav;
pub mod party;
pub mod pazaak;
pub mod minigame;
pub mod sheet;
pub mod object;
pub mod powers;
pub mod progression;
pub mod save;
mod spawn;
pub mod stats;
pub mod store;
pub mod tick;
pub mod traps;
pub mod stealth;
pub mod influence;
pub mod projectiles;
mod regen;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

use glam::Vec3;
use k2_data::{Module, Tables, Talk};
use k2_res::{Install, Resources};
use k2_script::{Builtins, Ncs, ObjectId, Outcome, Routines, ScriptState, Vm, OBJECT_INVALID};
use k2_walk::{DoorHandle, WalkMesh};
use serde::{Deserialize, Serialize};

pub use dialog::{Conversation, ConversationView, ReplyView};
pub use engine::{EngineStore, Location};
pub use globals::Globals;
pub use object::{Action, Detail, Object, ObjectKind, Target};
pub use tick::Motion;

#[derive(Debug, thiserror::Error)]
pub enum WorldError {
    #[error(transparent)]
    Data(#[from] k2_data::DataError),
    #[error(transparent)]
    Res(#[from] k2_res::ResError),
    #[error(transparent)]
    Script(#[from] k2_script::LoadError),
    #[error("{0}")]
    Other(String),
    /// A save this build cannot read (older or newer save format).
    #[error("{0}")]
    IncompatibleSave(String),
}

pub type Result<T> = std::result::Result<T, WorldError>;

/// A requested module change. The Bevy layer (or harness) calls
/// [`World::enter_module`] with it when it is ready to switch.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingTransition {
    pub module: String,
    /// Waypoint (or object) tag to place the party at; None = module entry.
    pub waypoint_tag: Option<String>,
    pub movies: Vec<String>,
}

/// Things the presentation layer should react to. Drain with
/// [`World::drain_events`].
#[derive(Debug, Clone, PartialEq)]
pub enum WorldEvent {
    ModuleEntered { module: String },
    ObjectSpawned(ObjectId),
    ObjectDestroyed(ObjectId),
    /// Control passed to another party member: Tab or the HUD portrait, a
    /// fallen leader replaced by the next standing member (party.rs
    /// party_member_down), or a raise that took control from a leader still
    /// down (party.rs hand_over_if_leader_down).
    LeaderChanged { old: ObjectId, new: ObjectId },
    /// `looping` animations run for `duration` seconds (< 0 = until replaced).
    PlayAnimation { object: ObjectId, animation: i32, speed: f32, duration: f32, looping: bool },
    DoorChanged { door: ObjectId, open: bool },
    PlaceableChanged { placeable: ObjectId, open: bool },
    ConversationStarted { owner: ObjectId, dialog: String },
    ConversationEnded { owner: ObjectId, aborted: bool },
    /// Floating text over an object (BarkString, ActionBarkString).
    Bark { object: ObjectId, text: String },
    Speak { object: ObjectId, text: String },
    /// A one-liner conversation played as a bark bubble: no dialog state,
    /// no camera, no pause. `vo` / `sound` are the line's audio (first found
    /// plays), `alien_race` / `emotion` pick the alien voice set when there is
    /// none; `delay` is the node Delay (seconds, when 1..59).
    /// `pos` is ActionStartConversation's nBarkX / nBarkY: the bubble's left
    /// and top in layout px (None = the layout's own position).
    BarkLine {
        speaker: ObjectId,
        dialog: String,
        text: String,
        vo: Option<String>,
        sound: Option<String>,
        alien_race: u32,
        emotion: i32,
        delay: Option<u32>,
        pos: Option<(i32, i32)>,
    },
    DebugText(String),
    FadeOut { wait: f32, length: f32, color: [f32; 3] },
    FadeIn { wait: f32, length: f32, color: [f32; 3] },
    Movie(String),
    PlaySound { object: ObjectId, sound: String },
    /// SoundObjectPlay / Stop / FadeAndStop (`fade` seconds, 0 = cut).
    SoundObject { object: ObjectId, playing: bool, fade: f32 },
    /// SoundObjectSetVolume (0..127).
    SoundObjectVolume { object: ObjectId, volume: i32 },
    /// SoundObjectSetFixedVariance: pitch shift in semitones (the swoop
    /// engine loops get 1.2 to 4 as the revs climb).
    SoundObjectVariance { object: ObjectId, variance: f32 },
    VisualEffect { object: ObjectId, effect: i32 },
    Journal { plot: String, state: i32 },
    TransitionRequested(PendingTransition),
    OpenStore { store: ObjectId },
    ShowGui(String),
    /// A swing or shot starts. `anim` is the suggested clip ("g2a1" style);
    /// try `fallbacks` in order when the model lacks it.
    AttackStarted { attacker: ObjectId, target: ObjectId, anim: String, fallbacks: Vec<String>, ranged: bool },
    AttackHit { attacker: ObjectId, target: ObjectId, damage: i32, critical: bool },
    AttackMissed { attacker: ObjectId, target: ObjectId },
    /// A blaster bolt was turned aside by the target's deflection (right after
    /// the AttackMissed for the same attack).
    AttackDeflected { attacker: ObjectId, target: ObjectId },
    /// A projectile leaves `shooter` from its `hand` node (`rhand`, or `lhand`
    /// for an off-hand blaster) `launch` seconds after this event, flies for
    /// `flight` seconds and ends at `to` (game space). `model` is the MDL drawn
    /// in flight; `arc` is a ballistic throw rather than a straight shot. A
    /// `bolt` is a blaster bolt drawn from the `bullethook` of the weapon in
    /// that hand (`muzzle` is its muzzle flash model); otherwise `model` is an
    /// item thrown from the hand. `ammo` is the ammunitiontypes row of that
    /// weapon (its shot and impact sounds). `end` says what happens at `to` (see
    /// [`projectiles::ProjectileEnd`]). `blast_model` / `blast_sound` are the
    /// explosion of a detonation.
    Projectile {
        shooter: ObjectId,
        model: String,
        hook: String,
        bolt: bool,
        muzzle: Option<String>,
        ammo: Option<usize>,
        to: Vec3,
        launch: f32,
        flight: f32,
        arc: bool,
        end: projectiles::ProjectileEnd,
        blast_model: Option<String>,
        blast_sound: Option<String>,
    },
    /// A lightsaber is thrown (spells with a catchanim: 4 and 49). It leaves
    /// `shooter`'s right hand `launch` seconds after this event and flies to
    /// each of `stops` (game space) in turn, `leg` seconds per stop (a hit at
    /// each). It comes back to the hand over `back` seconds. `model` is the
    /// saber's item model. The shooter plays `catch_anim` (spells.2da catchanim)
    /// `catch_lead` seconds before the saber is back.
    SaberThrow {
        shooter: ObjectId,
        model: String,
        stops: Vec<Vec3>,
        launch: f32,
        leg: f32,
        back: f32,
        catch_anim: String,
        catch_lead: f32,
    },
    /// HP lost from any source (attacks, scripted damage effects).
    Damaged { object: ObjectId, amount: i32, by: ObjectId },
    Died { object: ObjectId, killer: ObjectId },
    /// A party member reached 0 HP: unconscious, not dead. It gets up after
    /// the battle (see [`GotUp`](WorldEvent::GotUp)).
    KnockedDown { object: ObjectId, by: ObjectId },
    /// A downed party member gets up (after the battle, or healed).
    GotUp { object: ObjectId },
    /// The last standing party member went down (dead or knocked out): game
    /// over (load or quit).
    PartyDefeated,
    /// An item reached the party ("Item received" feedback).
    ItemReceived { item: ObjectId, name: String, stack: i32 },
    /// A creature's equipment changed in `slot` (respawn held weapon models).
    EquipmentChanged { creature: ObjectId, slot: i32 },
    /// The creature switched weapon sets; `set` (0 or 1) is now active.
    WeaponSetSwapped { creature: ObjectId, set: u8 },
    /// A creature's body model or texture changed (armour); respawn it.
    AppearanceChanged { object: ObjectId },
    /// A loot window opens (placeable inventory or a corpse).
    ContainerOpened { container: ObjectId, items: Vec<ObjectId> },
    ContainerClosed { container: ObjectId },
    /// ShowUpgradeScreen at a workbench with upgrading enabled. `item` is a
    /// forced target; `item_creation` says whether the create tab is open
    /// too, from the `recipes` table. See craft.rs.
    UpgradeScreen { item: Option<ObjectId>, character: ObjectId, item_creation: bool, recipes: String },
    /// Item creation/breakdown only: a workbench with upgrading disabled,
    /// or a lab station (ShowChemicalUpgradeScreen).
    ItemCreateScreen { station: craft::Station, character: ObjectId, recipes: String },
    /// A skill check against a DC (Security on locks).
    SkillCheck { user: ObjectId, target: ObjectId, skill: i32, dc: i32, total: i32, success: bool },
    /// Experience gained (shared PC XP reaches party members too).
    XpGained { creature: ObjectId, amount: i32 },
    /// `creature` has enough XP for `level`; the player picks via the
    /// level-up screen (World::level_up_options / apply_level_up).
    LevelUpAvailable { creature: ObjectId, level: i32 },
    LevelledUp { creature: ObjectId, level: i32, class: i32 },
    /// A Force power or item ability starts: play `conj_anim` then `anim`
    /// (spells.2da names, e.g. "self", "dark", "throw") over `duration` s.
    SpellCastStarted { caster: ObjectId, target: ObjectId, spell: i32, anim: String, conj_anim: String, sound: Option<String>, duration: f32, hostile: bool },
    /// The impact script ran (visuals come as VisualEffect events).
    SpellImpact { caster: ObjectId, target: ObjectId, spell: i32, hostile: bool, success: bool },
    /// A combat feat (Power Attack, Flurry, ...) arms the next round.
    FeatUsed { creature: ObjectId, target: ObjectId, feat: i32 },
    /// A feedback line for the message log (traps.rs, stealth.rs,
    /// influence.rs), with an optional guisounds.2da label to play.
    Feedback { text: String, cue: Option<String> },
    /// A mine or trap went off at `position` (traps.2da model and
    /// explosionsound).
    TrapTriggered { trap: ObjectId, position: Vec3, trap_type: i32, model: Option<String>, sound: Option<String> },
    /// A creature entered or left stealth mode.
    StealthChanged { creature: ObjectId, stealthed: bool },
}

/// Per-run script context: what the GetLastX / GetEnteringObject family
/// returns while a given script runs.
#[derive(Debug, Clone)]
pub struct ScriptCtx {
    pub entering: ObjectId,
    pub exiting: ObjectId,
    pub user_event: i32,
    pub params: [i32; 5],
    pub string_param: String,
    pub script_var: i32,
    pub last_speaker: ObjectId,
    pub listen_pattern: i32,
    pub last_used_by: ObjectId,
    pub last_opened_by: ObjectId,
    pub last_closed_by: ObjectId,
    pub clicking: ObjectId,
    pub last_perceived: ObjectId,
    pub perception_seen: bool,
}

impl Default for ScriptCtx {
    fn default() -> Self {
        Self {
            entering: OBJECT_INVALID,
            exiting: OBJECT_INVALID,
            user_event: 0,
            params: [0; 5],
            string_param: String::new(),
            script_var: -1,
            last_speaker: OBJECT_INVALID,
            listen_pattern: -1,
            last_used_by: OBJECT_INVALID,
            last_opened_by: OBJECT_INVALID,
            last_closed_by: OBJECT_INVALID,
            clicking: OBJECT_INVALID,
            last_perceived: OBJECT_INVALID,
            perception_seen: false,
        }
    }
}

/// Deferred work run after the current top-level script finishes.
#[derive(Debug, Clone)]
enum Pending {
    /// AssignCommand: resume the state with OBJECT_SELF = object.
    Assign(ObjectId, Arc<ScriptState>),
    /// Fire an object's event script.
    Event { object: ObjectId, label: String, ctx: ScriptCtx },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Timer {
    due: f64,
    kind: TimerKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum TimerKind {
    Resume(Arc<ScriptState>),
    Destroy(ObjectId),
}

/// Script iteration cursors (GetFirstX/GetNextX).
#[derive(Debug, Default)]
struct Iters {
    shape: (Vec<ObjectId>, usize),
    area: (Vec<ObjectId>, usize),
    inventory: HashMap<ObjectId, (Vec<ObjectId>, usize)>,
    effects: HashMap<ObjectId, usize>,
    pc: usize,
    persistent: (Vec<ObjectId>, usize),
}

/// A party NPC slot (K2 has 12, 0..=11).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AvailableNpc {
    pub template: String,
    pub object: Option<ObjectId>,
    pub selectable: bool,
    /// The creature as it was when party selection left it behind.
    pub stored: Option<Box<Object>>,
}

/// What the player character is made of: character generation's result
/// (chargen.rs). `class: None` is the pre-chargen default, a level 1 Jedi
/// Guardian with fixed scores.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerSpec {
    pub appearance: u32,
    pub gender: i32,
    pub name: String,
    pub class: Option<i32>,
    /// portraits.2da row.
    pub portrait: Option<u32>,
    pub abilities: Option<[u8; 6]>,
    /// Ranks bought at level 1 (SKILL_* order).
    pub skills: Option<[i32; craft::SKILL_COUNT]>,
    pub feats: Vec<u16>,
    pub powers: Vec<i32>,
}

pub struct World {
    pub res: Resources,
    pub tables: Tables,
    pub talk: Option<Talk>,
    pub routines: Arc<Routines>,
    scripts: HashMap<String, Option<Arc<Ncs>>>,
    pub module: Option<Module>,
    /// All live objects, keyed by id (ids grow, so this is creation order).
    pub objects: BTreeMap<ObjectId, Object>,
    next_id: ObjectId,
    pub module_id: ObjectId,
    pub area_id: ObjectId,
    pub player: ObjectId,
    /// Party members currently in the area; party[0] is the leader.
    pub party: Vec<ObjectId>,
    pub available_npcs: HashMap<i32, AvailableNpc>,
    pub globals: Globals,
    pub journal: HashMap<String, i32>,
    pub tokens: HashMap<i32, String>,
    pub influence: HashMap<i32, i32>,
    pub walk: WalkMesh,
    /// Nodes a steering route search expands a frame (`steer_leader`, at least 1).
    /// The game keeps `nav::USE_EXPAND`; a test lowers it so that a route spans frames.
    pub route_step: usize,
    door_walk: HashMap<ObjectId, DoorHandle>,
    /// Set by `move_towards` while a mover waits for a door to swing: the wait is not stuck.
    door_waiting: bool,
    door_anims: doors::DoorAnims,
    /// Placeable PWK meshes, disabled when the placeable is destroyed.
    placeable_walk: HashMap<ObjectId, k2_walk::MeshHandle>,
    /// Game time in seconds since the world was created.
    pub time: f64,
    timers: Vec<Timer>,
    /// Area scripts (area, script name) that queued their own next resume the
    /// last time one of their resumes ran: their loops (see [`World::settled`]).
    /// Runtime only, not saved, and cleared on a module change.
    looping_scripts: BTreeSet<(ObjectId, String)>,
    pending: VecDeque<Pending>,
    ctx: Vec<ScriptCtx>,
    pub engine: EngineStore,
    pub conversation: Option<Conversation>,
    /// Bare lines barked, per speaker (dialog.rs speak_line). talk_to compares
    /// the NPC's own count across its ScriptDialogue.
    pub(crate) barks_by: HashMap<ObjectId, u32>,
    pub pending_transition: Option<PendingTransition>,
    events: Vec<WorldEvent>,
    /// Routine name to call count for routines with no implementation.
    pub unimplemented: BTreeMap<String, usize>,
    pub vm_errors: Vec<String>,
    pub(crate) builtins: Builtins,
    iters: Iters,
    pub player_spec: PlayerSpec,
    /// Seconds until the next perception sweep.
    perception_in: f32,
    /// Set by SetGlobalFadeOut / NoClicksFor etc; informational.
    pub input_blocked_for: f32,
    /// Log script events to stdout (harness).
    pub trace: bool,
    /// Print every engine call (harness).
    pub call_log: bool,
    /// Pick reply 0 automatically (harness).
    pub auto_dialog: bool,
    /// Swoop gears shift by themselves at each gear's top speed (harness and
    /// the `--mg-auto` autopilot). Off in play: shifting is the accelerate
    /// key (Up arrow / W), as in the original.
    pub auto_gears: bool,
    /// Non-fatal load problems (missing blueprints, dialogs).
    pub warnings: Vec<String>,
    /// Objects moved by a scripted action during the last tick (walk/run
    /// animation); absent = not moved by the simulation.
    pub motion: HashMap<ObjectId, tick::Motion>,
    pub solo_mode: bool,
    /// SetAreaUnescapable: overrides the ARE "Unescapable" flag for this
    /// module (see `area_unescapable`). None = the ARE flag decides. Reset on
    /// module entry.
    pub area_unescapable_override: Option<bool>,
    /// SetDisableTransit: refuses the party's transitions (see
    /// `transit_blocked`). Reset on module entry.
    pub transit_disabled: bool,
    pub last_pazaak: i32,
    /// PlayPazaak waiting for the pazaak screen (pazaak.rs). Not saved:
    /// saving is refused while it is set.
    pub pazaak: Option<pazaak::PazaakRequest>,
    /// Swoop/turret minigame of the current module (minigame.rs). Not
    /// saved: saving is refused in minigame modules.
    pub minigame: Option<minigame::MiniGame>,
    /// ShowPartySelectionGUI waiting for the party screen (party.rs).
    pub party_select: Option<party::PartySelectRequest>,
    /// The original PC while SwitchPlayerCharacter has another in control.
    pub stashed_pc: Option<Object>,
    /// Galaxy map: planet -> (available, selectable).
    pub planets: HashMap<i32, (bool, bool)>,
    pub selected_planet: i32,
    /// Combat state: factions, rounds, per-creature memory. See combat.rs.
    pub combat: combat::CombatSystem,
    /// Path cache for scripted movers. See nav.rs.
    pub paths: nav::Paths,
    /// Creature-creature collision state. See crowd.rs.
    pub crowd: crowd::Crowd,
    /// Item catalog, open container/store state. See inventory.rs.
    pub inventory: inventory::Inventories,
    /// Workbench/lab screens, skills, installed upgrades. See craft.rs.
    pub craft: craft::CraftState,
    /// Character sheet, journal and item-use (spell) state. See sheet.rs.
    pub sheet: sheet::SheetState,
    /// XP, levels, known feats and powers. See progression.rs.
    pub progression: progression::ProgressionState,
    /// Force powers, talents, effect modifiers. See powers.rs.
    pub powers: powers::PowersState,
    /// State of modules the party left, restored on re-entry (dead stay
    /// dead, opened doors stay open). See save.rs.
    pub module_memory: HashMap<String, save::ModuleSnapshot>,
    /// Trap detection and stealth state. See traps.rs, stealth.rs.
    pub systems: traps::SystemsState,
    /// Message log lines (feedback, barks, damage, ...) for the control API,
    /// newest last, capped. Not saved. See control.rs.
    pub(crate) messages: VecDeque<String>,
}

impl World {
    /// Creates an empty world over an install. Call [`World::enter_module`]
    /// to load a module.
    pub fn new(install: Install) -> Result<Self> {
        let res = Resources::open(install.clone())?;
        let routines = Arc::new(k2_script::load_routines(&res)?);
        let talk = Talk::load(&install).ok();
        let tables = Tables::new();
        let globals = Globals::from_tables(&res, &tables);
        let player_spec = spawn::default_player_spec(&res, &tables);
        let walk = k2_walk::WalkMeshBuilder::new(k2_walk::Materials::default()).build();
        Ok(Self {
            res,
            tables,
            talk,
            routines,
            scripts: HashMap::new(),
            module: None,
            objects: BTreeMap::new(),
            next_id: 1,
            module_id: OBJECT_INVALID,
            area_id: OBJECT_INVALID,
            player: OBJECT_INVALID,
            party: Vec::new(),
            available_npcs: HashMap::new(),
            globals,
            journal: HashMap::new(),
            tokens: HashMap::new(),
            influence: HashMap::new(),
            walk,
            door_walk: HashMap::new(),
            door_waiting: false,
            door_anims: Default::default(),
            placeable_walk: HashMap::new(),
            time: 0.0,
            timers: Vec::new(),
            looping_scripts: BTreeSet::new(),
            pending: VecDeque::new(),
            ctx: Vec::new(),
            engine: EngineStore::default(),
            conversation: None,
            barks_by: HashMap::new(),
            pending_transition: None,
            events: Vec::new(),
            unimplemented: BTreeMap::new(),
            vm_errors: Vec::new(),
            builtins: Builtins::with_seed(0x6b32),
            iters: Iters::default(),
            player_spec,
            perception_in: 0.5,
            input_blocked_for: 0.0,
            trace: false,
            auto_dialog: false,
            auto_gears: false,
            call_log: std::env::var_os("K2_CALLS").is_some(),
            warnings: Vec::new(),
            motion: HashMap::new(),
            solo_mode: false,
            area_unescapable_override: None,
            transit_disabled: false,
            last_pazaak: 0,
            pazaak: None,
            minigame: None,
            party_select: None,
            stashed_pc: None,
            planets: HashMap::new(),
            selected_planet: -1,
            combat: combat::CombatSystem::default(),
            paths: nav::Paths::default(),
            crowd: crowd::Crowd::default(),
            inventory: inventory::Inventories::default(),
            craft: craft::CraftState::default(),
            sheet: sheet::SheetState::default(),
            progression: progression::ProgressionState::default(),
            powers: powers::PowersState::default(),
            module_memory: HashMap::new(),
            systems: traps::SystemsState::default(),
            messages: VecDeque::new(),
            route_step: nav::USE_EXPAND,
        })
    }

    // ---------------------------------------------------------------
    // Object access
    // ---------------------------------------------------------------

    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(&id).filter(|o| !o.destroyed)
    }

    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.get_mut(&id).filter(|o| !o.destroyed)
    }

    pub fn is_valid(&self, id: ObjectId) -> bool {
        self.get(id).is_some()
    }

    /// Live objects in creation order.
    pub fn iter(&self) -> impl Iterator<Item = &Object> {
        self.objects.values().filter(|o| !o.destroyed)
    }

    pub fn by_tag(&self, tag: &str, nth: usize) -> Option<ObjectId> {
        if tag.is_empty() {
            return None;
        }
        self.iter()
            .filter(|o| o.kind != ObjectKind::Item || o.item().is_some_and(|i| i.possessor.is_none()))
            .filter(|o| o.tag.eq_ignore_ascii_case(tag))
            .nth(nth)
            .map(|o| o.id)
    }

    pub fn party_leader(&self) -> ObjectId {
        self.party.first().copied().unwrap_or(self.player)
    }

    pub(crate) fn alloc_id(&mut self) -> ObjectId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub(crate) fn emit(&mut self, e: WorldEvent) {
        if self.trace {
            println!("    event {e:?}");
        }
        self.note_message(&e);
        self.events.push(e);
    }

    /// Events since the last drain.
    pub fn drain_events(&mut self) -> Vec<WorldEvent> {
        std::mem::take(&mut self.events)
    }

    // ---------------------------------------------------------------
    // Scripts
    // ---------------------------------------------------------------

    pub(crate) fn load_script(&mut self, name: &str) -> Option<Arc<Ncs>> {
        let key = name.to_ascii_lowercase();
        if let Some(s) = self.scripts.get(&key) {
            return s.clone();
        }
        let s = k2_script::load_ncs(&self.res, &key).ok();
        self.scripts.insert(key, s.clone());
        s
    }

    fn current_ctx(&self) -> ScriptCtx {
        self.ctx.last().cloned().unwrap_or_default()
    }

    pub(crate) fn ctx(&self) -> &ScriptCtx {
        static EMPTY: std::sync::OnceLock<ScriptCtx> = std::sync::OnceLock::new();
        self.ctx.last().unwrap_or_else(|| EMPTY.get_or_init(ScriptCtx::default))
    }

    /// Runs a script now with `caller` as OBJECT_SELF. Returns None when the
    /// script does not exist or the VM failed (the error is recorded).
    pub fn run_script(&mut self, name: &str, caller: ObjectId, ctx: ScriptCtx) -> Option<Outcome> {
        if name.is_empty() || self.ctx.len() >= 24 {
            return None;
        }
        let ncs = self.load_script(name)?;
        if self.trace {
            println!("    run {name} self={caller:#x}");
        }
        self.ctx.push(ctx);
        let routines = self.routines.clone();
        let vm = Vm::new(&routines);
        let r = vm.run(&ncs, self, caller);
        self.ctx.pop();
        match r {
            Ok(o) => Some(o),
            Err(e) => {
                self.vm_errors.push(e.to_string());
                None
            }
        }
    }

    /// Runs a conditional script (StartingConditional). Missing scripts pass.
    pub fn run_conditional(&mut self, name: &str, caller: ObjectId, ctx: ScriptCtx) -> bool {
        if self.load_script(name).is_none() {
            return true;
        }
        self.run_script(name, caller, ctx).is_some_and(|o| o.truthy())
    }

    pub(crate) fn resume(&mut self, state: &ScriptState, caller: ObjectId) {
        if self.ctx.len() >= 24 {
            return;
        }
        let mut st = state.clone();
        st.caller = caller;
        self.ctx.push(self.current_ctx());
        let routines = self.routines.clone();
        let vm = Vm::new(&routines);
        let r = vm.resume(&st, self);
        self.ctx.pop();
        if let Err(e) = r {
            self.vm_errors.push(e.to_string());
        }
    }

    /// Queues an object's event script (fired by [`World::pump`]).
    pub fn queue_event(&mut self, object: ObjectId, label: &str, ctx: ScriptCtx) {
        self.pending.push_back(Pending::Event { object, label: label.to_owned(), ctx });
    }

    /// Fires an object's event script now (and then pumps).
    pub fn fire(&mut self, object: ObjectId, label: &str, ctx: ScriptCtx) {
        self.queue_event(object, label, ctx);
        self.pump();
    }

    /// Runs queued AssignCommands and event scripts until none are left.
    pub fn pump(&mut self) {
        if !self.ctx.is_empty() {
            return; // runs when the outermost script returns
        }
        let mut budget = 20_000;
        while let Some(p) = self.pending.pop_front() {
            budget -= 1;
            if budget == 0 {
                self.vm_errors.push("pump budget exhausted (event loop?)".into());
                self.pending.clear();
                break;
            }
            match p {
                Pending::Assign(obj, state) => {
                    if self.is_valid(obj) {
                        self.resume(&state, obj);
                    }
                }
                Pending::Event { object, label, ctx } => {
                    let Some(script) = self.get(object).and_then(|o| o.script(&label)).map(str::to_owned) else {
                        continue;
                    };
                    if self.trace {
                        println!("  {label} on {} -> {script}", self.describe(object));
                    }
                    self.run_script(&script, object, ctx);
                }
            }
        }
    }

    /// Short human-readable object label for logs.
    pub fn describe(&self, id: ObjectId) -> String {
        match self.objects.get(&id) {
            Some(o) if !o.tag.is_empty() => format!("{:?}:{}", o.kind, o.tag),
            Some(o) => format!("{:?}#{}", o.kind, o.id),
            None => format!("{id:#x}"),
        }
    }

    // ---------------------------------------------------------------
    // Input from the presentation layer
    // ---------------------------------------------------------------

    /// The presentation layer moved the player (direct control).
    pub fn set_player_position(&mut self, position: Vec3, bearing: f32) {
        let leader = self.party_leader();
        if let Some(p) = self.get_mut(leader) {
            p.position = position;
            p.bearing = bearing;
        }
    }

    /// Whether a cutscene holds the party, so the leader must not move on the player's or a
    /// control verb's orders: a conversation (camera cutscenes and animated dialogs included, paused
    /// ones too), input blocked by a script (NoClicksFor), scripted actions on the leader (its own
    /// attacks do not count), or a leader that cannot act (dead, downed, not commandable). A movie
    /// is the presentation's to report (`control::Env::movie`).
    pub fn party_held(&self) -> bool {
        self.held_with_talk(self.conversation.is_some())
    }

    /// `party_held` for a control walk: a door's message box (its own line and an OK, see
    /// `ConversationView::boxed`) is feedback, not a cutscene, so it does not stop the walk. A walk
    /// that hits a locked door is answered by its own locked-door feedback, box or not.
    pub fn walk_held(&self) -> bool {
        self.held_with_talk(self.conversation.is_some() && !self.conversation_view().is_some_and(|v| v.boxed))
    }

    fn held_with_talk(&self, talking: bool) -> bool {
        let Some(o) = self.get(self.party_leader()) else { return true };
        talking
            || self.input_blocked_for > 0.0
            || !o.commandable
            || o.creature().is_none_or(|c| c.dead || c.down)
            || o.actions.iter().any(|a| !matches!(a, Action::Attack(_)))
    }

    /// Whether the player is free to move (no scripted actions, no dialog).
    pub fn player_controlled(&self) -> bool {
        let leader = self.party_leader();
        self.conversation.is_none() && self.get(leader).is_some_and(|p| p.actions.is_empty() && p.commandable && p.creature().is_none_or(|c| !c.dead && !c.down))
    }

    /// Player control, and nothing scripted still to come: no queued script
    /// event, no module change, no DelayCommand or other resume pending except
    /// an area loop (see `World::is_ambient`), and no queued action on any
    /// party member except follows (a companion's heartbeat ActionFollowLeader
    /// stays queued and counts as idle). wait_until idle uses this; the game's
    /// own input gating keeps [`World::player_controlled`].
    pub fn settled(&self) -> bool {
        self.player_controlled()
            && self.pending_transition.is_none()
            && self.pending.is_empty()
            && self.resumes_settled()
            && self.party.iter().all(|&id| self.get(id).is_none_or(|o| o.actions.iter().all(|a| matches!(a, Action::Follow { .. }))))
    }

    /// The resume part of [`World::settled`]: every pending resume is an area loop.
    fn resumes_settled(&self) -> bool {
        !self.timers.iter().any(|t| matches!(&t.kind, TimerKind::Resume(s) if !self.is_ambient(s)))
    }

    /// The player clicked / used an object.
    pub fn use_object(&mut self, user: ObjectId, target: ObjectId) {
        let Some(kind) = self.get(target).map(|o| o.kind) else { return };
        let ctx = ScriptCtx { last_used_by: user, clicking: user, last_opened_by: user, last_speaker: user, ..Default::default() };
        match kind {
            ObjectKind::Door => {
                let (locked, open, is_static, linked_module, linked_to, locked_at_load) = {
                    let d = self.get(target).and_then(|o| o.door()).cloned().unwrap_or_default();
                    (d.locked, d.open, d.is_static, d.linked_to_module.clone(), d.linked_to.clone(), d.locked_at_load.unwrap_or(false))
                };
                self.fire(target, "OnClick", ctx.clone());
                if locked {
                    self.fire(target, "OnFailToOpen", ctx);
                    // A locked door with a conversation says why (sealeddoor, turb101).
                    // Its OnFailToOpen (a_compdlg) may already queue that talk on the
                    // door (ActionStartConversation with the door's own dialog); then
                    // the queue gives the line, on the next tick.
                    if !self.has_queued_start(target) {
                        let _ = self.talk_on_use(target, user);
                    }
                } else if let Some(module) = linked_module.filter(|m| !m.is_empty()) {
                    // A trapped transition door goes off before the transit
                    // (inferred, as for any trapped door).
                    self.trigger_object_trap(target, user);
                    // A refused transit does nothing: no OnFailToOpen and no
                    // message. The data has no evidence of either (nwscript,
                    // FeedBackText.2da and dialog.tlk show no transit-refused
                    // text), and OnFailToOpen is the locked-door event.
                    if !self.transit_blocked() {
                        self.request_transition(module, Some(linked_to).filter(|s| !s.is_empty()), Vec::new());
                    }
                } else if is_static {
                    // A static door never opens or closes on use: its conversation,
                    // if it has one and the start passes, is its answer ("magnetically
                    // sealed"); otherwise nothing happens.
                    let _ = self.talk_on_use(target, user);
                } else if open {
                    self.set_door_open(target, false, user);
                } else if !locked_at_load && self.talk_on_use(target, user) {
                    // A closed door with a conversation does not open: the line is
                    // its answer ("cannot be opened from here"). Its start conditions
                    // decide: a start that fails (GuardRoomDr, once its talked flag,
                    // local bool 10, is set) lets the door open. A door locked at load
                    // that a script has unlocked opens on use instead (heuristic,
                    // docs/notes/world.md).
                } else {
                    self.trigger_object_trap(target, user);
                    self.set_door_open(target, true, user);
                }
            }
            ObjectKind::Placeable if self.is_container(target) => {
                // A container with a conversation talks on use (cont_sensor_drds,
                // plscylsp); it still opens, so a conversation that always starts
                // does not keep it shut.
                let _ = self.talk_on_use(target, user);
                let _ = self.open_container(user, target);
            }
            ObjectKind::Creature if self.is_container(target) => {
                let _ = self.open_container(user, target);
            }
            ObjectKind::Placeable => {
                // A locked placeable's trap does not go off (inferred: the use
                // fails on the lock, as a locked door does). OnUsed and the
                // conversation still run: shipped locked templates rely on it
                // (001ebo cont_sensor_drds, 604dan knrthegg).
                let conv = self.get(target).and_then(|o| o.conversation.clone());
                if !self.get(target).and_then(|o| o.placeable()).is_some_and(|p| p.locked) {
                    self.trigger_object_trap(target, user);
                }
                self.fire(target, "OnUsed", ctx);
                if let Some(dlg) = conv
                    && self.conversation.is_none()
                {
                    self.start_conversation(target, user, &dlg);
                }
            }
            ObjectKind::Creature => self.talk_to(user, target),
            _ => {}
        }
    }

    /// Whether `id` has a StartConversation waiting in its actions.
    fn has_queued_start(&self, id: ObjectId) -> bool {
        self.get(id).is_some_and(|o| o.actions.iter().any(|a| matches!(a, Action::StartConversation { .. })))
    }

    /// A door or container that has a conversation talks when it is used, as
    /// the placeable arm does. False when it has none or a conversation is
    /// already running.
    fn talk_on_use(&mut self, target: ObjectId, user: ObjectId) -> bool {
        let Some(dlg) = self.get(target).and_then(|o| o.conversation.clone()).filter(|s| !s.is_empty()) else { return false };
        self.conversation.is_none() && self.start_conversation(target, user, &dlg)
    }

    /// The player starts talking to a creature: fires its OnDialog script
    /// (which normally calls BeginConversation), falling back to its
    /// conversation file.
    pub fn talk_to(&mut self, pc: ObjectId, npc: ObjectId) {
        let has_script = self.get(npc).is_some_and(|o| o.script("ScriptDialogue").is_some());
        let ctx = ScriptCtx { last_speaker: pc, listen_pattern: -1, ..Default::default() };
        let (len, epoch) = self.get(npc).map_or((0, 0), |o| (o.actions.len(), o.action_epoch));
        let barks = self.barks_by.get(&npc).copied().unwrap_or(0);
        let was_free = self.conversation.is_none();
        if has_script {
            self.fire(npc, "ScriptDialogue", ctx);
        }
        // What the script already did with this talk, for this NPC only: a
        // bare line it barked, or a conversation it started.
        let barked = self.barks_by.get(&npc).copied().unwrap_or(0) != barks;
        let talked = barked || (was_free && self.conversation_owner() == npc);
        // The starts the script queued for this NPC are the ones past the old
        // end of its queue (all of them when ClearAllActions ran, as
        // k_ai_master does first).
        let Some((fresh, new_starts, front_is_new_start)) = self.new_starts_since(npc, len, epoch) else { return };
        // The first queued start is the talk when the queue is fresh and the
        // start is at its front: it runs on the next tick (tick.rs
        // run_actions). Every other new start is dropped so the talk runs once.
        let is_talk = fresh && !talked && front_is_new_start;
        let keep = if is_talk { new_starts.first().copied() } else { None };
        if let Some(o) = self.get_mut(npc) {
            for &i in new_starts.iter().rev().filter(|&&i| Some(i) != keep) {
                o.actions.remove(i);
            }
        }
        if is_talk {
            return;
        }
        // The fallback is for a ScriptDialogue that did nothing.
        if self.conversation.is_none()
            && !talked
            && let Some(dlg) = self.get(npc).and_then(|o| o.conversation.clone())
        {
            self.start_conversation(npc, pc, &dlg);
        }
    }

    /// Indices of StartConversation actions in `npc`'s queue past `len` (or
    /// all of them when the epoch changed, i.e. the queue was cleared). Also
    /// whether the queue is fresh (cleared or empty before) and its front is
    /// one of those starts. None when the NPC does not exist.
    fn new_starts_since(&self, npc: ObjectId, len: usize, epoch: u32) -> Option<(bool, Vec<usize>, bool)> {
        let o = self.get(npc)?;
        // A queue that shrank was cleared too (clear_new_leader_queue does not bump the epoch).
        let cleared = o.action_epoch != epoch || o.actions.len() < len;
        let fresh = cleared || len == 0;
        let base = if cleared { 0 } else { len.min(o.actions.len()) };
        let starts: Vec<usize> = o
            .actions
            .iter()
            .enumerate()
            .skip(base)
            .filter(|(_, a)| matches!(a, Action::StartConversation { .. }))
            .map(|(i, _)| i)
            .collect();
        let front = starts.first() == Some(&0);
        Some((fresh, starts, front))
    }

    /// The walkmesh handles of a door (diagnostics).
    pub fn door_handle(&self, door: ObjectId) -> Option<k2_walk::DoorHandle> {
        self.door_walk.get(&door).copied()
    }

    /// Events and scripts of a door that just opened or closed.
    pub(crate) fn finish_door_change(&mut self, door: ObjectId, open: bool, by: ObjectId) {
        self.emit(WorldEvent::DoorChanged { door, open });
        let ctx = ScriptCtx { last_opened_by: by, last_closed_by: by, ..Default::default() };
        self.queue_event(door, if open { "OnOpen" } else { "OnClosed" }, ctx);
        self.pump();
    }

    /// GetAreaUnescapable: the SetAreaUnescapable override if a script set
    /// one, else the ARE "Unescapable" flag of the current area. 34 of the 82
    /// modules set the ARE flag (101per, 103per, 105per, 001ebo-003ebo, ...).
    /// No transit checks it (see `transit_blocked`).
    pub fn area_unescapable(&self) -> bool {
        self.area_unescapable_override.unwrap_or_else(|| self.module.as_ref().is_some_and(|m| m.area.unescapable))
    }

    /// Whether the party's transitions (transition triggers and linked doors)
    /// are refused. That is SetDisableTransit and nothing else; nwscript says
    /// "This will disable or enable area transit". StartNewModule and
    /// same-module jumps are never refused.
    ///
    /// Working interpretation, not verified against the engine: Unescapable
    /// (SetAreaUnescapable and the ARE flag) is about leaving the area by
    /// travel, such as the map back to the Ebon Hawk, not about walking
    /// through transitions. 34 of 82 areas set it, including 101per's
    /// turbolift. The travel path is not in k2-world yet (ShowGalaxyMap only
    /// emits a GUI event), so when it is added it should check
    /// `area_unescapable`.
    pub fn transit_blocked(&self) -> bool {
        self.transit_disabled
    }

    pub(crate) fn request_transition(&mut self, module: String, waypoint_tag: Option<String>, movies: Vec<String>) {
        let t = PendingTransition { module: module.to_ascii_lowercase(), waypoint_tag, movies };
        if self.pending_transition.is_none() {
            self.emit(WorldEvent::TransitionRequested(t.clone()));
            self.pending_transition = Some(t);
        }
    }

    /// Whether a resume is an area loop: the area's own script resuming itself
    /// on a schedule for the whole visit. Only a script that has already re-armed
    /// itself counts (see `tick.rs`), so a one-shot DelayCommand from an area
    /// script still holds the party; the first delay of a loop does too, until
    /// the loop's next turn.
    fn is_ambient(&self, s: &ScriptState) -> bool {
        s.caller == self.area_id && self.looping_scripts.contains(&(self.area_id, s.script.name.clone()))
    }

    /// Records one turn of an area script's resume: `rearmed` when the script
    /// queued its own next resume, which makes it a loop; otherwise it is no
    /// longer looping, so its later one-shot delays hold the party again. A
    /// finite chain of the script's own delays reads as a loop until its last turn.
    pub(crate) fn record_area_turn(&mut self, script: &str, rearmed: bool) {
        let key = (self.area_id, script.to_owned());
        if rearmed {
            self.looping_scripts.insert(key);
        } else {
            self.looping_scripts.remove(&key);
        }
    }

    pub(crate) fn add_timer(&mut self, delay: f32, kind: TimerKind) {
        self.timers.push(Timer { due: self.time + f64::from(delay.max(0.0)), kind });
    }

    /// Queues `action` on `object` unless it is not commandable or knocked out.
    pub fn push_action(&mut self, object: ObjectId, action: Action) {
        // Downed creatures take no orders; get_up (party.rs) clears the state.
        if let Some(o) = self.get_mut(object)
            && o.commandable
            && o.creature().is_none_or(|c| !c.down)
        {
            o.actions.push_back(action);
        }
    }

    /// Distance between two objects in 3D (None if either is invalid).
    pub fn distance(&self, a: ObjectId, b: ObjectId) -> Option<f32> {
        Some(self.get(a)?.position.distance(self.get(b)?.position))
    }
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World").field("objects", &self.objects.len()).field("time", &self.time).finish()
    }
}

#[cfg(test)]
mod area_loop_tests;
