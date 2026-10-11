//! Vitality (HP) and Force point regeneration, once a second (run_powers).
//!
//! The base rates are regeneration.2da: timeperhp, the out-of-combat and
//! in-combat HP bases with the Constitution term, fpregentime with the
//! out-of-combat and in-combat Force bases. Item properties Regeneration (35)
//! and Regeneration_Force_Points (54) add points every timeperhp seconds,
//! EffectRegenerate adds amount per interval, EffectVPRegenModifier and
//! EffectFPRegenModifier scale the rates by a percentage, and feat 143
//! (Regenerate Force Points) scales the base Force rate. Knocked-out and dead
//! creatures do not regenerate. See docs/notes/combat.md, "Regeneration".

use k2_script::{ObjectId, Value};

use crate::engine::EngineData;
use crate::items::ItemProperty;
use crate::object::ObjectKind;
use crate::stats::SLOT_RIGHTWEAPON2;
use crate::World;

/// itempropdef.2da rows.
const PROP_REGENERATION: u16 = 35;
const PROP_REGENERATION_FORCE_POINTS: u16 = 54;

/// feat.2da row of Regenerate Force Points (TLK 48099, Level 4 Jedi).
const FEAT_REGENERATE_FORCE_POINTS: i32 = 143;
/// Base Force rate multipliers of that feat, out of combat and in combat
/// (StrategyWiki K2 Force powers, as summarised by a search result; not
/// checked in game).
const FEAT_FP_OUT_OF_COMBAT: f32 = 1.25;
const FEAT_FP_IN_COMBAT: f32 = 3.5;

/// regeneration.2da, read by row label (the value column).
#[derive(Debug, Clone, Copy)]
struct RegenTable {
    /// Seconds per point: the interval of item and effect regeneration.
    time_per_hp: f32,
    out_of_combat_hp: f32,
    in_combat_hp: f32,
    /// Points per Constitution modifier point, added to the HP base.
    con_bonus: f32,
    /// Seconds to refill Force points from 0 to the maximum out of combat.
    fp_time: f32,
    out_of_combat_fp: f32,
    in_combat_fp: f32,
    /// Added to the Force base per Wisdom modifier point (0 in K2).
    wis_bonus: f32,
}

impl World {
    fn regen_table(&self) -> Option<RegenTable> {
        let t = self.tables.get(&self.res, "regeneration").ok()?;
        let value = |label: &str| (0..t.rows()).find(|&r| t.row_label(r) == Some(label)).and_then(|r| t.float(r, "value"));
        let table = RegenTable {
            time_per_hp: value("timeperhp")?,
            out_of_combat_hp: value("outofcombathpbase")?,
            in_combat_hp: value("incombathpbase")?,
            con_bonus: value("conmodbonus")?,
            fp_time: value("fpregentime")?,
            out_of_combat_fp: value("outofcombatfpbase")?,
            in_combat_fp: value("incombatfpbase")?,
            wis_bonus: value("wismodbonus")?,
        };
        (table.time_per_hp > 0.0 && table.fp_time > 0.0).then_some(table)
    }

    /// Points an item property gives per regeneration interval: CostValue
    /// through its cost table (iprp_costtable names it, its value column is
    /// the points: iprp_meleecost 4 is 4).
    fn regen_points(&self, p: &ItemProperty) -> f32 {
        let name = self.tables.get(&self.res, "iprp_costtable").ok().and_then(|t| t.str(usize::from(p.cost_table), "name").map(str::to_ascii_lowercase));
        let Some(name) = name else { return 0.0 };
        self.tables.get(&self.res, &name).ok().and_then(|t| t.int(usize::from(p.cost_value), "value")).unwrap_or(0) as f32
    }

    /// Vitality points and Force points per second for a creature, before
    /// fractions are carried. None when the creature is gone.
    pub fn regeneration_rates(&mut self, id: ObjectId) -> Option<(f32, f32)> {
        let t = self.regen_table()?;
        self.rates_with(id, &t)
    }

    fn rates_with(&mut self, id: ObjectId, t: &RegenTable) -> Option<(f32, f32)> {
        let party = self.party.contains(&id);
        let in_combat = self.in_combat(id);
        let o = self.get(id)?;
        let c = o.creature()?;
        if c.dead || c.down {
            return Some((0.0, 0.0));
        }
        let max_force = c.max_force;
        let equipped: Vec<ObjectId> = c.equipped.iter().filter(|(slot, _)| **slot < SLOT_RIGHTWEAPON2).map(|(_, &item)| item).collect();
        let (mut hp_effects, mut vp_pct, mut fp_pct) = (0.0_f32, 0, 0);
        for ae in &o.effects {
            let Some(EngineData::Effect(e)) = self.engine.by_handle(ae.handle) else { continue };
            let arg = |n: usize| e.args.get(n).and_then(Value::as_int).unwrap_or(0);
            match e.name.as_str() {
                "EffectRegenerate" => {
                    let interval = e.args.get(1).and_then(Value::as_float).unwrap_or(0.0);
                    if interval > 0.0 {
                        hp_effects += arg(0) as f32 / interval;
                    }
                }
                "EffectVPRegenModifier" => vp_pct += arg(0),
                "EffectFPRegenModifier" => fp_pct += arg(0),
                _ => {}
            }
        }

        let (mut hp_items, mut fp_items) = (0.0_f32, 0.0_f32);
        for item in equipped {
            let Some(info) = self.item_info(item) else { continue };
            for p in &info.properties {
                match p.property {
                    PROP_REGENERATION => hp_items += self.regen_points(p),
                    PROP_REGENERATION_FORCE_POINTS => fp_items += self.regen_points(p),
                    _ => {}
                }
            }
        }

        // Constitution (party HP) and Wisdom (Force, only if the table gives
        // it weight) come from the stats with gear and effects applied.
        let stats = (party || t.wis_bonus != 0.0).then(|| self.combat_stats(id)).flatten();
        let con = stats.as_ref().map_or(0, |s| s.modifier(2));
        let wis = stats.as_ref().map_or(0, |s| s.modifier(4));

        // Innate HP regeneration is for party members only (nwscript:
        // DisableHealthRegen, "all party members have").
        let innate_hp = if party && !self.powers.innate_hp_regen_off {
            let base = if in_combat { t.in_combat_hp } else { t.out_of_combat_hp };
            (base + t.con_bonus * con as f32).max(0.0) / t.time_per_hp
        } else {
            0.0
        };
        let vp = (1.0 + vp_pct as f32 / 100.0).max(0.0);
        let hp_rate = (innate_hp + hp_items / t.time_per_hp + hp_effects) * vp;

        let feat = if self.has_feat(id, FEAT_REGENERATE_FORCE_POINTS) {
            if in_combat { FEAT_FP_IN_COMBAT } else { FEAT_FP_OUT_OF_COMBAT }
        } else {
            1.0
        };
        let base_fp = if in_combat { t.in_combat_fp } else { t.out_of_combat_fp } + t.wis_bonus * wis as f32;
        let fp_base_rate = max_force as f32 * base_fp.max(0.0) * feat / t.fp_time;
        let fp = (fp_base_rate + fp_items / t.time_per_hp) * (1.0 + fp_pct as f32 / 100.0).max(0.0);
        Some((hp_rate.max(0.0), fp.max(0.0)))
    }

    /// One regeneration step of `dt` seconds for every living creature. Paused
    /// while a conversation runs (guess: the original's dialogue behaviour is
    /// not verified).
    pub(crate) fn run_regen(&mut self, dt: f32) {
        if self.conversation.is_some() {
            return;
        }
        let Some(t) = self.regen_table() else { return };
        let ids: Vec<ObjectId> = self.iter().filter(|o| o.kind == ObjectKind::Creature).map(|o| o.id).collect();
        for id in ids {
            let Some(c) = self.get(id).and_then(|o| o.creature()) else { continue };
            if c.dead || c.down {
                continue;
            }
            let Some((hp_rate, fp_rate)) = self.rates_with(id, &t) else { continue };
            let Some(c) = self.get_mut(id).and_then(|o| o.creature_mut()) else { continue };
            c.hp += bank(&mut c.regen_carry[0], hp_rate, dt, c.max_hp - c.hp);
            c.force += bank(&mut c.regen_carry[1], fp_rate, dt, c.max_force - c.force);
        }
    }
}

/// Adds `per_second * dt` to the carried fraction and pays out the whole
/// points, at most `room`. A full creature carries nothing.
fn bank(carry: &mut f32, per_second: f32, dt: f32, room: i32) -> i32 {
    if room <= 0 {
        *carry = 0.0;
        return 0;
    }
    *carry += per_second * dt;
    let whole = carry.floor();
    *carry -= whole;
    (whole as i32).min(room)
}
