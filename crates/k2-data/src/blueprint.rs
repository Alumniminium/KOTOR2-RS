//! Blueprints: UTC, UTP, UTD, UTT, UTW, UTS, UTI (the parts the engine uses).

use k2_res::Resources;
use rakata_core::ResourceType;
use rakata_formats::gff::GffStruct;

use crate::gff::{GffExt, LocString};
use crate::tables::{CreatureModels, Tables};
use crate::{DataError, Result, read_gff};

/// Equip slot bit for body armor in Equip_ItemList struct ids.
pub const SLOT_BODY: i32 = 0x2;
/// Right-hand weapon slot bit.
pub const SLOT_RIGHT_HAND: i32 = 0x10;
/// Left-hand weapon slot bit.
pub const SLOT_LEFT_HAND: i32 = 0x20;

fn load(res: &Resources, resref: &str, ty: ResourceType) -> Result<GffStruct> {
    Ok(read_gff(res, resref, ty)?.root)
}

/// Named event scripts, label to resref, only non-empty ones.
fn scripts(s: &GffStruct, labels: &[&str]) -> Vec<(String, String)> {
    labels.iter().filter_map(|l| s.resref(l).map(|r| ((*l).to_owned(), r))).collect()
}

/// Creature blueprint (UTC).
#[derive(Debug, Clone, Default)]
pub struct Creature {
    pub resref: String,
    pub tag: String,
    pub first_name: LocString,
    pub last_name: LocString,
    pub appearance: u32,
    pub gender: u32,
    pub portrait: u32,
    pub body_variation: u32,
    pub texture_variation: u32,
    pub conversation: Option<String>,
    pub faction: u32,
    pub is_pc: bool,
    pub plot: bool,
    pub min1hp: bool,
    pub party_interact: bool,
    pub hologram: bool,
    pub will_not_render: bool,
    pub not_reorienting: bool,
    pub walk_rate: i32,
    pub hit_points: i32,
    pub current_hit_points: i32,
    pub max_hit_points: i32,
    pub force_points: i32,
    pub current_force: i32,
    pub abilities: [u8; 6],
    pub natural_ac: i32,
    pub good_evil: i32,
    pub perception_range: u32,
    pub sound_set: u32,
    /// (class id, level)
    pub classes: Vec<(i32, i32)>,
    /// (slot bitmask, item resref)
    pub equipment: Vec<(i32, String)>,
    pub inventory: Vec<String>,
    /// Event label (ScriptHeartbeat, ScriptSpawn, ...) to script resref.
    pub scripts: Vec<(String, String)>,
}

const UTC_SCRIPTS: &[&str] = &[
    "ScriptHeartbeat", "ScriptOnNotice", "ScriptSpellAt", "ScriptAttacked", "ScriptDamaged",
    "ScriptDisturbed", "ScriptEndRound", "ScriptEndDialogu", "ScriptDialogue", "ScriptSpawn",
    "ScriptRested", "ScriptDeath", "ScriptUserDefine", "ScriptOnBlocked",
];

impl Creature {
    pub fn load(res: &Resources, resref: &str) -> Result<Self> {
        Ok(Self::from_gff(&load(res, resref, ResourceType::Utc)?, resref))
    }

    pub fn from_gff(s: &GffStruct, resref: &str) -> Self {
        let ab = |l| s.int(l).unwrap_or(10).clamp(0, 255) as u8;
        Self {
            resref: resref.to_ascii_lowercase(),
            tag: s.string_or_empty("Tag"),
            first_name: s.locstring("FirstName"),
            last_name: s.locstring("LastName"),
            appearance: s.uint("Appearance_Type").unwrap_or(0),
            gender: s.uint("Gender").unwrap_or(0),
            portrait: s.uint("PortraitId").unwrap_or(0),
            body_variation: s.uint("BodyVariation").unwrap_or(0),
            texture_variation: s.uint("TextureVar").unwrap_or(1),
            conversation: s.resref("Conversation"),
            faction: s.uint("FactionID").unwrap_or(0),
            is_pc: s.flag("IsPC"),
            plot: s.flag("Plot"),
            min1hp: s.flag("Min1HP"),
            party_interact: s.flag("PartyInteract"),
            hologram: s.flag("Hologram"),
            will_not_render: s.flag("WillNotRender"),
            not_reorienting: s.flag("NotReorienting"),
            walk_rate: s.int_or("WalkRate", 0),
            hit_points: s.int_or("HitPoints", 1),
            current_hit_points: s.int_or("CurrentHitPoints", 1),
            max_hit_points: s.int_or("MaxHitPoints", 1),
            force_points: s.int_or("ForcePoints", 0),
            current_force: s.int_or("CurrentForce", 0),
            abilities: [ab("Str"), ab("Dex"), ab("Con"), ab("Int"), ab("Wis"), ab("Cha")],
            natural_ac: s.int_or("NaturalAC", 0),
            good_evil: s.int_or("GoodEvil", 50),
            perception_range: s.uint("PerceptionRange").unwrap_or(0),
            sound_set: s.uint("SoundSetFile").unwrap_or(0),
            classes: s.list("ClassList").iter().map(|c| (c.int_or("Class", 0), c.int_or("ClassLevel", 1))).collect(),
            equipment: s
                .list("Equip_ItemList")
                .iter()
                .filter_map(|e| e.resref("EquippedRes").map(|r| (e.struct_id, r)))
                .collect(),
            inventory: s.list("ItemList").iter().filter_map(|e| e.resref("InventoryRes")).collect(),
            scripts: scripts(s, UTC_SCRIPTS),
        }
    }

    pub fn equipped(&self, slot: i32) -> Option<&str> {
        self.equipment.iter().find(|(s, _)| *s == slot).map(|(_, r)| r.as_str())
    }

    pub fn script(&self, label: &str) -> Option<&str> {
        self.scripts.iter().find(|(l, _)| l == label).map(|(_, r)| r.as_str())
    }

    /// Resolve body/head models from appearance.2da, honouring equipped
    /// body armor (baseitems.2da bodyvar and the item's TextureVar).
    pub fn models(&self, res: &Resources, tables: &Tables) -> Result<CreatureModels> {
        let (mut body_var, mut tex_var) = ('a', 1u32);
        if let Some(armor) = self.equipped(SLOT_BODY)
            && let Ok(item) = Item::load(res, armor) {
                let base = tables.get(res, "baseitems")?;
                if let Some(v) = base.str(item.base_item as usize, "bodyvar").and_then(|s| s.chars().next()) {
                    body_var = v.to_ascii_lowercase();
                }
                tex_var = item.texture_variation.max(1);
            }
        tables.creature_models(res, self.appearance, body_var, tex_var)
    }
}

/// Item blueprint (UTI), only what models and equipment need.
#[derive(Debug, Clone, Default)]
pub struct Item {
    pub resref: String,
    pub tag: String,
    pub name: LocString,
    pub base_item: u32,
    pub model_variation: u32,
    pub body_variation: u32,
    pub texture_variation: u32,
    pub stack_size: u32,
    pub charges: u32,
    pub cost: u32,
    pub plot: bool,
}

impl Item {
    pub fn load(res: &Resources, resref: &str) -> Result<Self> {
        let s = load(res, resref, ResourceType::Uti)?;
        Ok(Self {
            resref: resref.to_ascii_lowercase(),
            tag: s.string_or_empty("Tag"),
            name: s.locstring("LocalizedName"),
            base_item: s.uint("BaseItem").unwrap_or(0),
            model_variation: s.uint("ModelVariation").unwrap_or(1),
            body_variation: s.uint("BodyVariation").unwrap_or(0),
            texture_variation: s.uint("TextureVar").unwrap_or(1),
            stack_size: s.uint("StackSize").unwrap_or(1),
            charges: s.uint("Charges").unwrap_or(0),
            cost: s.uint("Cost").unwrap_or(0),
            plot: s.flag("Plot"),
        })
    }
}

/// Trap fields of a UTP or UTD (TrapFlag and the keys beside it).
#[derive(Debug, Clone, Default)]
pub struct TrapFields {
    pub flag: bool,
    pub trap_type: u32,
    pub detectable: bool,
    pub detect_dc: i32,
    pub disarmable: bool,
    pub disarm_dc: i32,
    pub one_shot: bool,
    pub faction: u32,
}

/// The trap fields of a UTD or UTP struct (public for the synthetic GFF test).
pub fn trap_fields(s: &GffStruct) -> TrapFields {
    TrapFields {
        flag: s.flag("TrapFlag"),
        trap_type: s.uint("TrapType").unwrap_or(0),
        detectable: s.flag("TrapDetectable"),
        detect_dc: s.int_or("TrapDetectDC", 0),
        disarmable: s.flag("TrapDisarmable"),
        disarm_dc: s.int_or("DisarmDC", 0),
        one_shot: s.flag("TrapOneShot"),
        faction: s.uint("Faction").unwrap_or(0),
    }
}

/// Placeable blueprint (UTP).
#[derive(Debug, Clone, Default)]
pub struct Placeable {
    pub resref: String,
    pub tag: String,
    pub name: LocString,
    pub appearance: u32,
    pub useable: bool,
    pub is_static: bool,
    pub party_interact: bool,
    pub has_inventory: bool,
    pub locked: bool,
    pub key_required: bool,
    pub key_name: String,
    pub open_lock_dc: i32,
    pub conversation: Option<String>,
    pub animation_state: u32,
    pub plot: bool,
    pub hp: i32,
    pub current_hp: i32,
    /// Damage subtracted from every hit (bashing).
    pub hardness: i32,
    pub faction: u32,
    pub trap: TrapFields,
    pub inventory: Vec<String>,
    pub scripts: Vec<(String, String)>,
}

const UTP_SCRIPTS: &[&str] = &[
    "OnClosed", "OnDamaged", "OnDeath", "OnDisarm", "OnHeartbeat", "OnInvDisturbed", "OnLock",
    "OnMeleeAttacked", "OnOpen", "OnSpellCastAt", "OnTrapTriggered", "OnUnlock", "OnUserDefined",
    "OnUsed", "OnEndDialogue", "OnFailToOpen",
];

impl Placeable {
    pub fn load(res: &Resources, resref: &str) -> Result<Self> {
        let s = load(res, resref, ResourceType::Utp)?;
        Ok(Self {
            resref: resref.to_ascii_lowercase(),
            tag: s.string_or_empty("Tag"),
            name: s.locstring("LocName"),
            appearance: s.uint("Appearance").unwrap_or(0),
            useable: s.flag("Useable"),
            is_static: s.flag("Static"),
            party_interact: s.flag("PartyInteract"),
            has_inventory: s.flag("HasInventory"),
            locked: s.flag("Locked"),
            key_required: s.flag("KeyRequired"),
            key_name: s.string_or_empty("KeyName"),
            open_lock_dc: s.int_or("OpenLockDC", 0),
            conversation: s.resref("Conversation"),
            animation_state: s.uint("AnimationState").unwrap_or(0),
            plot: s.flag("Plot"),
            hp: s.int_or("HP", 1),
            current_hp: s.int_or("CurrentHP", 1),
            hardness: s.int_or("Hardness", 0),
            faction: s.uint("Faction").unwrap_or(0),
            trap: trap_fields(&s),
            inventory: s.list("ItemList").iter().filter_map(|e| e.resref("InventoryRes")).collect(),
            scripts: scripts(&s, UTP_SCRIPTS),
        })
    }

    pub fn model(&self, res: &Resources, tables: &Tables) -> Result<String> {
        tables
            .placeable_model(res, self.appearance)?
            .ok_or_else(|| DataError::Missing(format!("placeables.2da row {} modelname", self.appearance)))
    }
}

/// Door blueprint (UTD).
#[derive(Debug, Clone, Default)]
pub struct Door {
    pub resref: String,
    pub tag: String,
    pub name: LocString,
    /// genericdoors.2da row.
    pub generic_type: u32,
    pub is_static: bool,
    pub locked: bool,
    pub lockable: bool,
    pub key_required: bool,
    pub key_name: String,
    pub open_lock_dc: i32,
    pub open_state: u32,
    pub plot: bool,
    pub not_blastable: bool,
    pub hp: i32,
    pub current_hp: i32,
    /// Damage subtracted from every hit (bashing).
    pub hardness: i32,
    pub faction: u32,
    pub conversation: Option<String>,
    pub linked_to: String,
    pub linked_to_module: Option<String>,
    pub linked_to_flags: u32,
    pub trap: TrapFields,
    pub scripts: Vec<(String, String)>,
}

const UTD_SCRIPTS: &[&str] = &[
    "OnClick", "OnClosed", "OnDamaged", "OnDeath", "OnDisarm", "OnFailToOpen", "OnHeartbeat",
    "OnLock", "OnMeleeAttacked", "OnOpen", "OnSpellCastAt", "OnTrapTriggered", "OnUnlock",
    "OnUserDefined",
];

impl Door {
    pub fn load(res: &Resources, resref: &str) -> Result<Self> {
        let s = load(res, resref, ResourceType::Utd)?;
        Ok(Self {
            resref: resref.to_ascii_lowercase(),
            tag: s.string_or_empty("Tag"),
            name: s.locstring("LocName"),
            generic_type: s.uint("GenericType").unwrap_or(0),
            is_static: s.flag("Static"),
            locked: s.flag("Locked"),
            lockable: s.flag("Lockable"),
            key_required: s.flag("KeyRequired"),
            key_name: s.string_or_empty("KeyName"),
            open_lock_dc: s.int_or("OpenLockDC", 0),
            open_state: s.uint("OpenState").unwrap_or(0),
            plot: s.flag("Plot"),
            not_blastable: s.flag("NotBlastable"),
            hp: s.int_or("HP", 1),
            current_hp: s.int_or("CurrentHP", 1),
            hardness: s.int_or("Hardness", 0),
            faction: s.uint("Faction").unwrap_or(0),
            conversation: s.resref("Conversation"),
            linked_to: s.string_or_empty("LinkedTo"),
            linked_to_module: s.resref("LinkedToModule"),
            linked_to_flags: s.uint("LinkedToFlags").unwrap_or(0),
            trap: trap_fields(&s),
            scripts: scripts(&s, UTD_SCRIPTS),
        })
    }

    pub fn model(&self, res: &Resources, tables: &Tables) -> Result<String> {
        tables
            .door_model(res, self.generic_type)?
            .ok_or_else(|| DataError::Missing(format!("genericdoors.2da row {} modelname", self.generic_type)))
    }
}

/// Trigger blueprint (UTT).
#[derive(Debug, Clone, Default)]
pub struct Trigger {
    pub resref: String,
    pub tag: String,
    pub name: LocString,
    /// 0 generic, 1 area transition, 2 trap.
    pub kind: i32,
    pub cursor: u32,
    pub highlight_height: f32,
    pub linked_to: String,
    pub linked_to_module: Option<String>,
    pub linked_to_flags: u32,
    pub trap_flag: bool,
    pub trap_type: u32,
    pub trap_detectable: bool,
    pub trap_detect_dc: i32,
    pub trap_disarmable: bool,
    pub disarm_dc: i32,
    pub trap_one_shot: bool,
    pub faction: u32,
    pub scripts: Vec<(String, String)>,
}

const UTT_SCRIPTS: &[&str] = &[
    "OnClick", "OnDisarm", "OnTrapTriggered", "ScriptHeartbeat", "ScriptOnEnter", "ScriptOnExit",
    "ScriptUserDefine",
];

impl Trigger {
    pub fn load(res: &Resources, resref: &str) -> Result<Self> {
        let s = load(res, resref, ResourceType::Utt)?;
        Ok(Self {
            resref: resref.to_ascii_lowercase(),
            tag: s.string_or_empty("Tag"),
            name: s.locstring("LocalizedName"),
            kind: s.int_or("Type", 0),
            cursor: s.uint("Cursor").unwrap_or(0),
            highlight_height: s.float_or("HighlightHeight", 0.0),
            linked_to: s.string_or_empty("LinkedTo"),
            linked_to_module: s.resref("LinkedToModule"),
            linked_to_flags: s.uint("LinkedToFlags").unwrap_or(0),
            trap_flag: s.flag("TrapFlag"),
            trap_type: s.uint("TrapType").unwrap_or(0),
            trap_detectable: s.flag("TrapDetectable"),
            trap_detect_dc: s.int_or("TrapDetectDC", 0),
            trap_disarmable: s.flag("TrapDisarmable"),
            disarm_dc: s.int_or("DisarmDC", 0),
            trap_one_shot: s.flag("TrapOneShot"),
            faction: s.uint("Faction").unwrap_or(0),
            scripts: scripts(&s, UTT_SCRIPTS),
        })
    }

    pub fn is_transition(&self) -> bool {
        self.kind == 1
    }
}

/// Waypoint blueprint (UTW).
#[derive(Debug, Clone, Default)]
pub struct Waypoint {
    pub resref: String,
    pub tag: String,
    pub name: LocString,
    pub linked_to: String,
    pub has_map_note: bool,
    pub map_note: LocString,
}

impl Waypoint {
    pub fn load(res: &Resources, resref: &str) -> Result<Self> {
        let s = load(res, resref, ResourceType::Utw)?;
        Ok(Self {
            resref: resref.to_ascii_lowercase(),
            tag: s.string_or_empty("Tag"),
            name: s.locstring("LocalizedName"),
            linked_to: s.string_or_empty("LinkedTo"),
            has_map_note: s.flag("HasMapNote"),
            map_note: s.locstring("MapNote"),
        })
    }
}

/// Sound blueprint (UTS).
#[derive(Debug, Clone, Default)]
pub struct SoundBlueprint {
    pub resref: String,
    pub tag: String,
    pub active: bool,
    pub continuous: bool,
    pub looping: bool,
    pub positional: bool,
    pub random: bool,
    pub random_position: bool,
    pub volume: u32,
    pub volume_variation: u32,
    pub pitch_variation: f32,
    pub min_distance: f32,
    pub max_distance: f32,
    pub elevation: f32,
    /// Milliseconds between plays.
    pub interval: u32,
    pub interval_variation: u32,
    pub sounds: Vec<String>,
}

impl SoundBlueprint {
    pub fn load(res: &Resources, resref: &str) -> Result<Self> {
        let s = load(res, resref, ResourceType::Uts)?;
        Ok(Self {
            resref: resref.to_ascii_lowercase(),
            tag: s.string_or_empty("Tag"),
            active: s.flag("Active"),
            continuous: s.flag("Continuous"),
            looping: s.flag("Looping"),
            positional: s.flag("Positional"),
            random: s.flag("Random"),
            random_position: s.flag("RandomPosition"),
            volume: s.uint("Volume").unwrap_or(127),
            volume_variation: s.uint("VolumeVrtn").unwrap_or(0),
            pitch_variation: s.float_or("PitchVariation", 0.0),
            min_distance: s.float_or("MinDistance", 1.0),
            max_distance: s.float_or("MaxDistance", 10.0),
            elevation: s.float_or("Elevation", 0.0),
            interval: s.uint("Interval").unwrap_or(0),
            interval_variation: s.uint("IntervalVrtn").unwrap_or(0),
            sounds: s.list("Sounds").iter().filter_map(|e| e.resref("Sound")).collect(),
        })
    }
}

/// One CreatureList entry of an encounter blueprint (EncounterCreature).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EncounterCreature {
    /// UTC blueprint to spawn.
    pub resref: String,
    pub appearance: i32,
    /// Challenge rating (stored on the entry so the CR needs no UTC read).
    pub cr: f32,
    /// Only one of this creature may be alive from the encounter at a time.
    pub single_spawn: bool,
    /// K2 field not in the Aurora format document: copies to spawn
    /// regardless of the CR budget.
    pub guaranteed_count: i32,
}

/// Encounter blueprint (UTE). Geometry and spawn points are per instance
/// (`module::EncounterInstance`).
#[derive(Debug, Clone, Default)]
pub struct Encounter {
    pub resref: String,
    pub tag: String,
    pub name: LocString,
    pub active: bool,
    /// encdifficulty.2da row.
    pub difficulty_index: i32,
    /// Only creatures hostile to this faction fire the encounter.
    pub faction: u32,
    /// Most creatures alive from this encounter at once (1..8).
    pub max_creatures: i32,
    /// Recommended creature count (not a floor).
    pub rec_creatures: i32,
    /// Only party members fire the encounter.
    pub player_only: bool,
    pub reset: bool,
    /// Seconds from the last spawned creature's death to the next spawn cycle.
    pub reset_time: i32,
    /// Spawn cycles after the first; -1 = unlimited.
    pub respawns: i32,
    /// SpawnOption: 0 = continuous (refill as creatures die), 1 = single shot.
    pub spawn_option: i32,
    pub creatures: Vec<EncounterCreature>,
    /// Event label (OnEntered, OnExit, OnExhausted, OnHeartbeat,
    /// OnUserDefined) to script resref.
    pub scripts: Vec<(String, String)>,
}

const UTE_SCRIPTS: &[&str] = &["OnEntered", "OnExit", "OnExhausted", "OnHeartbeat", "OnUserDefined"];

impl Encounter {
    pub fn load(res: &Resources, resref: &str) -> Result<Self> {
        Ok(Self::from_gff(&load(res, resref, ResourceType::Ute)?, resref))
    }

    pub fn from_gff(s: &GffStruct, resref: &str) -> Self {
        Self {
            resref: resref.to_ascii_lowercase(),
            tag: s.string_or_empty("Tag"),
            name: s.locstring("LocalizedName"),
            // Absent-field defaults as rakata's UTE page gives them (Active 1,
            // Faction 1, ResetTime 60).
            active: !s.has("Active") || s.flag("Active"),
            difficulty_index: s.int_or("DifficultyIndex", 0),
            faction: s.uint("Faction").unwrap_or(1),
            max_creatures: s.int_or("MaxCreatures", 8),
            rec_creatures: s.int_or("RecCreatures", 2),
            player_only: s.flag("PlayerOnly"),
            reset: s.flag("Reset"),
            reset_time: s.int_or("ResetTime", 60),
            respawns: s.int_or("Respawns", 0),
            spawn_option: s.int_or("SpawnOption", 0),
            creatures: s
                .list("CreatureList")
                .iter()
                .filter_map(|c| {
                    Some(EncounterCreature {
                        resref: c.resref("ResRef").filter(|r| !r.is_empty())?.to_ascii_lowercase(),
                        appearance: c.int_or("Appearance", 0),
                        cr: c.float_or("CR", 0.0),
                        single_spawn: c.flag("SingleSpawn"),
                        guaranteed_count: c.int_or("GuaranteedCount", 0),
                    })
                })
                .collect(),
            scripts: scripts(s, UTE_SCRIPTS),
        }
    }

    pub fn script(&self, label: &str) -> Option<&str> {
        self.scripts.iter().find(|(l, _)| l == label).map(|(_, r)| r.as_str())
    }
}
