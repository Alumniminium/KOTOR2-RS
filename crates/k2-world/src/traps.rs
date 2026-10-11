//! Mines and traps.
//!
//! A trap is a trigger (UTT TrapFlag = 1, or a mine set by a creature)
//! carrying a [`TrapState`] on its object. traps.2da row = TrapType: the
//! trap script (k_trp_generic for every mine), DC modifiers, the model and
//! the explosion sound.
//!
//! - Detection: every [`DETECT_INTERVAL`] s each party member within
//!   [`DETECT_RANGE`] of an undetected, detectable trap rolls d20 + Awareness
//!   against the trap's detect DC. Detection is per creature
//!   (GetTrapDetectedBy); the party sees what any member detected. Mines a
//!   creature sets are detected by its side from the start.
//! - Triggering: a living creature entering the polygon whose faction is
//!   hostile to the trap's faction sets it off (GIT traps are Hostile_1 and
//!   go off under the party; player mines are Friendly_1 and go off under
//!   enemies). The trap's OnTrapTriggered script runs if set, else the
//!   traps.2da trapscript, with OBJECT_SELF = the trap. One-shot traps are
//!   destroyed afterwards.
//! - Blast: a detonation also breaks doors within [`BLAST_RADIUS`] whose UTD
//!   has NotBlastable = 0, Plot or not (k_trp_generic only damages
//!   creatures; the engine blasts doors itself). The 001ebo engine-room door
//!   (Plot, NotBlastable 0, "You can use a mine to open this door. Once you
//!   plant the mine, back away from the door to avoid taking damage") opens
//!   this way.
//! - Breaching: a mine set within [`BREACH_RANGE`] of a closed blastable door
//!   is a breaching charge and goes off after [`BREACH_FUSE`] s on its own.
//!   This is inferred from the eng_door text above; nothing in the scripts
//!   detonates it.
//! - Disarm / Recover: d20 + Demolitions against the disarm DC (Recover is
//!   DC + [`RECOVER_DC_BONUS`] and gives the mine item back). Failing by 5 or
//!   more sets the trap off. A creature's own mine (creator = user) comes
//!   back without a roll (inferred; other party members roll).
//! - Trapped doors and placeables (UTD / UTP TrapFlag): the same TrapState on
//!   the door or placeable object. A player use that opens or uses an
//!   unlocked object sets an armed trap off first (inferred: nothing in the
//!   install says whether the open still goes through, so it does). A locked
//!   object's trap does not go off while it is locked (inferred, as doors
//!   do); a locked placeable's use still runs OnUsed and its conversation
//!   (shipped templates rely on it). A trapped transition door goes off
//!   before the transit. Bashing a door or placeable open (destroying it)
//!   sets its trap off, locked or not, because the blow does not need the
//!   lock to go (inferred from the NWN Bash page,
//!   https://nwn.fandom.com/wiki/Bash, which says a trap on a bashed object is
//!   set off by the blow; read from a search summary, and KotOR is not
//!   checked). They are never destroyed by a trap or a disarm and never blast
//!   doors; one-shot ones just stop being armed. Scripted opens
//!   (ActionOpenDoor) do not trip them.
//! - Flag Trap (SUBSKILL_FLAGTRAP 100): a known trap becomes visible to every
//!   creature (GetTrapFlagged). Examine Trap (102): a trigger trap gets TLK
//!   1517 with its traps.2da name; a door or placeable trap gets the traps.2da
//!   name alone (no original line names one). A party member stepping on a
//!   mine gets TLK 1461.
//! - Recover Mine: "Recover Mine: success" (TLK 1531 and 1392) and the
//!   item-received line for the kit both go out (inferred: both are shown).
//! - Scripted DetonateMine plays the explosion; a trigger mine is removed
//!   whatever its one_shot or creator flags, a door or placeable is only
//!   disabled, and a plain trigger with no trap state is left alone. The
//!   damage script only runs if the script ExecuteScripts it.

use std::collections::{BTreeSet, HashMap};

use glam::Vec3;
use k2_data::GffExt;
use k2_script::{ObjectId, OBJECT_INVALID, Value};
use rakata_core::ResourceType;
use serde::{Deserialize, Serialize};

use crate::object::{Detail, Object, ObjectKind, TriggerState};
use crate::{ScriptCtx, World, WorldEvent};

pub const SKILL_DEMOLITIONS: usize = 1;
pub const SKILL_AWARENESS: usize = 3;
/// nwscript SUBSKILL_* for ActionUseSkill(SKILL_DEMOLITIONS, trap). Disable
/// has no constant; it is the default 0.
const SUBSKILL_RECOVER_TRAP: i32 = 101;
const SUBSKILL_FLAG_TRAP: i32 = 100;
const SUBSKILL_EXAMINE_TRAP: i32 = 102;
/// TLK 1517 "This mine appears to be a: <CUSTOM0>".
const TLK_EXAMINE_TRAP: i32 = 1517;
/// TLK 1529 "Disable Mine", 1531 "Recover Mine" (action labels).
const TLK_DISABLE_MINE: i32 = 1529;
const TLK_RECOVER_MINE: i32 = 1531;
/// TLK 37251 "Mine successfully laid." (Set Mine feedback).
const TLK_MINE_LAID: i32 = 37251;
/// TLK 1461 "You triggered a Mine!" (a party member steps on a mine).
const TLK_TRIGGERED_MINE: i32 = 1461;
/// TLK 1392 "success", 1393 "failure" (skill result words).
const TLK_SUCCESS: i32 = 1392;
const TLK_FAILURE: i32 = 1393;
/// TLK 42132 "<CUSTOM0> successfully detects <CUSTOM1>: awareness <CUSTOM2>
/// vs. DC <CUSTOM3>", used for a trap detection (a guess: the line is a
/// DC-based detection, the same shape as the stealth one at 38023).
const TLK_DETECTS_DC: i32 = 42132;
/// Item property "Trap" (itempropdef.2da row 46); Subtype = traps.2da row.
const PROPERTY_TRAP: u32 = 46;
/// baseitems.2da Trap_Kit.
pub const BASE_ITEM_TRAP_KIT: u32 = 58;
pub const DETECT_INTERVAL: f32 = 2.0;
pub const DETECT_RANGE: f32 = 10.0;
pub const BLAST_RADIUS: f32 = 4.0;
pub const BREACH_RANGE: f32 = 3.5;
pub const BREACH_FUSE: f32 = 4.0;
pub const RECOVER_DC_BONUS: i32 = 5;
/// Disarming/recovering needs the user this close (KotOR walks there first).
pub const USE_RANGE: f32 = 3.0;
/// Half the side of a set mine's square trigger.
const MINE_HALF: f32 = 0.75;
/// STANDARD_FACTION_FRIENDLY_1 (the party) / HOSTILE_1.
const FACTION_PARTY: i32 = 2;
const FACTION_HOSTILE: i32 = 1;

/// Trap data on a trigger object (saved with it).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrapState {
    /// traps.2da row.
    pub trap_type: i32,
    pub detect_dc: i32,
    pub disarm_dc: i32,
    pub detectable: bool,
    pub disarmable: bool,
    pub one_shot: bool,
    /// STANDARD_FACTION_* the trap belongs to.
    pub faction: i32,
    /// Who set it (mines), else None.
    pub creator: Option<ObjectId>,
    /// Demolitions rank of the creator when set (GetOwnerDemolitionsSkill).
    pub owner_demolitions: i32,
    pub detected_by: BTreeSet<ObjectId>,
    /// SetTrapDisabled / disarmed: never goes off again.
    pub disabled: bool,
    /// Flag Trap: visible to every creature (GetTrapFlagged).
    pub flagged: bool,
    /// Creatures inside the polygon last tick.
    pub inside: BTreeSet<ObjectId>,
    /// Breaching charge: seconds until it goes off by itself.
    pub fuse: Option<f32>,
}

/// World-level trap and stealth state (saved).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemsState {
    pub detect_in: f32,
    pub last_disarmed: Option<ObjectId>,
    /// GetLastTrapDetected per creature.
    pub last_detected: HashMap<ObjectId, ObjectId>,
    pub stealth: crate::stealth::StealthXp,
    /// door template -> NotBlastable (UTD), cached.
    #[serde(skip)]
    pub not_blastable: HashMap<String, bool>,
}

/// A trap marker for the renderer: where it is and whose it is.
#[derive(Debug, Clone)]
pub struct TrapMarker {
    pub id: ObjectId,
    pub position: Vec3,
    /// Hostile to the party (red) or the party's own / neutral (green).
    pub hostile: bool,
    pub trap_type: i32,
    /// traps.2da model, e.g. v_mnfrag.
    pub model: Option<String>,
}

impl TrapState {
    /// From a UTT blueprint; None unless TrapFlag is set.
    pub fn from_trigger(bp: &k2_data::Trigger) -> Option<Self> {
        bp.trap_flag.then(|| TrapState {
            trap_type: bp.trap_type as i32,
            detect_dc: bp.trap_detect_dc,
            disarm_dc: bp.disarm_dc,
            detectable: bp.trap_detectable,
            disarmable: bp.trap_disarmable,
            one_shot: bp.trap_one_shot,
            faction: if bp.faction == 0 { FACTION_HOSTILE } else { bp.faction as i32 },
            ..Default::default()
        })
    }

    /// From the trap block of a UTD or UTP; None unless TrapFlag is set.
    pub fn from_fields(f: &k2_data::blueprint::TrapFields) -> Option<Self> {
        f.flag.then(|| TrapState {
            trap_type: f.trap_type as i32,
            detect_dc: f.detect_dc,
            disarm_dc: f.disarm_dc,
            detectable: f.detectable,
            disarmable: f.disarmable,
            one_shot: f.one_shot,
            faction: if f.faction == 0 { FACTION_HOSTILE } else { f.faction as i32 },
            ..Default::default()
        })
    }
}

fn centroid(poly: &[[f32; 3]]) -> Option<Vec3> {
    if poly.is_empty() {
        return None;
    }
    let s = poly.iter().fold(Vec3::ZERO, |a, p| a + Vec3::from(*p));
    Some(s / poly.len() as f32)
}

impl World {
    // ---------------------------------------------------------------
    // Queries
    // ---------------------------------------------------------------

    pub fn trap(&self, id: ObjectId) -> Option<&TrapState> {
        self.get(id).and_then(|o| o.trap.as_deref())
    }

    fn trap_mut(&mut self, id: ObjectId) -> Option<&mut TrapState> {
        self.get_mut(id).and_then(|o| o.trap.as_deref_mut())
    }

    /// Armed (not disabled) trap object.
    pub fn is_armed_trap(&self, id: ObjectId) -> bool {
        self.trap(id).is_some_and(|t| !t.disabled)
    }

    /// Where a trap sits: the trigger polygon's centre, else its position.
    pub fn trap_position(&self, id: ObjectId) -> Option<Vec3> {
        let o = self.get(id)?;
        match &o.detail {
            Detail::Trigger(t) => centroid(&t.polygon).or(Some(o.position)),
            _ => Some(o.position),
        }
    }

    /// Whether anyone in the party has detected the trap (or set it).
    pub fn party_detected(&self, id: ObjectId) -> bool {
        let Some(t) = self.trap(id) else { return false };
        t.flagged
            || t.creator.is_some_and(|c| self.party.contains(&c) || self.stashed_pc.as_ref().is_some_and(|p| p.id == c))
            || t.faction == FACTION_PARTY
            || t.detected_by.iter().any(|d| self.party.contains(d))
    }

    /// Whether `who` knows the trap: it flagged or detected it, or it is a
    /// party member and the party knows it.
    pub fn trap_known_to(&self, id: ObjectId, who: ObjectId) -> bool {
        let Some(t) = self.trap(id) else { return false };
        t.flagged || t.detected_by.contains(&who) || (self.party.contains(&who) && self.party_detected(id))
    }

    /// Trap triggers (mines and GIT traps) are removed when a one-shot trap
    /// goes off or is disarmed; trapped doors and placeables keep their object.
    fn is_trigger_trap(&self, id: ObjectId) -> bool {
        self.get(id).is_some_and(|o| o.kind == ObjectKind::Trigger)
    }

    /// Armed traps the party knows about, for map/scene markers.
    pub fn visible_traps(&self) -> Vec<TrapMarker> {
        let ids: Vec<ObjectId> = self.iter().filter(|o| o.trap.as_ref().is_some_and(|t| !t.disabled)).map(|o| o.id).collect();
        let mut out = Vec::new();
        for id in ids {
            if !self.party_detected(id) {
                continue;
            }
            let t = self.trap(id).expect("trap");
            let model = self.trap_row_str(t.trap_type, "model");
            out.push(TrapMarker {
                id,
                position: self.trap_position(id).unwrap_or_default(),
                hostile: t.faction != FACTION_PARTY,
                trap_type: t.trap_type,
                model,
            });
        }
        out
    }

    /// The nearest armed, party-detected trap within `range` of `from`.
    pub fn nearest_known_trap(&self, from: Vec3, range: f32) -> Option<ObjectId> {
        self.visible_traps()
            .into_iter()
            .map(|m| (m.position.truncate().distance(from.truncate()), m.id))
            .filter(|(d, _)| *d <= range)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id)
    }

    fn trap_row_str(&self, trap_type: i32, column: &str) -> Option<String> {
        let t = self.tables.get(&self.res, "traps").ok()?;
        t.str(usize::try_from(trap_type).ok()?, column).filter(|s| !s.is_empty() && *s != "****").map(str::to_ascii_lowercase)
    }

    fn trap_row_int(&self, trap_type: i32, column: &str) -> i32 {
        self.tables
            .get(&self.res, "traps")
            .ok()
            .and_then(|t| t.int(usize::try_from(trap_type).ok()?, column))
            .unwrap_or(0)
    }

    /// The traps.2da name of a trap type (dialog.tlk text).
    fn trap_name(&self, trap_type: i32) -> String {
        self.talk_str(self.trap_row_int(trap_type, "name"))
    }

    /// traps.2da row a mine item sets (its Trap property's subtype).
    pub fn mine_trap_type(&self, item: ObjectId) -> Option<i32> {
        let o = self.get(item).filter(|o| o.kind == ObjectKind::Item)?;
        let g = k2_data::read_gff(&self.res, &o.template, ResourceType::Uti).ok()?;
        g.root
            .list("PropertiesList")
            .iter()
            .find(|p| p.uint("PropertyName") == Some(PROPERTY_TRAP))
            .and_then(|p| p.int("Subtype"))
    }

    /// Mine items in the party inventory: (item, traps.2da row).
    pub fn party_mines(&self) -> Vec<(ObjectId, i32)> {
        self.party_inventory().into_iter().filter_map(|i| self.mine_trap_type(i).map(|t| (i, t))).collect()
    }

    fn door_blastable(&mut self, door: ObjectId) -> bool {
        let Some(o) = self.get(door) else { return false };
        if o.kind != ObjectKind::Door {
            return false;
        }
        let template = o.template.clone();
        if let Some(&nb) = self.systems.not_blastable.get(&template) {
            return !nb;
        }
        let nb = k2_data::Door::load(&self.res, &template).map(|bp| bp.not_blastable).unwrap_or(true);
        self.systems.not_blastable.insert(template, nb);
        !nb
    }

    /// Closed doors a blast at `at` would break.
    fn blastable_doors_near(&mut self, at: Vec3, radius: f32) -> Vec<ObjectId> {
        let near: Vec<ObjectId> = self
            .iter()
            .filter(|o| o.kind == ObjectKind::Door)
            .filter(|o| o.door().is_some_and(|d| !d.open && !d.hp.destroyed))
            .filter(|o| o.position.truncate().distance(at.truncate()) <= radius)
            .map(|o| o.id)
            .collect();
        near.into_iter().filter(|&d| self.door_blastable(d)).collect()
    }

    // ---------------------------------------------------------------
    // Creating traps
    // ---------------------------------------------------------------

    /// Creates a square mine trigger at `at` (SpawnMine and Set Mine).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_mine(&mut self, trap_type: i32, at: Vec3, detect_dc: i32, disarm_dc: i32, creator: Option<ObjectId>, faction: i32) -> ObjectId {
        let mut at = at;
        if let Some(h) = self.walk.ground(at.x, at.y, at.z + 1.0) {
            at.z = h.z;
        }
        let id = self.alloc_id();
        let mut o = Object::new(id, ObjectKind::Trigger);
        o.tag = format!("mine_{id}");
        o.position = at;
        o.name = self.talk_str(self.trap_row_int(trap_type, "name"));
        let h = MINE_HALF;
        let poly = vec![[at.x - h, at.y - h, at.z], [at.x + h, at.y - h, at.z], [at.x + h, at.y + h, at.z], [at.x - h, at.y + h, at.z]];
        o.detail = Detail::Trigger(TriggerState { polygon: poly, ..Default::default() });
        let owner_demolitions = creator.map_or(0, |c| self.skill_rank(c, SKILL_DEMOLITIONS).max(0));
        let mut t = TrapState {
            trap_type,
            detect_dc,
            disarm_dc,
            detectable: true,
            disarmable: true,
            one_shot: true,
            faction,
            creator,
            owner_demolitions,
            ..Default::default()
        };
        if let Some(c) = creator {
            t.detected_by.insert(c);
        }
        // Whoever stands on a fresh mine does not set it off by stepping in.
        let inside: Vec<ObjectId> = self
            .iter()
            .filter(|x| x.kind == ObjectKind::Creature && x.position.truncate().distance(at.truncate()) < h * 1.5)
            .map(|x| x.id)
            .collect();
        t.inside.extend(inside);
        o.trap = Some(Box::new(t));
        self.objects.insert(id, o);
        self.emit(WorldEvent::ObjectSpawned(id));
        id
    }

    /// Set Mine: `user` plants one of `item` (a trap kit) in front of
    /// itself. Near a closed blastable door it becomes a breaching charge.
    pub fn set_mine(&mut self, user: ObjectId, item: ObjectId) -> Result<ObjectId, String> {
        let trap_type = self.mine_trap_type(item).ok_or("not a mine")?;
        let (pos, bearing) = self.get(user).map(|o| (o.position, o.bearing)).ok_or("no user")?;
        let fwd = Vec3::new(-bearing.sin(), bearing.cos(), 0.0);
        let at = pos + fwd * 1.0;
        let demo = self.skill_rank(user, SKILL_DEMOLITIONS).max(0);
        let detect_dc = demo + self.trap_row_int(trap_type, "detectdcmod");
        let disarm_dc = demo + self.trap_row_int(trap_type, "disarmdcmod");
        let faction = self.faction_of(user);
        // Use up one from the stack.
        let left = {
            let it = self.get_mut(item).and_then(|o| o.item_mut()).ok_or("no item")?;
            it.stack_size -= 1;
            it.stack_size
        };
        if left <= 0 {
            self.detach_item(item);
            if let Some(o) = self.get_mut(item) {
                o.destroyed = true;
            }
        }
        let mine = self.spawn_mine(trap_type, at, detect_dc, disarm_dc, Some(user), faction);
        let doors = self.blastable_doors_near(at, BREACH_RANGE);
        if !doors.is_empty()
            && let Some(t) = self.trap_mut(mine)
        {
            t.fuse = Some(BREACH_FUSE);
        }
        self.feedback(self.talk_str(TLK_MINE_LAID), None);
        Ok(mine)
    }

    // ---------------------------------------------------------------
    // Disarm / recover
    // ---------------------------------------------------------------

    /// Disable Mine (or Recover Mine with `recover`): d20 + Demolitions
    /// against the disarm DC. Ok(true) on success; failing by 5+ sets it off.
    pub fn disarm_trap(&mut self, user: ObjectId, trap: ObjectId, recover: bool) -> Result<bool, String> {
        let t = self.trap(trap).cloned().ok_or("not a trap")?;
        if t.disabled {
            return Err("already disarmed".into());
        }
        // Inferred: only the creature that set the mine gets it back without a roll.
        let own = t.creator == Some(user);
        if !t.disarmable && !own {
            return Err("cannot be disarmed".into());
        }
        if recover && !self.is_trigger_trap(trap) {
            return Err("nothing to recover".into());
        }
        let (Some(up), Some(tp)) = (self.get(user).map(|o| o.position), self.trap_position(trap)) else {
            return Err("gone".into());
        };
        if up.truncate().distance(tp.truncate()) > USE_RANGE + MINE_HALF {
            return Err("too far".into());
        }
        let dc = t.disarm_dc + if recover { RECOVER_DC_BONUS } else { 0 };
        let rank = self.skill_rank(user, SKILL_DEMOLITIONS).max(0);
        // Own mines: take 20 (no roll).
        let roll = if own { 20 } else { self.combat.roll(20) };
        let total = roll + rank;
        let success = total >= dc;
        self.emit(WorldEvent::SkillCheck { user, target: trap, skill: SKILL_DEMOLITIONS as i32, dc, total, success });
        if success {
            if let Some(t) = self.trap_mut(trap) {
                t.disabled = true;
            }
            self.systems.last_disarmed = Some(trap);
            self.queue_event(trap, "OnDisarm", ScriptCtx { entering: user, clicking: user, ..Default::default() });
            if recover && let Some(item) = self.trap_row_str(t.trap_type, "resref") {
                self.give_new_item(&item, user, 1, false);
            }
            let text = self.talk_str(if recover { TLK_RECOVER_MINE } else { 37363 });
            // Guess: "<Recover Mine>: <success>" from TLK 1531 and 1392.
            self.feedback(if recover { format!("{text}: {}", self.talk_str(TLK_SUCCESS)) } else { text }, Some("Inventory_Add"));
            // Before the destroy: a destroyed trigger's OnDisarm would not run.
            self.pump();
            if self.is_trigger_trap(trap) {
                self.destroy(trap);
            }
            Ok(true)
        } else {
            // Guess: "<Disable/Recover Mine>: <failure>" from TLK 1529/1531 and 1393.
            let label = self.talk_str(if recover { TLK_RECOVER_MINE } else { TLK_DISABLE_MINE });
            self.feedback(format!("{label}: {}", self.talk_str(TLK_FAILURE)), Some("Error_Default"));
            if dc - total >= 5 {
                self.detonate_trap(trap, user);
            }
            Ok(false)
        }
    }

    /// Flag Trap: a trap the user knows becomes visible to every creature.
    pub fn flag_trap(&mut self, user: ObjectId, trap: ObjectId) -> Result<(), String> {
        if !self.is_armed_trap(trap) {
            return Err("not a trap".into());
        }
        if !self.trap_known_to(trap, user) {
            return Err("not detected".into());
        }
        if let Some(t) = self.trap_mut(trap) {
            t.flagged = true;
        }
        Ok(())
    }

    /// Examine Trap: the trap's type, for a trap the user knows.
    pub fn examine_trap(&mut self, user: ObjectId, trap: ObjectId) -> Result<String, String> {
        let ty = self.trap(trap).ok_or("not a trap")?.trap_type;
        if !self.trap_known_to(trap, user) {
            return Err("not detected".into());
        }
        // TLK 1517 is the mine line. A door or placeable has no original line
        // for its trap, so it gets the traps.2da name alone.
        let text = if self.is_trigger_trap(trap) {
            self.talk_str(TLK_EXAMINE_TRAP).replace("<CUSTOM0>", &self.trap_name(ty))
        } else {
            self.trap_name(ty)
        };
        self.feedback(text.clone(), None);
        Ok(text)
    }

    /// A trapped door or placeable the user is opening or using: an armed
    /// trap goes off first. Returns whether one did. Trigger traps (mines,
    /// GIT traps) are not used this way.
    pub(crate) fn trigger_object_trap(&mut self, target: ObjectId, user: ObjectId) -> bool {
        if !self.is_armed_trap(target) || self.is_trigger_trap(target) {
            return false;
        }
        self.detonate_trap(target, user);
        true
    }

    // ---------------------------------------------------------------
    // Detonation
    // ---------------------------------------------------------------

    /// Sets a trap off: its script, the blast on doors, the event.
    pub fn detonate_trap(&mut self, trap: ObjectId, by: ObjectId) {
        self.trap_goes_off(trap, by, true);
    }

    /// DetonateMine: the explosion, then a trigger mine is destroyed whatever
    /// its one_shot or creator flags (nwscript 857 says the mine is
    /// destroyed). A trapped door or placeable is only disabled. The damage
    /// script is not run (nwscript: the script ExecuteScripts k_trp_generic
    /// first).
    pub fn detonate_mine(&mut self, mine: ObjectId) {
        // A trigger with no trap state is not a mine: nothing to detonate.
        if self.trap(mine).is_none() {
            return;
        }
        self.trap_goes_off(mine, OBJECT_INVALID, false);
        if let Some(t) = self.trap_mut(mine) {
            t.disabled = true;
        }
        if self.is_trigger_trap(mine) {
            self.destroy(mine);
        }
    }

    fn trap_goes_off(&mut self, trap: ObjectId, by: ObjectId, run_script: bool) {
        let Some(t) = self.trap(trap).cloned() else { return };
        if t.disabled {
            return;
        }
        let at = self.trap_position(trap).unwrap_or_default();
        if self.trace {
            println!("  trap {} goes off (by {})", self.describe(trap), self.describe(by));
        }
        let model = self.trap_row_str(t.trap_type, "model");
        let sound = self.trap_row_str(t.trap_type, "explosionsound");
        self.emit(WorldEvent::TrapTriggered { trap, position: at, trap_type: t.trap_type, model, sound });
        if run_script {
            let ctx = ScriptCtx { entering: by, clicking: by, ..Default::default() };
            let script = self.get(trap).and_then(|o| o.script("OnTrapTriggered").map(str::to_owned)).or_else(|| self.trap_row_str(t.trap_type, "trapscript"));
            if let Some(s) = script {
                self.run_script(&s, trap, ctx);
            }
        }
        // The engine blasts doors that allow it (scripts only hurt creatures).
        // Only trigger traps blast: a trapped door does not break itself.
        let trigger = self.is_trigger_trap(trap);
        let radius = BLAST_RADIUS + 0.1 * t.owner_demolitions as f32;
        let creator = t.creator.unwrap_or(by);
        let blast = if trigger { self.blastable_doors_near(at, radius) } else { Vec::new() };
        for door in blast {
            let hp = self.object_hp(door).map_or(1, |h| h.current.max(1));
            if self.trace {
                println!("  blast breaks {}", self.describe(door));
            }
            self.damage_object(door, hp + 100, creator);
        }
        if t.one_shot || t.creator.is_some() {
            if let Some(t) = self.trap_mut(trap) {
                t.disabled = true;
            }
            if trigger {
                self.destroy(trap);
            }
        }
        self.pump();
    }

    // ---------------------------------------------------------------
    // Tick
    // ---------------------------------------------------------------

    /// Fuses, detection and stepping into traps. Called from tick().
    pub(crate) fn run_traps(&mut self, dt: f32) {
        let traps: Vec<ObjectId> = self.iter().filter(|o| o.trap.as_ref().is_some_and(|t| !t.disabled)).map(|o| o.id).collect();
        if traps.is_empty() {
            return;
        }
        // Breaching charges.
        let mut boom = Vec::new();
        for &id in &traps {
            if let Some(t) = self.trap_mut(id)
                && let Some(f) = t.fuse.as_mut()
            {
                *f -= dt;
                if *f <= 0.0 {
                    boom.push((id, t.creator.unwrap_or(OBJECT_INVALID)));
                }
            }
        }
        for (id, by) in boom {
            self.detonate_trap(id, by);
        }
        self.detect_traps(dt);
        self.step_into_traps();
    }

    fn detect_traps(&mut self, dt: f32) {
        self.systems.detect_in -= dt;
        if self.systems.detect_in > 0.0 {
            return;
        }
        self.systems.detect_in = DETECT_INTERVAL;
        let party: Vec<(ObjectId, Vec3)> = self
            .party
            .iter()
            .filter_map(|&p| self.get(p).filter(|o| o.creature().is_some_and(|c| !c.dead)).map(|o| (p, o.position)))
            .collect();
        let traps: Vec<ObjectId> = self.iter().filter(|o| o.trap.as_ref().is_some_and(|t| !t.disabled && t.detectable)).map(|o| o.id).collect();
        for trap in traps {
            if self.party_detected(trap) {
                continue;
            }
            let Some(tp) = self.trap_position(trap) else { continue };
            let dc = self.trap(trap).map_or(0, |t| t.detect_dc);
            for &(p, pos) in &party {
                if pos.distance(tp) > DETECT_RANGE {
                    continue;
                }
                let total = self.combat.roll(20) + self.skill_rank(p, SKILL_AWARENESS).max(0);
                if total >= dc {
                    if let Some(t) = self.trap_mut(trap) {
                        t.detected_by.insert(p);
                    }
                    self.systems.last_detected.insert(p, trap);
                    // An unnamed trap (a door or placeable) falls back to its traps.2da name.
                    let name = self
                        .get(trap)
                        .map(|o| o.name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| self.trap_name(self.trap(trap).map_or(0, |t| t.trap_type)));
                    let text = self
                        .talk_str(TLK_DETECTS_DC)
                        .replace("<CUSTOM0>", &self.describe_short(p))
                        .replace("<CUSTOM1>", &name)
                        .replace("<CUSTOM2>", &total.to_string())
                        .replace("<CUSTOM3>", &dc.to_string());
                    self.feedback(text, Some("Prompt"));
                    break;
                }
            }
        }
    }

    fn step_into_traps(&mut self) {
        let creatures: Vec<(ObjectId, Vec3)> = self
            .iter()
            .filter(|o| o.kind == ObjectKind::Creature && o.creature().is_some_and(|c| !c.dead))
            .map(|o| (o.id, o.position))
            .collect();
        let traps: Vec<ObjectId> = self.iter().filter(|o| o.trap.as_ref().is_some_and(|t| !t.disabled)).map(|o| o.id).collect();
        let mut fire = Vec::new();
        for trap in traps {
            let Some(o) = self.get(trap) else { continue };
            let Detail::Trigger(tr) = &o.detail else { continue };
            let poly = tr.polygon.clone();
            let t = o.trap.as_deref().cloned().unwrap_or_default();
            let mut inside_now = BTreeSet::new();
            for &(c, p) in &creatures {
                if k2_data::module::point_in_polygon(&poly, p.x, p.y) && (p.z - poly.first().map_or(p.z, |q| q[2])).abs() < 2.5 {
                    inside_now.insert(c);
                }
            }
            let mut by = None;
            for &c in inside_now.difference(&t.inside) {
                // repute.2da row = the creature, column = the trap: the
                // party rows hate the Trap faction (13), enemies don't;
                // enemies hate a party-set (Friendly_1) mine.
                let hostile = self.faction_hostile(self.faction_of(c), t.faction);
                if hostile {
                    by = Some(c);
                    break;
                }
            }
            if let Some(tm) = self.trap_mut(trap) {
                tm.inside = inside_now;
            }
            if let Some(c) = by {
                fire.push((trap, c));
            }
        }
        for (trap, c) in fire {
            // "You triggered a Mine!" only for mines (trapscript k_trp_generic),
            // not the rockfalls, and only when a party member stepped in.
            let mine = self.trap(trap).is_some_and(|t| self.trap_row_str(t.trap_type, "trapscript").as_deref() == Some("k_trp_generic"));
            if mine && self.party.contains(&c) {
                self.feedback(self.talk_str(TLK_TRIGGERED_MINE), None);
            }
            self.detonate_trap(trap, c);
        }
    }

    fn faction_hostile(&mut self, a: i32, b: i32) -> bool {
        self.ensure_factions();
        self.combat.factions.reputation(a, b) <= crate::factions::HOSTILE_MAX
    }

    // ---------------------------------------------------------------
    // Feedback helpers
    // ---------------------------------------------------------------

    /// A dialog.tlk string by strref (empty when missing).
    pub fn talk_str(&self, strref: i32) -> String {
        let Ok(strref) = u32::try_from(strref) else { return String::new() };
        self.talk.as_ref().and_then(|t| t.get(strref)).map(str::to_owned).unwrap_or_default()
    }

    /// A feedback line for the message log, with an optional guisounds cue.
    pub(crate) fn feedback(&mut self, text: String, cue: Option<&str>) {
        self.emit(WorldEvent::Feedback { text, cue: cue.map(str::to_owned) });
    }

    fn describe_short(&self, id: ObjectId) -> String {
        let n = self.get(id).map(|o| self.substitute(&o.name)).unwrap_or_default();
        if n.is_empty() { self.get(id).map(|o| o.tag.clone()).unwrap_or_default() } else { n }
    }

    // ---------------------------------------------------------------
    // Engine routines
    // ---------------------------------------------------------------

    /// Trap routines. None = not handled here.
    pub(crate) fn traps_call(&mut self, name: &str, me: ObjectId, a: &[Value]) -> Option<Value> {
        let obj = |n: usize| match a.get(n) {
            Some(Value::Object(o)) => *o,
            _ => OBJECT_INVALID,
        };
        let int = |n: usize| match a.get(n) {
            Some(Value::Int(i)) => *i,
            _ => 0,
        };
        let b = |x: bool| Value::Int(i32::from(x));
        let t = |w: &World, n: usize| w.trap(obj(n)).cloned();
        let v = match name {
            "GetIsTrapped" => b(self.is_armed_trap(obj(0))),
            "GetTrapBaseType" => Value::Int(t(self, 0).map_or(-1, |t| t.trap_type)),
            "GetTrapDetectable" => b(t(self, 0).is_some_and(|t| t.detectable)),
            "GetTrapDisarmable" => b(t(self, 0).is_some_and(|t| t.disarmable)),
            "GetTrapFlagged" => b(t(self, 0).is_some_and(|t| t.flagged)),
            "GetTrapOneShot" => b(t(self, 0).is_some_and(|t| t.one_shot)),
            "GetTrapDisarmDC" => Value::Int(t(self, 0).map_or(0, |t| t.disarm_dc)),
            "GetTrapDetectDC" => Value::Int(t(self, 0).map_or(0, |t| t.detect_dc)),
            "GetTrapKeyTag" => Value::String("".into()),
            "GetTrapCreator" => Value::Object(t(self, 0).and_then(|t| t.creator).unwrap_or(OBJECT_INVALID)),
            "GetTrapDetectedBy" => b(t(self, 0).is_some_and(|t| t.flagged || t.detected_by.contains(&obj(1)))),
            "GetNearestTrapToObject" => {
                // GetNearestTrapToObject(oTarget, nTrapDetected): armed traps
                // on triggers, placeables and doors; detected-only by default.
                let who = if a.is_empty() { me } else { obj(0) };
                let detected_only = !matches!(a.get(1), Some(Value::Int(0)));
                let from = self.get(who).map(|o| o.position);
                let near = self
                    .iter()
                    .filter(|o| o.trap.as_ref().is_some_and(|t| !t.disabled))
                    .map(|o| o.id)
                    .filter(|&id| !detected_only || self.trap_known_to(id, who))
                    .filter_map(|id| Some((self.trap_position(id)?, id)))
                    .filter_map(|(p, id)| from.map(|f| (p.distance(f), id)))
                    .min_by(|x, y| x.0.total_cmp(&y.0));
                Value::Object(near.map_or(OBJECT_INVALID, |(_, id)| id))
            }
            "SetTrapDetectedBy" => {
                let who = obj(1);
                let ok = match self.trap_mut(obj(0)) {
                    Some(t) => {
                        t.detected_by.insert(who);
                        true
                    }
                    None => false,
                };
                b(ok)
            }
            "SetTrapDisabled" => {
                if let Some(t) = self.trap_mut(obj(0)) {
                    t.disabled = true;
                }
                Value::Void
            }
            "GetLastDisarmed" => Value::Object(self.systems.last_disarmed.unwrap_or(OBJECT_INVALID)),
            "GetLastTrapDetected" => {
                let who = if a.is_empty() { me } else { obj(0) };
                Value::Object(self.systems.last_detected.get(&who).copied().unwrap_or(OBJECT_INVALID))
            }
            "GetOwnerDemolitionsSkill" => {
                let who = if a.is_empty() { me } else { obj(0) };
                Value::Int(self.trap(who).map_or(-1, |t| if t.creator.is_some() { t.owner_demolitions } else { -1 }))
            }
            "SpawnMine" => {
                // SpawnMine(nMineType, lPoint, nDetectDCBase, nDisarmDCBase, oCreator)
                let Some(at) = a.get(1).and_then(|v| self.engine.location(v)).map(|l| l.position) else {
                    return Some(Value::Void);
                };
                let ty = int(0);
                let creator = obj(4);
                let creator = self.is_valid(creator).then_some(creator);
                let detect = int(2) + self.trap_row_int(ty, "detectdcmod");
                let disarm = int(3) + self.trap_row_int(ty, "disarmdcmod");
                let faction = creator.map_or(FACTION_HOSTILE, |c| self.faction_of(c));
                self.spawn_mine(ty, at, detect, disarm, creator, faction);
                Value::Void
            }
            "DetonateMine" => {
                self.detonate_mine(obj(0));
                Value::Void
            }
            "ActionUseSkill" if usize::try_from(int(0)).ok() == Some(SKILL_DEMOLITIONS) && self.trap(obj(1)).is_some() => {
                // nSubSkill: SUBSKILL_* (0 = disable, 100 flag, 101 recover,
                // 102 examine).
                match int(2) {
                    SUBSKILL_FLAG_TRAP => {
                        let _ = self.flag_trap(me, obj(1));
                    }
                    SUBSKILL_RECOVER_TRAP => {
                        let _ = self.disarm_trap(me, obj(1), true);
                    }
                    SUBSKILL_EXAMINE_TRAP => {
                        let _ = self.examine_trap(me, obj(1));
                    }
                    _ => {
                        let _ = self.disarm_trap(me, obj(1), false);
                    }
                }
                Value::Void
            }
            _ => return None,
        };
        Some(v)
    }
}
