//! Influence (K2) and alignment feedback.
//!
//! Influence is per NPC index (NPC_* constants, the party table's
//! numbering), 0..100, default 50. GetInfluence / SetInfluence /
//! ModifyInfluence are the engine routines; dialog scripts (a_influence_inc
//! and friends) call them. A change shows the engine's feedback line
//! "Influence Gained: <CUSTOM0>" / "Influence Lost: <CUSTOM0>" (TLK 48607 /
//! 48608) with the companion's name.
//!
//! Alignment: AdjustAlignment and SetGoodEvilValue on party members show
//! "Light Side Points Gained" / "Dark Side Points Gained" (TLK 42441 /
//! 42440) with the LightSide / DarkSide interface cue (guisounds.2da).
//! Values live on the creature (good_evil, 0..100, 50 neutral).

use k2_script::{ObjectId, OBJECT_INVALID, Value};

use crate::World;

const ALIGNMENT_LIGHT_SIDE: i32 = 2;
const ALIGNMENT_DARK_SIDE: i32 = 3;

impl World {
    pub fn influence_of(&self, npc: i32) -> i32 {
        self.influence.get(&npc).copied().unwrap_or(50)
    }

    /// Display name of a party NPC by index: its creature, else its party
    /// template, else the stock p_<tag> template from npc.2da, else the tag.
    pub fn npc_name(&self, npc: i32) -> String {
        if let Some(n) = self.available_npcs.get(&npc) {
            if let Some(o) = n.object.and_then(|id| self.get(id)) {
                return self.substitute(&o.name);
            }
            if let Some(o) = n.stored.as_deref() {
                return self.substitute(&o.name);
            }
            if let Ok(bp) = k2_data::Creature::load(&self.res, &n.template) {
                return self.resolve(&bp.first_name);
            }
        }
        let tag = self
            .tables
            .get(&self.res, "npc")
            .ok()
            .and_then(|t| t.str(usize::try_from(npc).ok()?, "tag").map(str::to_owned))
            .unwrap_or_else(|| format!("NPC {npc}"));
        k2_data::Creature::load(&self.res, &format!("p_{}", tag.to_ascii_lowercase()))
            .map(|bp| self.resolve(&bp.first_name))
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or(tag)
    }

    fn change_influence(&mut self, npc: i32, value: i32) {
        let before = self.influence_of(npc);
        let after = value.clamp(0, 100);
        self.influence.insert(npc, after);
        if after == before {
            return;
        }
        let strref = if after > before { 48607 } else { 48608 };
        let text = self.talk_str(strref).replace("<CUSTOM0>", &self.npc_name(npc));
        self.feedback(text, Some("Prompt"));
    }

    fn change_alignment(&mut self, target: ObjectId, value: i32) {
        let Some(c) = self.get_mut(target).and_then(|o| o.creature_mut()) else { return };
        let before = c.good_evil;
        c.good_evil = value.clamp(0, 100);
        let after = c.good_evil;
        if after == before || !self.party.contains(&target) {
            return;
        }
        let (strref, cue) = if after > before { (42441, "LightSide") } else { (42440, "DarkSide") };
        let text = self.talk_str(strref);
        self.feedback(text, Some(cue));
    }

    /// Influence and alignment routines. None = not handled here.
    pub(crate) fn influence_call(&mut self, name: &str, _me: ObjectId, a: &[Value]) -> Option<Value> {
        let obj = |n: usize| match a.get(n) {
            Some(Value::Object(o)) => *o,
            _ => OBJECT_INVALID,
        };
        let int = |n: usize| match a.get(n) {
            Some(Value::Int(i)) => *i,
            _ => 0,
        };
        let v = match name {
            "GetInfluence" => Value::Int(self.influence_of(int(0))),
            "SetInfluence" => {
                self.change_influence(int(0), int(1));
                Value::Void
            }
            "ModifyInfluence" => {
                let n = int(0);
                let v = self.influence_of(n) + int(1);
                self.change_influence(n, v);
                Value::Void
            }
            "AdjustAlignment" => {
                // AdjustAlignment(oSubject, nAlignment, nShift, bDontModifyNPCs)
                let (t, al, shift) = (obj(0), int(1), int(2));
                let delta = match al {
                    ALIGNMENT_LIGHT_SIDE => shift,
                    ALIGNMENT_DARK_SIDE => -shift,
                    _ => 0,
                };
                let cur = self.get(t).and_then(|o| o.creature()).map(|c| c.good_evil);
                if let Some(cur) = cur {
                    self.change_alignment(t, cur + delta);
                }
                Value::Void
            }
            "SetGoodEvilValue" => {
                self.change_alignment(obj(0), int(1));
                Value::Void
            }
            _ => return None,
        };
        Some(v)
    }
}
