//! Force powers, talents and the gameplay side of effects.
//!
//! - Talents: TalentSpell/TalentFeat/TalentSkill, GetCreatureTalentBest /
//!   Random over a creature's known powers (with enough Force points),
//!   item abilities (CastSpell items it can reach) and combat feats.
//!   Categories (spells.2da / feat.2da `category`) match a mask nibble by
//!   nibble, 0xF being a wildcard: 0xf8ff = any buff (0x1808), 0xf1ff = any
//!   damage power, 0x1104 = melee feats exactly.
//! - Casting: ActionUseTalentOnObject / ActionCastSpellAtObject /
//!   ActionCastFakeSpellAtObject queue an [`Action::Cast`]. The caster
//!   closes to range, pays the spells.2da `forcepoints`, plays
//!   castanim/conjanim, runs `impactscript` (k_sp1_generic, k_sup_*) with
//!   the spell routines answering for it at conjtime + casttime/2, and in
//!   combat the cast takes a full round and ends with OnEndCombatRound so
//!   k_ai_master picks the next move.
//! - Combat feats (Power Attack, Flurry, Critical Strike, Rapid Shot,
//!   Sniper Shot and their improved/master forms) modify the next round.
//! - Effects: attack/AC/damage/ability/save modifiers, extra attacks, speed,
//!   temporary hit points, damage resistance, Force point damage and drain,
//!   and debilitation (stun, paralyze, droid stun, horror, choke, whirlwind,
//!   push, sleep, entangle, confusion, fear, mind trick) which blocks
//!   actions. Timed effects expire. Everything else is recorded only.
//! - Saves: d20 + class save table + ability + effects vs DC.

use std::collections::{HashMap, HashSet};

use glam::Vec3;
use k2_script::{EngineValue, ObjectId, Value, OBJECT_INVALID};
use serde::{Deserialize, Serialize};

use crate::combat::ROUND;
use crate::engine::{EngineData, Talent, ENGINE_TALENT};
use crate::object::{bearing_towards, Action, ObjectKind, Target};
use crate::stats::CombatStats;
use crate::projectiles::saber_catch;
use crate::{ScriptCtx, World, WorldEvent};

pub const TALENT_SPELL: i32 = 0;
pub const TALENT_FEAT: i32 = 1;
pub const TALENT_SKILL: i32 = 2;

/// itempropdef.2da row of CastSpell.
const PROPERTY_CAST_SPELL: u32 = 10;

/// One queued cast (Force power, item ability, combat feat).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CastAction {
    pub talent_type: i32,
    pub id: i32,
    pub target: Target,
    /// ActionCastFakeSpell: animation only, no impact, no cost.
    pub fake: bool,
    /// bCheat: no knowledge or Force point check.
    pub cheat: bool,
    pub elapsed: f32,
    pub started: bool,
    pub impacted: bool,
    pub duration: f32,
    pub impact_at: f32,
    /// Item used for an item ability.
    pub item: Option<ObjectId>,
    /// Lightsaber throw: (seconds after the cast starts, target) of each stop
    /// still to reach. Each stop's hit lands when the saber gets there.
    #[serde(default)]
    pub saber_hits: Vec<(f32, ObjectId)>,
}

#[derive(Debug, Clone, Copy)]
struct ActiveCast {
    spell: i32,
    caster: ObjectId,
    target: ObjectId,
    location: Vec3,
    item: ObjectId,
    cost: i32,
    harmful: bool,
}

/// Per-world power state (saved, except the live cast stack).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PowersState {
    #[serde(skip)]
    casts: Vec<ActiveCast>,
    pub last_power: HashMap<ObjectId, i32>,
    pub last_feat: HashMap<ObjectId, i32>,
    pub power_failed: HashSet<ObjectId>,
    /// Effect handle -> spell id (GetEffectSpellId, GetHasSpellEffect).
    pub effect_spell: HashMap<u32, i32>,
    /// Combat feat for the current round, with the world time it lapses.
    pub pending_feat: HashMap<ObjectId, (i32, f64)>,
    /// Last spell cast at a creature: (caster, spell, harmful).
    pub spell_at: HashMap<ObjectId, (ObjectId, i32, bool)>,
    /// Remaining absorption of TemporaryHitpoints / DamageResistance limits.
    pub absorb_left: HashMap<u32, i32>,
    pub bonus_force: HashMap<ObjectId, i32>,
    /// DisableHealthRegen(TRUE): innate vitality regeneration of the party is off.
    #[serde(default)]
    pub innate_hp_regen_off: bool,
    regen_in: f32,
    rng: u64,
}

impl PowersState {
    fn roll(&mut self, die: i32) -> i32 {
        if self.rng == 0 {
            self.rng = 0x51_7cc1_b727_220a;
        }
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x % die.max(1) as u64) as i32 + 1
    }
}

/// Nibble-wise category match; 0xF in the mask matches anything.
pub fn category_matches(mask: i32, category: i32) -> bool {
    if category <= 0 {
        return false;
    }
    (0..4).all(|i| {
        let m = (mask >> (i * 4)) & 0xf;
        m == 0xf || m == (category >> (i * 4)) & 0xf
    })
}

/// spells.2da `range` code to meters.
fn range_meters(code: &str) -> f32 {
    match code.to_ascii_uppercase().as_str() {
        "P" => 0.0,
        "T" => 2.5,
        "S" => 8.0,
        "M" => 15.0,
        "L" => 25.0,
        "W" => 15.0,
        _ => 10.0,
    }
}

/// Effects that leave the creature unable to act.
const DEBILITATING: &[&str] = &[
    "EffectStunned",
    "EffectParalyze",
    "EffectDroidStun",
    "EffectHorrified",
    "EffectChoke",
    "EffectWhirlWind",
    "EffectForcePushed",
    "EffectForcePushTargeted",
    "EffectSleep",
    "EffectEntangle",
    "EffectConfused",
    "EffectFrightened",
    "EffectMindTrick",
    "EffectDroidScramble",
    "EffectCrush",
];

/// Totals of the active modifier effects on a creature.
#[derive(Debug, Clone, Copy, Default)]
pub struct Modifiers {
    pub attack: i32,
    pub defense: i32,
    pub damage: i32,
    pub abilities: [i32; 6],
    pub saves: [i32; 4],
    pub extra_attacks: i32,
    pub speed_pct: i32,
    pub force_resistance: i32,
    pub debilitated: bool,
}

/// One combat feat's effect on a round.
struct FeatMods {
    attack: i32,
    damage: i32,
    crit: i32,
    extra: u32,
    defense: i32,
}

fn feat_mods(feat: i32) -> Option<FeatMods> {
    let m = |attack, damage, crit, extra, defense| Some(FeatMods { attack, damage, crit, extra, defense });
    match feat {
        28 | 29 => m(-3, 5, 0, 0, 0),         // Power Attack / Power Blast
        17 | 18 => m(-3, 8, 0, 0, 0),         // Improved
        83 | 82 => m(-3, 10, 0, 0, 0),        // Master
        8 | 31 => m(0, 0, 2, 0, -5),          // Critical Strike / Sniper Shot
        19 | 20 => m(0, 0, 4, 0, -5),         // Improved
        81 | 77 => m(0, 0, 6, 0, -5),         // Master
        11 | 30 => m(-4, 0, 0, 1, -4),        // Flurry / Rapid Shot
        91 | 92 => m(-2, 0, 0, 1, -2),        // Improved
        53 => m(-2, 0, 0, 1, -2),             // Whirlwind Attack
        26 => m(-2, 0, 0, 2, -2),             // Multi Shot
        _ => None,
    }
}

impl World {
    fn spells_int(&self, spell: i32, col: &str) -> Option<i32> {
        let t = self.tables.get(&self.res, "spells").ok()?;
        t.int(usize::try_from(spell).ok()?, col)
    }

    fn spells_str(&self, spell: i32, col: &str) -> Option<String> {
        let t = self.tables.get(&self.res, "spells").ok()?;
        t.resref(usize::try_from(spell).ok()?, col)
    }

    /// Force point cost of a power (0 for item and innate abilities).
    pub fn spell_cost(&self, spell: i32) -> i32 {
        if self.spells_int(spell, "usertype") != Some(1) {
            return 0;
        }
        self.spells_int(spell, "forcepoints").unwrap_or(0)
    }

    fn is_hostile_spell(&self, spell: i32) -> bool {
        self.spells_str(spell, "forcehostile").is_some()
    }

    /// Items the creature can use for a CastSpell ability: the party's shared
    /// inventory for party members, otherwise its own.
    fn reachable_items(&self, id: ObjectId) -> Vec<ObjectId> {
        let mut items: Vec<ObjectId> = self.get(id).map(|o| o.inventory.clone()).unwrap_or_default();
        if self.party.contains(&id) && self.is_valid(self.player) && id != self.player {
            items.extend(self.get(self.player).map(|o| o.inventory.clone()).unwrap_or_default());
        }
        items
    }

    fn item_cast_spells(&mut self, item: ObjectId) -> Vec<i32> {
        self.item_info(item)
            .map(|i| i.properties.iter().filter(|p| u32::from(p.property) == PROPERTY_CAST_SPELL).map(|p| i32::from(p.subtype)).collect())
            .unwrap_or_default()
    }

    /// An item the creature can use to cast `spell`.
    fn item_for_spell(&mut self, id: ObjectId, spell: i32) -> Option<ObjectId> {
        self.reachable_items(id).into_iter().find(|&item| self.item_cast_spells(item).contains(&spell))
    }

    fn force_points(&self, id: ObjectId) -> i32 {
        self.get(id).and_then(|o| o.creature()).map_or(0, |c| c.force)
    }

    /// Whether the creature can use the talent right now.
    pub fn has_talent(&mut self, id: ObjectId, ty: i32, talent: i32) -> bool {
        match ty {
            TALENT_SPELL => {
                if self.spells_int(talent, "usertype") == Some(1) {
                    self.has_power(id, talent) && self.force_points(id) >= self.spell_cost(talent)
                } else {
                    self.has_power(id, talent) || self.item_for_spell(id, talent).is_some()
                }
            }
            TALENT_FEAT => self.has_feat(id, talent),
            TALENT_SKILL => self.skill_rank(id, talent.max(0) as usize) > 0,
            _ => false,
        }
    }

    /// (type, id, category, maxcr) of everything the creature can use.
    fn talent_candidates(&mut self, id: ObjectId) -> Vec<(i32, i32, i32, i32)> {
        let mut out = Vec::new();
        let powers = self.known_powers(id);
        let fp = self.force_points(id);
        for p in powers {
            if self.spell_cost(p) > fp {
                continue;
            }
            let cat = self.spells_int(p, "category").unwrap_or(0);
            out.push((TALENT_SPELL, p, cat, self.spells_int(p, "pips").unwrap_or(1)));
        }
        let mut seen = HashSet::new();
        for item in self.reachable_items(id) {
            for sp in self.item_cast_spells(item) {
                if seen.insert(sp) {
                    let cat = self.spells_int(sp, "category").unwrap_or(0);
                    out.push((TALENT_SPELL, sp, cat, self.spells_int(sp, "inate").unwrap_or(1)));
                }
            }
        }
        if let Ok(t) = self.tables.get(&self.res, "feat") {
            for f in self.known_feats(id) {
                let r = usize::from(f);
                let cat = t.int(r, "category").unwrap_or(0);
                if cat > 0 && feat_mods(i32::from(f)).is_some() {
                    out.push((TALENT_FEAT, i32::from(f), cat, t.int(r, "maxcr").unwrap_or(1)));
                }
            }
        }
        out
    }

    fn talent_value(&mut self, ty: i32, id: i32) -> Value {
        self.engine.add(EngineData::Talent(Talent { ty, id }))
    }

    fn invalid_talent() -> Value {
        Value::Engine(EngineValue { kind: ENGINE_TALENT, handle: 0 })
    }

    fn talent_of(&self, v: Option<&Value>) -> Option<Talent> {
        match self.engine.get(v?)? {
            EngineData::Talent(t) => Some(t.clone()),
            _ => None,
        }
    }

    fn talent_category(&self, t: &Talent) -> i32 {
        match t.ty {
            TALENT_SPELL => self.spells_int(t.id, "category").unwrap_or(0),
            TALENT_FEAT => self
                .tables
                .get(&self.res, "feat")
                .ok()
                .and_then(|x| x.int(usize::try_from(t.id).unwrap_or(usize::MAX), "category"))
                .unwrap_or(0),
            _ => 0,
        }
    }

    // ---------------------------------------------------------------
    // Effects
    // ---------------------------------------------------------------

    /// Totals of the creature's active effects.
    pub fn modifiers(&self, id: ObjectId) -> Modifiers {
        let mut m = Modifiers::default();
        let Some(o) = self.get(id) else { return m };
        for ae in &o.effects {
            let Some(EngineData::Effect(e)) = self.engine.by_handle(ae.handle) else { continue };
            let arg = |n: usize| e.args.get(n).and_then(Value::as_int).unwrap_or(0);
            match e.name.as_str() {
                "EffectAttackIncrease" => m.attack += arg(0),
                "EffectAttackDecrease" => m.attack -= arg(0),
                "EffectACIncrease" => m.defense += arg(0),
                "EffectACDecrease" => m.defense -= arg(0),
                "EffectDamageIncrease" => m.damage += damage_bonus(arg(0)),
                "EffectDamageDecrease" => m.damage -= arg(0),
                "EffectAbilityIncrease" => m.abilities[arg(0).clamp(0, 5) as usize] += arg(1),
                "EffectAbilityDecrease" => m.abilities[arg(0).clamp(0, 5) as usize] -= arg(1),
                "EffectSavingThrowIncrease" | "EffectSavingThrowDecrease" => {
                    let v = if e.name.ends_with("Increase") { arg(1) } else { -arg(1) };
                    match arg(0) {
                        0 => m.saves.iter_mut().for_each(|s| *s += v),
                        s @ 1..=3 => m.saves[s as usize] += v,
                        _ => {}
                    }
                }
                "EffectModifyAttacks" => m.extra_attacks += arg(0),
                "EffectMovementSpeedIncrease" => m.speed_pct += arg(0),
                "EffectMovementSpeedDecrease" => m.speed_pct -= arg(0),
                "EffectForceResistanceIncrease" => m.force_resistance += arg(0),
                "EffectForceResistanceDecrease" => m.force_resistance -= arg(0),
                n if DEBILITATING.contains(&n) => m.debilitated = true,
                _ => {}
            }
        }
        m
    }

    /// Stunned, paralyzed, choked, ...: no actions this tick.
    pub fn is_debilitated(&self, id: ObjectId) -> bool {
        self.modifiers(id).debilitated
    }

    /// Movement speed multiplier from speed effects.
    pub fn speed_factor(&self, id: ObjectId) -> f32 {
        (1.0 + self.modifiers(id).speed_pct as f32 / 100.0).clamp(0.2, 3.0)
    }

    /// Effect and combat feat modifiers folded into combat numbers.
    pub(crate) fn apply_power_modifiers(&mut self, id: ObjectId, s: &mut CombatStats) {
        let m = self.modifiers(id);
        let old_dex = s.dex_mod();
        for (i, a) in s.abilities.iter_mut().enumerate() {
            *a = (i32::from(*a) + m.abilities[i]).clamp(1, 60) as u8;
        }
        let dex_delta = s.dex_mod() - old_dex;
        let dex_delta = match s.armor.max_dex {
            Some(cap) if old_dex >= cap => 0,
            Some(cap) => dex_delta.min(cap - old_dex),
            None => dex_delta,
        };
        s.defense += m.defense + dex_delta;
        s.bab += m.attack;
        s.main.damage_bonus += m.damage;
        if let Some(off) = s.offhand.as_mut() {
            off.damage_bonus += m.damage;
        }
        s.extra_attacks += m.extra_attacks.max(0) as u32;
        if let Some(&(feat, until)) = self.powers.pending_feat.get(&id)
            && until > self.time
            && let Some(f) = feat_mods(feat)
        {
            s.bab += f.attack;
            s.defense += f.defense;
            s.main.damage_bonus += f.damage;
            s.main.crit_threat += f.crit;
            s.extra_attacks += f.extra;
        }
    }

    /// Temporary hit points and damage resistance soak up `amount`.
    pub(crate) fn absorb_damage(&mut self, target: ObjectId, amount: i32) -> i32 {
        let Some(o) = self.get(target) else { return amount };
        let mut left = amount;
        let mut spent = Vec::new();
        for ae in &o.effects {
            if left <= 0 {
                break;
            }
            let Some(EngineData::Effect(e)) = self.engine.by_handle(ae.handle) else { continue };
            let arg = |n: usize| e.args.get(n).and_then(Value::as_int).unwrap_or(0);
            match e.name.as_str() {
                "EffectTemporaryHitpoints" => {
                    let pool = self.powers.absorb_left.get(&ae.handle).copied().unwrap_or(arg(0));
                    let used = pool.min(left);
                    left -= used;
                    spent.push((ae.handle, pool - used));
                }
                "EffectDamageResistance" => {
                    let limit = arg(2);
                    let per_hit = arg(1).min(left);
                    if limit > 0 {
                        let pool = self.powers.absorb_left.get(&ae.handle).copied().unwrap_or(limit);
                        let used = per_hit.min(pool);
                        left -= used;
                        spent.push((ae.handle, pool - used));
                    } else {
                        left -= per_hit;
                    }
                }
                _ => {}
            }
        }
        let empty: Vec<u32> = spent.iter().filter(|(_, p)| *p <= 0).map(|(h, _)| *h).collect();
        for (h, p) in spent {
            self.powers.absorb_left.insert(h, p);
        }
        if !empty.is_empty() {
            if let Some(o) = self.get_mut(target) {
                o.effects.retain(|e| !empty.contains(&e.handle));
            }
            self.forget_effect_handles(&empty);
        }
        left.max(0)
    }

    // ---------------------------------------------------------------
    // Saves
    // ---------------------------------------------------------------

    /// Total save bonus (1 fortitude, 2 reflex, 3 will).
    pub fn save_bonus(&self, id: ObjectId, save: i32) -> i32 {
        let Some(c) = self.get(id).and_then(|o| o.creature()) else { return 0 };
        let col = match save {
            1 => "fortsave",
            2 => "refsave",
            _ => "willsave",
        };
        let mut base = 0;
        if let Ok(classes) = self.tables.get(&self.res, "classes") {
            for &(class, lvl) in &c.classes {
                let row = usize::try_from(class).unwrap_or(usize::MAX);
                if let Some(t) = classes.resref(row, "savingthrowtable").and_then(|n| self.tables.get(&self.res, &n).ok()) {
                    base += t.int(usize::try_from(lvl.max(1) - 1).unwrap_or(0), col).unwrap_or(0);
                }
            }
        }
        let m = self.modifiers(id);
        let ability = match save {
            1 => 2,
            2 => 1,
            _ => 4,
        };
        let score = i32::from(c.abilities[ability]) + m.abilities[ability];
        base + crate::progression::ability_mod(score) + m.saves[save.clamp(1, 3) as usize]
    }

    /// d20 + save bonus vs DC: 1 success, 0 failure.
    pub fn saving_throw(&mut self, id: ObjectId, save: i32, dc: i32) -> i32 {
        if !self.get(id).is_some_and(|o| o.kind == ObjectKind::Creature) {
            return 0;
        }
        let roll = self.powers.roll(20);
        let ok = roll == 20 || (roll != 1 && roll + self.save_bonus(id, save) >= dc);
        i32::from(ok)
    }

    /// GetSpellSaveDC: 5 + caster level + the better of Wis/Cha modifiers.
    /// Guess: no install data holds the DC formula (docs/notes/progression.md).
    fn spell_save_dc(&self, caster: ObjectId) -> i32 {
        let Some(c) = self.get(caster).and_then(|o| o.creature()) else { return 10 };
        let m = self.modifiers(caster);
        let wis = crate::progression::ability_mod(i32::from(c.abilities[4]) + m.abilities[4]);
        let cha = crate::progression::ability_mod(i32::from(c.abilities[5]) + m.abilities[5]);
        5 + self.level_of(caster) + wis.max(cha)
    }

    // ---------------------------------------------------------------
    // Casting
    // ---------------------------------------------------------------

    /// Queues a talent use (the player's action menu and the AI).
    pub fn use_talent(&mut self, caster: ObjectId, ty: i32, id: i32, target: ObjectId) {
        let item = if ty == TALENT_SPELL && self.spells_int(id, "usertype") != Some(1) && !self.has_power(caster, id) {
            self.item_for_spell(caster, id)
        } else {
            None
        };
        self.push_action(caster, Action::Cast(Box::new(new_cast(ty, id, Target::Object(target), false, false, item))));
    }

    /// Talents the player can pick for `target` right now: (type, id, name,
    /// icon, cost). Hostile powers only on enemies, friendly ones on allies.
    pub fn usable_talents(&mut self, caster: ObjectId, target: ObjectId) -> Vec<TalentView> {
        let hostile_target = target != OBJECT_INVALID && target != caster && self.is_enemy(caster, target);
        let mut out = Vec::new();
        let names = self.tables.get(&self.res, "spells").ok();
        for (ty, id, _cat, _) in self.talent_candidates(caster) {
            let (name, icon, hostile) = if ty == TALENT_SPELL {
                let r = usize::try_from(id).unwrap_or(usize::MAX);
                (
                    names.as_ref().and_then(|t| t.int(r, "name")),
                    names.as_ref().and_then(|t| t.resref(r, "iconresref")),
                    self.is_hostile_spell(id),
                )
            } else {
                let t = self.tables.get(&self.res, "feat").ok();
                let r = usize::try_from(id).unwrap_or(usize::MAX);
                (t.as_ref().and_then(|t| t.int(r, "name")), t.as_ref().and_then(|t| t.resref(r, "icon")), true)
            };
            if hostile != hostile_target {
                continue;
            }
            let name = match (name, &self.talk) {
                (Some(s), Some(t)) if s >= 0 => t.get(s as u32).unwrap_or("").to_owned(),
                _ => String::new(),
            };
            if name.is_empty() {
                continue;
            }
            // spells.2da has no icon for item powers (medpacs, stims, grenades,
            // shields): the item that casts the power shows its own icon, as
            // the use path picks that item (`item_for_spell`).
            let icon = match icon.filter(|i| !i.is_empty()) {
                Some(i) => Some(i),
                None if ty == TALENT_SPELL => self.item_for_spell(caster, id).and_then(|item| self.item_def_of(item)).and_then(|d| d.icon.clone()),
                None => None,
            };
            out.push(TalentView { ty, id, name, icon, cost: if ty == TALENT_SPELL { self.spell_cost(id) } else { 0 }, hostile });
        }
        out.sort_by(|a, b| (a.ty, &a.name).cmp(&(b.ty, &b.name)));
        out.dedup_by(|a, b| a.ty == b.ty && a.id == b.id);
        out
    }

    pub(crate) fn step_cast(&mut self, id: ObjectId, mut c: CastAction, dt: f32) -> (bool, Option<Action>) {
        if !c.started {
            // Combat feats: arm the next round and attack.
            if c.talent_type == TALENT_FEAT {
                let target = match c.target {
                    Target::Object(t) => t,
                    Target::Point(_) => OBJECT_INVALID,
                };
                if self.has_feat(id, c.id) || c.cheat {
                    self.powers.last_feat.insert(id, c.id);
                    if feat_mods(c.id).is_some() && self.is_valid(target) {
                        self.powers.pending_feat.insert(id, (c.id, self.time + f64::from(ROUND) + 0.5));
                        self.emit(WorldEvent::FeatUsed { creature: id, target, feat: c.id });
                        if let Some(o) = self.get_mut(id) {
                            o.actions.insert(1, Action::Attack(target));
                        }
                    }
                }
                return (true, None);
            }
            if c.talent_type != TALENT_SPELL {
                return (true, None);
            }
            let spell = c.id;
            if !c.fake && !c.cheat {
                let power = self.spells_int(spell, "usertype") == Some(1);
                let ok = if power {
                    self.has_power(id, spell) && self.force_points(id) >= self.spell_cost(spell)
                } else {
                    self.has_power(id, spell) || c.item.is_some_and(|i| self.get(i).is_some()) || {
                        c.item = self.item_for_spell(id, spell);
                        c.item.is_some()
                    }
                };
                if !ok {
                    self.powers.power_failed.insert(id);
                    return (true, None);
                }
            }
            // Close to range.
            let me = self.get(id).map_or(Vec3::ZERO, |o| o.position);
            let tp = match c.target {
                Target::Object(t) => self.get(t).map(|o| o.position),
                Target::Point(p) => Some(p),
            };
            let Some(tp) = tp else { return (true, None) };
            let range = self.spells_str(spell, "range").map_or(10.0, |r| range_meters(&r));
            let self_target = matches!(c.target, Target::Object(t) if t == id);
            if !self_target && range > 0.0 && (tp - me).truncate().length() > range {
                let (moved, arrived) = self.move_towards(id, tp, true, range * 0.9, dt);
                c.elapsed += dt;
                if !arrived && (moved > 1e-3 || c.elapsed < 4.0) {
                    return (false, Some(Action::Cast(Box::new(c))));
                }
                if !arrived {
                    self.powers.power_failed.insert(id);
                    return (true, None);
                }
                c.elapsed = 0.0;
            }
            if !self_target
                && let Some(b) = bearing_towards(me, tp)
                && let Some(o) = self.get_mut(id)
            {
                o.bearing = b;
            }
            // A lightsaber throw needs a saber in hand, and a target at least 5 m away
            // unless it is a cheat (the saber's damage is only dealt when it is thrown).
            if !c.fake && saber_catch(self, spell).is_some() && !self.saber_throw_ready(id, tp, c.cheat) {
                self.powers.power_failed.insert(id);
                return (true, None);
            }
            let cost = if c.fake || c.cheat { 0 } else { self.spell_cost(spell) };
            if let Some(cr) = self.get_mut(id).and_then(|o| o.creature_mut()) {
                cr.force = (cr.force - cost).max(0);
            }
            let conj = self.spells_int(spell, "conjtime").unwrap_or(170) as f32 / 1000.0;
            let cast = self.spells_int(spell, "casttime").unwrap_or(1330) as f32 / 1000.0;
            let hostile = self.is_hostile_spell(spell);
            let target_obj = match c.target {
                Target::Object(t) => t,
                Target::Point(_) => OBJECT_INVALID,
            };
            let in_fight = self.in_combat(id) || (hostile && target_obj != OBJECT_INVALID);
            c.impact_at = conj + cast * 0.5;
            c.duration = if in_fight { ROUND.max(conj + cast) } else { conj + cast };
            c.started = true;
            c.elapsed = 0.0;
            self.powers.last_power.insert(id, spell);
            self.powers.power_failed.remove(&id);
            self.emit(WorldEvent::SpellCastStarted {
                caster: id,
                target: target_obj,
                spell,
                anim: self.spells_str(spell, "castanim").unwrap_or_default(),
                conj_anim: self.spells_str(spell, "conjanim").unwrap_or_default(),
                sound: self.spells_str(spell, "castsound"),
                duration: c.duration,
                hostile,
            });
            // A grenade, rocket or thrown saber leaves at conjtime (projectiles.rs).
            // A fake cast runs no impact, so nothing is thrown for it either.
            if !c.fake {
                if saber_catch(self, spell).is_some() {
                    // The saber reaches stop i at conjtime + leg * (i + 1), the same
                    // times projectiles.rs draws (the last stop is the impact).
                    let targets = self.saber_targets(spell, c.target);
                    let leg = (c.impact_at - conj) / targets.len().max(1) as f32;
                    c.saber_hits = targets.iter().enumerate().map(|(i, &t)| (conj + leg * (i + 1) as f32, t)).collect();
                }
                self.emit_spell_projectile(id, spell, c.target, conj, c.impact_at, conj + cast);
            }
            return (false, Some(Action::Cast(Box::new(c))));
        }
        c.elapsed += dt;
        while c.saber_hits.first().is_some_and(|&(at, _)| c.elapsed >= at) {
            let (_, target) = c.saber_hits.remove(0);
            self.saber_hit(id, target);
        }
        if !c.impacted && c.elapsed >= c.impact_at {
            c.impacted = true;
            if !c.fake {
                self.cast_impact(id, &c);
            }
        }
        if c.elapsed >= c.duration {
            if self.in_combat(id) && !self.is_debilitated(id) {
                self.queue_event(id, "ScriptEndRound", ScriptCtx::default());
                self.pump();
            }
            return (true, None);
        }
        (false, Some(Action::Cast(Box::new(c))))
    }

    fn cast_impact(&mut self, caster: ObjectId, c: &CastAction) {
        let spell = c.id;
        let (target, location) = match c.target {
            Target::Object(t) => (t, self.get(t).map_or(Vec3::ZERO, |o| o.position)),
            Target::Point(p) => (OBJECT_INVALID, p),
        };
        let harmful = self.is_hostile_spell(spell);
        let Some(script) = self.spells_str(spell, "impactscript") else { return };
        let mark = self.engine.len() as u32;
        self.powers.casts.push(ActiveCast {
            spell,
            caster,
            target,
            location,
            item: c.item.unwrap_or(OBJECT_INVALID),
            cost: if c.cheat { 0 } else { self.spell_cost(spell) },
            harmful,
        });
        self.run_script(&script, caster, ScriptCtx::default());
        self.powers.casts.pop();
        // Effects created while the impact script ran belong to the spell.
        let new: Vec<u32> = self.iter().flat_map(|o| o.effects.iter().map(|e| e.handle)).filter(|&h| h > mark).collect();
        for h in new {
            self.powers.effect_spell.insert(h, spell);
        }
        if let Some(item) = c.item {
            self.consume_charge(item);
        }
        if harmful && target != OBJECT_INVALID && target != caster {
            self.powers.spell_at.insert(target, (caster, spell, true));
        }
        let ok = !self.powers.power_failed.contains(&caster);
        self.emit(WorldEvent::SpellImpact { caster, target, spell, hostile: harmful, success: ok });
        self.pump();
    }

    fn consume_charge(&mut self, item: ObjectId) {
        let left = {
            let Some(it) = self.get_mut(item).and_then(|o| o.item_mut()) else { return };
            it.stack_size -= 1;
            it.stack_size
        };
        if left <= 0 {
            self.detach_item(item);
            if let Some(o) = self.get_mut(item) {
                o.destroyed = true;
            }
        }
    }

    /// Per tick: effect timers, then once a second Force/vitality regeneration (regen.rs).
    pub(crate) fn run_powers(&mut self, dt: f32) {
        let mut expired: Vec<(ObjectId, u32)> = Vec::new();
        for o in self.objects.values_mut() {
            if o.destroyed || o.effects.is_empty() {
                continue;
            }
            for e in &mut o.effects {
                if let Some(r) = e.remaining.as_mut() {
                    *r -= dt;
                    if *r <= 0.0 {
                        expired.push((o.id, e.handle));
                    }
                }
            }
            o.effects.retain(|e| e.remaining.is_none_or(|r| r > 0.0));
        }
        let expired: Vec<u32> = expired.into_iter().map(|(_, h)| h).collect();
        self.forget_effect_handles(&expired);
        let now = self.time;
        self.powers.pending_feat.retain(|_, (_, until)| *until > now);
        self.powers.regen_in -= dt;
        if self.powers.regen_in > 0.0 {
            return;
        }
        self.powers.regen_in = 1.0;
        self.run_regen(1.0);
    }

    /// Spell, talent, save and effect routines. None = not handled here.
    pub(crate) fn power_call(&mut self, name: &str, me: ObjectId, a: &[Value]) -> Option<Value> {
        let int = |n: usize| a.get(n).and_then(Value::as_int).unwrap_or(0);
        let obj = |n: usize| a.get(n).and_then(Value::as_object).unwrap_or(OBJECT_INVALID);
        let obj_or_me = |n: usize| match a.get(n).and_then(Value::as_object) {
            Some(o) if o != OBJECT_INVALID => o,
            _ => me,
        };
        if let Some(v) = self.progression_call(name, me, a) {
            return Some(v);
        }
        let cast = self.powers.casts.last().copied();
        Some(match name {
            // ---- spell context ----
            "GetSpellId" if cast.is_some() => Value::Int(cast?.spell),
            "GetSpellTargetObject" | "GetSpellTarget" | "GetAttemptedSpellTarget" if cast.is_some() => Value::Object(cast?.target),
            "GetSpellTargetLocation" if cast.is_some() => {
                let c = cast?;
                self.engine.add(EngineData::Location(crate::engine::Location { position: c.location, facing: 0.0, area: self.area_id }))
            }
            "GetSpellCastItem" if cast.is_some() => Value::Object(cast?.item),
            "GetLastSpellCaster" => match cast {
                Some(c) => Value::Object(c.caster),
                None => match self.powers.spell_at.get(&me) {
                    Some(&(caster, _, _)) => Value::Object(caster),
                    None => return None,
                },
            },
            "GetLastSpell" => Value::Int(cast.map(|c| c.spell).or_else(|| self.powers.spell_at.get(&me).map(|x| x.1)).unwrap_or(-1)),
            "GetLastSpellHarmful" => Value::Int(i32::from(cast.map(|c| c.harmful).or_else(|| self.powers.spell_at.get(&me).map(|x| x.2)).unwrap_or(false))),
            "GetSpellSaveDC" => Value::Int(self.spell_save_dc(cast.map_or(me, |c| c.caster))),
            "GetSpellForcePointCost" => Value::Int(cast.map_or(0, |c| c.cost)),
            "GetSpellBaseForcePointCost" => Value::Int(self.spells_int(int(0), "forcepoints").unwrap_or(0)),
            "GetCasterLevel" => Value::Int(self.level_of(obj_or_me(0))),
            "GetMetaMagicFeat" => Value::Int(0),
            "GetLastForcePowerUsed" => Value::Int(self.powers.last_power.get(&obj_or_me(0)).copied().unwrap_or(-1)),
            "GetLastCombatFeatUsed" => Value::Int(self.powers.last_feat.get(&obj_or_me(0)).copied().unwrap_or(-1)),
            "GetWasForcePowerSuccessful" => Value::Int(i32::from(!self.powers.power_failed.contains(&obj_or_me(0)))),
            "SetForcePowerUnsuccessful" => {
                let id = obj_or_me(1);
                if int(0) != 0 {
                    self.powers.power_failed.insert(id);
                } else {
                    self.powers.power_failed.remove(&id);
                }
                Value::Void
            }
            // ---- saves ----
            "FortitudeSave" => Value::Int(self.saving_throw(obj(0), 1, int(1))),
            "ReflexSave" => Value::Int(self.saving_throw(obj(0), 2, int(1))),
            "WillSave" => Value::Int(self.saving_throw(obj(0), 3, int(1))),
            "GetFortitudeSavingThrow" => Value::Int(self.save_bonus(obj(0), 1)),
            "GetReflexSavingThrow" => Value::Int(self.save_bonus(obj(0), 2)),
            "GetWillSavingThrow" => Value::Int(self.save_bonus(obj(0), 3)),
            "ResistForce" => {
                let (src, tgt) = (obj(0), obj(1));
                let fr = self.modifiers(tgt).force_resistance;
                if fr <= 0 {
                    Value::Int(0)
                } else {
                    let roll = self.powers.roll(20) + self.level_of(src);
                    Value::Int(i32::from(roll < fr))
                }
            }
            // ---- knowledge ----
            "GetHasSpell" | "GetSpellAcquired" => {
                let id = obj_or_me(1);
                let sp = int(0);
                Value::Int(i32::from(self.has_power(id, sp) || (name == "GetHasSpell" && self.item_for_spell(id, sp).is_some())))
            }
            "GetHasSpellEffect" => {
                let id = obj_or_me(1);
                let sp = int(0);
                let has = self.get(id).is_some_and(|o| o.effects.iter().any(|e| self.powers.effect_spell.get(&e.handle) == Some(&sp)));
                Value::Int(i32::from(has))
            }
            "GetHasFeatEffect" => Value::Int(0),
            "GetEffectSpellId" => match a.first() {
                Some(Value::Engine(e)) => Value::Int(self.powers.effect_spell.get(&e.handle).copied().unwrap_or(-1)),
                _ => Value::Int(-1),
            },
            "GetSpellFormMask" => Value::Int(0),
            "SetBonusForcePoints" | "AddBonusForcePoints" => {
                let id = obj(0);
                let n = int(1);
                let old = self.powers.bonus_force.get(&id).copied().unwrap_or(0);
                let new = if name == "SetBonusForcePoints" { n } else { old + n };
                self.powers.bonus_force.insert(id, new);
                if let Some(c) = self.get_mut(id).and_then(|o| o.creature_mut()) {
                    c.max_force = (c.max_force + new - old).max(0);
                    c.force = c.force.min(c.max_force);
                }
                Value::Void
            }
            "GetBonusForcePoints" => Value::Int(self.powers.bonus_force.get(&obj(0)).copied().unwrap_or(0)),
            // ---- talents ----
            "TalentSpell" => self.talent_value(TALENT_SPELL, int(0)),
            "TalentFeat" => self.talent_value(TALENT_FEAT, int(0)),
            "TalentSkill" => self.talent_value(TALENT_SKILL, int(0)),
            "GetIdFromTalent" => Value::Int(self.talent_of(a.first()).map_or(-1, |t| t.id)),
            "GetTypeFromTalent" => Value::Int(self.talent_of(a.first()).map_or(-1, |t| t.ty)),
            "GetCategoryFromTalent" => Value::Int(self.talent_of(a.first()).map_or(0, |t| self.talent_category(&t))),
            "GetCreatureHasTalent" => {
                let id = obj_or_me(1);
                match self.talent_of(a.first()) {
                    Some(t) => Value::Int(i32::from(self.has_talent(id, t.ty, t.id))),
                    None => Value::Int(0),
                }
            }
            "GetCreatureTalentBest" | "GetCreatureTalentRandom" => {
                let best = name == "GetCreatureTalentBest";
                let mask = int(0);
                let (cr_max, who) = if best { (int(1), obj_or_me(2)) } else { (i32::MAX, obj_or_me(1)) };
                let (ex_ty, ex_id) = if best { (a.get(4).and_then(Value::as_int).unwrap_or(-1), a.get(5).and_then(Value::as_int).unwrap_or(-1)) } else { (-1, -1) };
                let mut cands: Vec<(i32, i32, i32)> = self
                    .talent_candidates(who)
                    .into_iter()
                    .filter(|&(ty, id, cat, cr)| category_matches(mask, cat) && cr <= cr_max && !(ty == ex_ty && (ex_id < 0 || id == ex_id)))
                    .map(|(ty, id, _, cr)| (ty, id, cr))
                    .collect();
                if cands.is_empty() {
                    Self::invalid_talent()
                } else {
                    let pick = if best {
                        let top = cands.iter().map(|c| c.2).max().unwrap_or(0);
                        cands.retain(|c| c.2 == top);
                        cands[(self.powers.roll(cands.len() as i32) - 1) as usize]
                    } else {
                        cands[(self.powers.roll(cands.len() as i32) - 1) as usize]
                    };
                    self.talent_value(pick.0, pick.1)
                }
            }
            "ActionUseTalentOnObject" | "ActionUseTalentAtLocation" => {
                let Some(t) = self.talent_of(a.first()) else { return Some(Value::Void) };
                let target = if name == "ActionUseTalentOnObject" {
                    Target::Object(obj(1))
                } else {
                    Target::Point(a.get(1).and_then(|v| self.engine.location(v)).map_or(Vec3::ZERO, |l| l.position))
                };
                let item = if t.ty == TALENT_SPELL && self.spells_int(t.id, "usertype") != Some(1) && !self.has_power(me, t.id) {
                    self.item_for_spell(me, t.id)
                } else {
                    None
                };
                self.push_action(me, Action::Cast(Box::new(new_cast(t.ty, t.id, target, false, false, item))));
                Value::Void
            }
            "ActionUseFeat" => {
                self.push_action(me, Action::Cast(Box::new(new_cast(TALENT_FEAT, int(0), Target::Object(obj(1)), false, false, None))));
                Value::Void
            }
            "ActionCastSpellAtObject" | "ActionCastFakeSpellAtObject" => {
                let fake = name.contains("Fake");
                let cheat = !fake && int(3) != 0;
                self.push_action(me, Action::Cast(Box::new(new_cast(TALENT_SPELL, int(0), Target::Object(obj(1)), fake, cheat, None))));
                Value::Void
            }
            "ActionCastSpellAtLocation" | "ActionCastFakeSpellAtLocation" => {
                let fake = name.contains("Fake");
                let cheat = !fake && int(3) != 0;
                let p = a.get(1).and_then(|v| self.engine.location(v)).map_or(Vec3::ZERO, |l| l.position);
                self.push_action(me, Action::Cast(Box::new(new_cast(TALENT_SPELL, int(0), Target::Point(p), fake, cheat, None))));
                Value::Void
            }
            // ---- events ----
            "EventSpellCastAt" => {
                let ev = format!("spellcastat:{}:{}:{}", obj(0), int(1), int(2));
                self.engine.add(EngineData::Event(crate::engine::Event::Other(ev)))
            }
            "SignalEvent" => {
                let target = obj(0);
                let crate::engine::Event::Other(ev) = a.get(1).and_then(|v| self.engine.event(v)).cloned()? else { return None };
                let rest = ev.strip_prefix("spellcastat:")?;
                let mut it = rest.split(':').map(|x| x.parse::<i64>().unwrap_or(0));
                let (caster, spell, harmful) = (it.next().unwrap_or(0) as ObjectId, it.next().unwrap_or(0) as i32, it.next().unwrap_or(0) != 0);
                self.powers.spell_at.insert(target, (caster, spell, harmful));
                if self.get(target).is_some_and(|o| o.kind == ObjectKind::Creature) {
                    self.queue_event(target, "ScriptSpellAt", ScriptCtx::default());
                }
                Value::Void
            }
            // ---- instant Force point effects ----
            "ApplyEffectToObject" if int(0) == 0 => {
                let target = obj(2);
                let e = a.get(1).and_then(|v| self.engine.effect(v)).cloned()?;
                let n = e.args.first().and_then(Value::as_int).unwrap_or(0);
                match e.name.as_str() {
                    "EffectDamageForcePoints" => {
                        if let Some(c) = self.get_mut(target).and_then(|o| o.creature_mut()) {
                            c.force = (c.force - n).max(0);
                        }
                    }
                    "EffectForceDrain" => {
                        let taken = self.get(target).and_then(|o| o.creature()).map_or(0, |c| c.force.min(n));
                        if let Some(c) = self.get_mut(target).and_then(|o| o.creature_mut()) {
                            c.force -= taken;
                        }
                        if let Some(c) = self.get_mut(e.creator).and_then(|o| o.creature_mut()) {
                            c.force = (c.force + taken).min(c.max_force);
                        }
                    }
                    _ => return None,
                }
                Value::Void
            }
            _ => return None,
        })
    }
}

/// What the action menu shows for one usable talent.
#[derive(Debug, Clone, PartialEq)]
pub struct TalentView {
    pub ty: i32,
    pub id: i32,
    pub name: String,
    pub icon: Option<String>,
    pub cost: i32,
    pub hostile: bool,
}

fn new_cast(talent_type: i32, id: i32, target: Target, fake: bool, cheat: bool, item: Option<ObjectId>) -> CastAction {
    CastAction { talent_type, id, target, fake, cheat, elapsed: 0.0, started: false, impacted: false, duration: 0.0, impact_at: 0.0, item, saber_hits: Vec::new() }
}

/// DAMAGE_BONUS_* constant to an average bonus.
fn damage_bonus(code: i32) -> i32 {
    match code {
        1..=5 => code,
        6 => 2,  // 1d4
        7 => 3,  // 1d6
        8 => 4,  // 1d8
        9 => 5,  // 1d10
        10 => 7, // 2d6
        _ => code.clamp(0, 20),
    }
}
