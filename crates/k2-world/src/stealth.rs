//! Stealth.
//!
//! A creature with a Stealth Field Generator (baseitems Stealth_Unit, row
//! 88, belt slot) equipped can enter stealth mode, outside combat only
//! ("You cannot enter stealth mode while in combat", TLK 1452). While
//! stealthed it cannot run (TLK 1464 "Stealth Mode Activated (Run
//! Disabled)").
//!
//! Hiding: every [`HIDE_INTERVAL`] s each stealthed creature is checked
//! against every other creature within its perception range: it stays
//! hidden from an observer unless the observer is within [`EXPOSE_RANGE`],
//! or the observer's Awareness + 10 reaches the stealther's Stealth + 10 +
//! [`FIELD_BONUS`]. KotOR rolls these opposed checks; here they take 10 on
//! both sides so the result does not flicker. Hidden pairs feed perception
//! (OnNotice), GetObjectSeen and the combat target search.
//!
//! Attacking, casting at someone or being in combat ends stealth.
//!
//! Stealth XP (K2): the GetMaxStealthXP / SetCurrentStealthXP /
//! AwardStealthXP family stores numbers that area scripts use to reward
//! sneaking past guards; AwardStealthXP gives the current pool as XP.

use std::collections::HashSet;

use k2_script::{ObjectId, OBJECT_INVALID, Value};
use serde::{Deserialize, Serialize};

use crate::object::ObjectKind;
use crate::{World, WorldEvent};

pub const SKILL_STEALTH: usize = 2;
const SKILL_AWARENESS: usize = 3;
/// baseitems.2da Stealth_Unit.
pub const BASE_ITEM_STEALTH_UNIT: u32 = 88;
pub const HIDE_INTERVAL: f32 = 0.5;
/// Closer than this, a stealthed creature is always seen.
pub const EXPOSE_RANGE: f32 = 1.5;
/// The field generator's edge over a plain Awareness check.
pub const FIELD_BONUS: i32 = 5;

/// K2 stealth XP pool (saved).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StealthXp {
    pub enabled: bool,
    pub max: i32,
    pub current: i32,
    pub decrement: i32,
    /// SetKeepStealthInDialog. Not saved (nwscript 819: "it isn't saved").
    #[serde(skip)]
    pub keep_in_dialog: bool,
    /// Seconds to the next hidden-pairs refresh (not saved meaningfully).
    pub refresh_in: f32,
    /// (observer, stealther) pairs where the stealther is hidden.
    #[serde(skip)]
    pub hidden: HashSet<(ObjectId, ObjectId)>,
}

impl World {
    pub fn is_stealthed(&self, id: ObjectId) -> bool {
        self.get(id).and_then(|o| o.creature()).is_some_and(|c| c.stealth && !c.dead)
    }

    /// Whether `target` is hidden from `observer` by stealth.
    pub fn hidden_from(&self, observer: ObjectId, target: ObjectId) -> bool {
        self.systems.stealth.hidden.contains(&(observer, target))
    }

    /// Whether `id` has a Stealth Field Generator equipped.
    pub fn has_stealth_unit(&mut self, id: ObjectId) -> bool {
        let items: Vec<ObjectId> = self.equipment(id).into_iter().map(|(_, i)| i).collect();
        items.into_iter().any(|i| self.item_def_of(i).is_some_and(|d| d.base_item == BASE_ITEM_STEALTH_UNIT))
    }

    /// Enters or leaves stealth mode. Err = refused (with the reason).
    pub fn set_stealth(&mut self, id: ObjectId, on: bool) -> Result<(), String> {
        if self.is_stealthed(id) == on {
            return Ok(());
        }
        if on {
            if !self.has_stealth_unit(id) {
                return Err("no stealth field generator equipped".into());
            }
            if self.in_combat(id) {
                let text = self.talk_str(1452);
                self.feedback(text.clone(), Some("Error_Default"));
                return Err(text);
            }
        }
        let Some(c) = self.get_mut(id).and_then(|o| o.creature_mut()) else { return Err("not a creature".into()) };
        c.stealth = on;
        self.emit(WorldEvent::StealthChanged { creature: id, stealthed: on });
        let text = self.talk_str(if on { 1464 } else { 1465 });
        if self.party.contains(&id) {
            self.feedback(text, None);
        }
        self.systems.stealth.refresh_in = 0.0;
        Ok(())
    }

    /// A cutscene line ends stealth for its two participants unless
    /// SetKeepStealthInDialog is on (nwscript: by default stealth is removed
    /// from anyone taking part in a cutscene). dialog.rs calls this for every
    /// staged node reached (AnimatedCut dialog or CameraAngle 4 line), so a
    /// plain conversation that reaches one ends stealth there. Plain lines
    /// keep stealth (guess: the nwscript text names cutscenes only).
    pub(crate) fn end_stealth_for_dialog(&mut self, owner: ObjectId, pc: ObjectId) {
        if self.systems.stealth.keep_in_dialog {
            return;
        }
        for id in [owner, pc] {
            if self.is_stealthed(id) {
                let _ = self.set_stealth(id, false);
            }
        }
    }

    /// Per tick: drop stealth in combat, refresh who is hidden from whom.
    pub(crate) fn run_stealth(&mut self, dt: f32) {
        let stealthers: Vec<ObjectId> = self.iter().filter(|o| o.creature().is_some_and(|c| c.stealth && !c.dead)).map(|o| o.id).collect();
        if stealthers.is_empty() {
            self.systems.stealth.hidden.clear();
            return;
        }
        for &s in &stealthers {
            if self.in_combat(s) {
                let _ = self.set_stealth(s, false);
            }
        }
        self.systems.stealth.refresh_in -= dt;
        if self.systems.stealth.refresh_in > 0.0 {
            return;
        }
        self.systems.stealth.refresh_in = HIDE_INTERVAL;
        let observers: Vec<(ObjectId, glam::Vec3, f32)> = self
            .iter()
            .filter(|o| o.kind == ObjectKind::Creature)
            .filter_map(|o| o.creature().filter(|c| !c.dead).map(|c| (o.id, o.position, c.perception_range)))
            .collect();
        let mut hidden = HashSet::new();
        for s in stealthers {
            if !self.is_stealthed(s) {
                continue;
            }
            let Some(sp) = self.get(s).map(|o| o.position) else { continue };
            let stealth = self.skill_rank(s, SKILL_STEALTH).max(0) + 10 + FIELD_BONUS;
            let side = self.faction_of(s);
            for &(ob, op, range) in &observers {
                if ob == s || self.faction_of(ob) == side {
                    continue;
                }
                let d = op.distance(sp);
                if d > range.max(20.0) {
                    continue;
                }
                let awareness = self.skill_rank(ob, SKILL_AWARENESS).max(0) + 10;
                if d > EXPOSE_RANGE && awareness < stealth {
                    hidden.insert((ob, s));
                }
            }
        }
        self.systems.stealth.hidden = hidden;
    }

    /// Stealth routines. None = not handled here.
    pub(crate) fn stealth_call(&mut self, name: &str, me: ObjectId, a: &[Value]) -> Option<Value> {
        let obj = |n: usize| match a.get(n) {
            Some(Value::Object(o)) => *o,
            _ => OBJECT_INVALID,
        };
        let int = |n: usize| match a.get(n) {
            Some(Value::Int(i)) => *i,
            _ => 0,
        };
        let st = &mut self.systems.stealth;
        let v = match name {
            "IsStealthed" => Value::Int(i32::from(self.is_stealthed(obj(0)))),
            "GetMaxStealthXP" => Value::Int(st.max),
            "SetMaxStealthXP" => {
                st.max = int(0);
                Value::Void
            }
            "GetCurrentStealthXP" => Value::Int(st.current),
            "SetCurrentStealthXP" => {
                st.current = int(0).clamp(0, st.max.max(int(0)));
                Value::Void
            }
            "GetStealthXPEnabled" => Value::Int(i32::from(st.enabled)),
            "SetStealthXPEnabled" => {
                st.enabled = int(0) != 0;
                Value::Void
            }
            "GetStealthXPDecrement" => Value::Int(st.decrement),
            "SetStealthXPDecrement" => {
                st.decrement = int(0);
                Value::Void
            }
            "SetKeepStealthInDialog" => {
                st.keep_in_dialog = int(0) != 0;
                Value::Void
            }
            "AwardStealthXP" => {
                let (enabled, xp) = (st.enabled, st.current);
                let target = obj(0);
                if enabled && xp > 0 && self.is_valid(target) {
                    self.give_xp(target, xp);
                }
                Value::Void
            }
            "ActionUseSkill" if usize::try_from(int(0)).ok() == Some(SKILL_STEALTH) => {
                let on = !self.is_stealthed(me);
                let _ = self.set_stealth(me, on);
                Value::Void
            }
            _ => return None,
        };
        Some(v)
    }
}
