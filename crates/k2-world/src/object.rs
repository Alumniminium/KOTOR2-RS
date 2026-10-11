//! Game objects and their action queues.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use glam::Vec3;
use k2_script::{ObjectId, ScriptState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ObjectKind {
    Module,
    Area,
    Creature,
    Door,
    Placeable,
    Trigger,
    Waypoint,
    Sound,
    Store,
    Encounter,
    Item,
}

impl ObjectKind {
    /// OBJECT_TYPE_* bit as used by script filters.
    pub fn type_bit(self) -> i32 {
        match self {
            ObjectKind::Creature => 1,
            ObjectKind::Item => 2,
            ObjectKind::Trigger => 4,
            ObjectKind::Door => 8,
            ObjectKind::Waypoint => 32,
            ObjectKind::Placeable => 64,
            ObjectKind::Store => 128,
            ObjectKind::Encounter => 256,
            ObjectKind::Sound => 512,
            ObjectKind::Module | ObjectKind::Area => 0,
        }
    }
}

/// Where a move action is headed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Target {
    Point(Vec3),
    Object(ObjectId),
}

/// Whom a follow action walks after.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum FollowTarget {
    /// The party leader (ActionFollowLeader).
    Leader,
    /// The object's owner (ActionFollowOwner), see `Object::owner`.
    Owner,
    /// A given object (ActionForceFollowObject).
    Object(ObjectId),
}

/// One queued action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    MoveTo { target: Target, run: bool, range: f32, timeout: Option<f32>, elapsed: f32, stuck: f32 },
    MoveAway { from: ObjectId, run: bool, range: f32, elapsed: f32 },
    /// Walks after `of` and stands within `range` of it. ActionFollowLeader
    /// yields to the next queued action; the others only ClearAllActions
    /// removes (tick.rs).
    Follow { of: FollowTarget, range: f32 },
    JumpTo { target: Target, bearing: Option<f32> },
    Wait { remaining: f32 },
    PlayAnimation { anim: i32, speed: f32, duration: f32, started: bool, remaining: f32 },
    OpenDoor(ObjectId),
    CloseDoor(ObjectId),
    Lock(ObjectId, bool),
    /// `bark`: ActionStartConversation's nBarkX / nBarkY, the bubble's left and
    /// top when the conversation turns out to be a bark (None = the layout's).
    StartConversation {
        with: ObjectId,
        dialog: String,
        #[serde(default)]
        bark: Option<(i32, i32)>,
    },
    DoCommand(Arc<ScriptState>),
    /// ActionSpeakString: `volume` is the TALKVOLUME_* that decides who hears it.
    Speak {
        text: String,
        #[serde(default)]
        volume: i32,
    },
    Attack(ObjectId),
    PauseConversation,
    ResumeConversation,
    /// Accepted but not simulated: the routines in host.rs that no other
    /// handler takes (ActionEquipMostEffectiveArmor, ActionPickUpItem, ...).
    Noop(std::borrow::Cow<'static, str>),
    /// ActionSwitchWeapons: swaps the two weapon sets, then completes.
    SwitchWeapons,
    /// Force power, item ability or combat feat. See powers.rs.
    Cast(Box<crate::powers::CastAction>),
}

impl Action {
    /// ACTION_* constant name for GetCurrentAction.
    pub fn constant_name(&self) -> &'static str {
        match self {
            Action::MoveTo { target: Target::Point(_), .. } => "ACTION_MOVETOPOINT",
            Action::MoveTo { target: Target::Object(_), .. } => "ACTION_MOVETOPOINT",
            Action::MoveAway { .. } => "ACTION_MOVETOPOINT",
            Action::Follow { of: FollowTarget::Leader, .. } => "ACTION_FOLLOWLEADER",
            Action::Follow { of: FollowTarget::Owner, .. } => "ACTION_FOLLOWOWNER",
            Action::Follow { of: FollowTarget::Object(_), .. } => "ACTION_FOLLOW",
            Action::JumpTo { .. } => "ACTION_JUMPTOPOINT",
            Action::Wait { .. } => "ACTION_WAIT",
            Action::PlayAnimation { .. } => "ACTION_PLAYANIMATION",
            Action::OpenDoor(_) => "ACTION_OPENDOOR",
            Action::CloseDoor(_) => "ACTION_CLOSEDOOR",
            Action::Lock(_, true) => "ACTION_LOCK",
            Action::Lock(_, false) => "ACTION_OPENLOCK",
            Action::StartConversation { .. } => "ACTION_DIALOGOBJECT",
            Action::DoCommand(_) => "ACTION_DOCOMMAND",
            Action::Speak { .. } => "ACTION_WAIT",
            Action::Attack(_) => "ACTION_ATTACKOBJECT",
            Action::PauseConversation | Action::ResumeConversation => "ACTION_WAIT",
            Action::Noop(_) | Action::SwitchWeapons => "ACTION_WAIT",
            Action::Cast(_) => "ACTION_CASTSPELL",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DoorState {
    pub open: bool,
    pub locked: bool,
    pub key_required: bool,
    pub key_name: String,
    pub is_static: bool,
    pub linked_to: String,
    pub linked_to_module: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub hp: Hp,
    /// Opening clip / DWK set (1 or 2) of the last open; 0 = never opened.
    pub side: u8,
    /// Seconds of the opening animation left; the walkmesh stays closed
    /// until it reaches 0 (doors.rs). A save taken mid transition loads
    /// settled.
    pub opening: f32,
    /// Whether the door was locked when its area loaded (spawn.rs). Set once at load
    /// and saved with the door. A door locked at load that a script has since
    /// unlocked opens on use instead of talking (lib.rs use_object; heuristic, see
    /// docs/notes/world.md). None only in a save from before the field existed: the
    /// load takes it from the template (save.rs World::load).
    pub locked_at_load: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaceableState {
    pub useable: bool,
    pub locked: bool,
    pub open: bool,
    pub has_inventory: bool,
    pub is_static: bool,
    pub model: Option<String>,
    #[serde(default)]
    pub hp: Hp,
}

/// Hit points of a door or placeable (bash.rs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hp {
    pub current: i32,
    pub max: i32,
    pub hardness: i32,
    /// Can be attacked at all: not plot, not static, (doors) not NotBlastable.
    pub bashable: bool,
    /// Brought to 0 HP: a door stays open for good.
    pub destroyed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CreatureState {
    pub appearance: u32,
    pub gender: i32,
    pub racial_type: i32,
    pub hp: i32,
    pub max_hp: i32,
    pub force: i32,
    pub max_force: i32,
    pub dead: bool,
    /// Knocked unconscious (a party member at 0 HP, not dead): it takes no
    /// orders and is not a target. Only EffectDeath (which kills it) or a raise
    /// (heal above 0, resurrection, the battle-end revive)
    /// changes it; see party.rs.
    #[serde(default)]
    pub down: bool,
    /// Seconds of battle-free time left before a downed party member gets up.
    #[serde(default)]
    pub get_up_in: f32,
    /// Killed outright (EffectDeath, from this build on): a slain creature does
    /// not come back. Saves written before this flag existed read it as false
    /// (serde default), so a party member killed by EffectDeath in an older save
    /// loads knocked out, like any other dead party member at 0 HP
    /// (party.rs settle_loaded_party).
    #[serde(default)]
    pub slain: bool,
    /// Fractions of a point (HP, Force) carried between regeneration steps
    /// (regen.rs).
    #[serde(default)]
    pub regen_carry: [f32; 2],
    pub min1hp: bool,
    pub is_pc: bool,
    pub good_evil: i32,
    /// STANDARD_FACTION_* (5 = neutral).
    pub faction: i32,
    pub abilities: [u8; 6],
    pub classes: Vec<(i32, i32)>,
    pub walk_speed: f32,
    pub run_speed: f32,
    pub radius: f32,
    pub perception_range: f32,
    pub body_model: Option<String>,
    pub body_texture: Option<String>,
    pub head_model: Option<String>,
    /// UTC Hologram (K2): drawn as a blue projection (security-console logs).
    #[serde(default)]
    pub hologram: bool,
    /// UTC NotReorienting (turrets, static emplacements): never turns to face
    /// its conversation partner (Object::turns_in_dialog). Combat, bashing and
    /// aim still turn it.
    #[serde(default)]
    pub not_reorienting: bool,
    /// Inventory slot index to item object.
    ///
    /// Slots 4/5 always hold the active weapon set (combat, models and
    /// scripts read them); the inactive set sits in 18/19.
    pub equipped: HashMap<i32, ObjectId>,
    /// Which weapon set is active: 0 or 1 (set 1 is the loadout that lives
    /// in slots 4/5 while this is 0, and in 18/19 while it is 1).
    pub weapon_set: u8,
    /// World time until which the creature is busy drawing a weapon (after
    /// an equip, unequip or weapon-set swap in its hands); attacks wait.
    pub equip_until: f64,
    pub gold: i32,
    pub xp: i32,
    pub seen: HashSet<ObjectId>,
    pub listening: bool,
    pub listen_patterns: Vec<(String, i32)>,
    /// In stealth mode (stealth.rs).
    pub stealth: bool,
    /// The encounter that spawned this creature (encounter.rs), if any.
    pub encounter: Option<ObjectId>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TriggerState {
    /// World-space polygon (game coordinates).
    pub polygon: Vec<[f32; 3]>,
    pub linked_to: Option<String>,
    pub linked_to_module: Option<String>,
    pub is_transition: bool,
    pub inside: HashSet<ObjectId>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemState {
    pub base_item: u32,
    pub stack_size: i32,
    pub possessor: Option<ObjectId>,
    pub plot: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundState {
    pub active: bool,
    pub looping: bool,
    pub sounds: Vec<String>,
}

/// Kind-specific state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Detail {
    None,
    Creature(Box<CreatureState>),
    Door(DoorState),
    Placeable(PlaceableState),
    Trigger(TriggerState),
    Item(ItemState),
    Sound(SoundState),
    /// GIT encounter (encounter.rs).
    Encounter(Box<crate::encounter::EncounterState>),
}

/// An effect applied to an object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedEffect {
    pub handle: u32,
    pub duration_type: i32,
    pub remaining: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Object {
    pub id: ObjectId,
    pub kind: ObjectKind,
    pub tag: String,
    pub template: String,
    pub name: String,
    pub position: Vec3,
    /// Rotation about +Z of a +Y-facing model, radians (k2-data convention).
    pub bearing: f32,
    /// Event label (e.g. ScriptHeartbeat, OnUsed) to script resref.
    pub scripts: HashMap<String, String>,
    pub conversation: Option<String>,
    pub local_bools: HashMap<i32, bool>,
    pub local_numbers: HashMap<i32, i32>,
    pub plot: bool,
    pub commandable: bool,
    pub destroyed: bool,
    pub actions: VecDeque<Action>,
    /// Bumped by ClearAllActions so a running step knows its queue was reset.
    pub action_epoch: u32,
    pub inventory: Vec<ObjectId>,
    pub effects: Vec<AppliedEffect>,
    pub detail: Detail,
    /// Seconds until the next heartbeat.
    pub heartbeat_in: f32,
    /// Mine / trap data (trap triggers and set mines). See traps.rs.
    #[serde(default)]
    pub trap: Option<Box<crate::traps::TrapState>>,
    /// The creature a puppet or summoned creature follows (ActionFollowOwner,
    /// GetPUPOwner). Nothing in the engine sets it yet: AssignPUP and
    /// SpawnAvailablePUP are still stubs.
    #[serde(default)]
    pub owner: Option<ObjectId>,
}

impl Object {
    pub fn new(id: ObjectId, kind: ObjectKind) -> Self {
        Self {
            id,
            kind,
            tag: String::new(),
            template: String::new(),
            name: String::new(),
            position: Vec3::ZERO,
            bearing: 0.0,
            scripts: HashMap::new(),
            conversation: None,
            local_bools: HashMap::new(),
            local_numbers: HashMap::new(),
            plot: false,
            commandable: true,
            destroyed: false,
            actions: VecDeque::new(),
            action_epoch: 0,
            inventory: Vec::new(),
            effects: Vec::new(),
            detail: Detail::None,
            heartbeat_in: 6.0,
            trap: None,
            owner: None,
        }
    }

    pub fn script(&self, label: &str) -> Option<&str> {
        self.scripts.get(label).map(String::as_str).filter(|s| !s.is_empty())
    }

    pub fn creature(&self) -> Option<&CreatureState> {
        match &self.detail {
            Detail::Creature(c) => Some(c),
            _ => None,
        }
    }

    /// A living creature that turns to face its conversation partner. Placeables
    /// and doors never do, and neither does a NotReorienting creature. Combat,
    /// bashing and aim still turn it.
    pub fn turns_in_dialog(&self) -> bool {
        self.creature().is_some_and(|c| !c.dead && !c.not_reorienting)
    }

    pub fn creature_mut(&mut self) -> Option<&mut CreatureState> {
        match &mut self.detail {
            Detail::Creature(c) => Some(c),
            _ => None,
        }
    }

    pub fn door(&self) -> Option<&DoorState> {
        match &self.detail {
            Detail::Door(d) => Some(d),
            _ => None,
        }
    }

    pub fn door_mut(&mut self) -> Option<&mut DoorState> {
        match &mut self.detail {
            Detail::Door(d) => Some(d),
            _ => None,
        }
    }

    pub fn placeable(&self) -> Option<&PlaceableState> {
        match &self.detail {
            Detail::Placeable(p) => Some(p),
            _ => None,
        }
    }

    pub fn placeable_mut(&mut self) -> Option<&mut PlaceableState> {
        match &mut self.detail {
            Detail::Placeable(p) => Some(p),
            _ => None,
        }
    }

    pub fn trigger(&self) -> Option<&TriggerState> {
        match &self.detail {
            Detail::Trigger(t) => Some(t),
            _ => None,
        }
    }

    pub fn encounter(&self) -> Option<&crate::encounter::EncounterState> {
        match &self.detail {
            Detail::Encounter(e) => Some(e),
            _ => None,
        }
    }

    pub fn encounter_mut(&mut self) -> Option<&mut crate::encounter::EncounterState> {
        match &mut self.detail {
            Detail::Encounter(e) => Some(e),
            _ => None,
        }
    }

    pub fn item(&self) -> Option<&ItemState> {
        match &self.detail {
            Detail::Item(i) => Some(i),
            _ => None,
        }
    }

    pub fn item_mut(&mut self) -> Option<&mut ItemState> {
        match &mut self.detail {
            Detail::Item(i) => Some(i),
            _ => None,
        }
    }

    /// NWScript facing in degrees (0 = +X, counter-clockwise).
    pub fn facing_deg(&self) -> f32 {
        bearing_to_facing(self.bearing)
    }
}

/// k2-data bearing (radians, +Y-facing model) to NWScript facing (degrees, 0 = +X).
pub fn bearing_to_facing(bearing: f32) -> f32 {
    (bearing.to_degrees() + 90.0).rem_euclid(360.0)
}

/// NWScript facing (degrees, 0 = +X) to k2-data bearing (radians).
pub fn facing_to_bearing(facing: f32) -> f32 {
    (facing - 90.0).to_radians()
}

/// Bearing that makes an object at `from` face `to`.
pub fn bearing_towards(from: Vec3, to: Vec3) -> Option<f32> {
    let d = to - from;
    if d.x.abs() + d.y.abs() < 1e-4 {
        return None;
    }
    Some(k2_data::module::bearing_from_dir(d.x, d.y))
}

