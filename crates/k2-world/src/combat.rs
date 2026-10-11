//! Real-time-with-rounds combat (KotOR style), headless.
//!
//! - A combat round is 3 s. An attacker in range resolves its attacks at
//!   fixed points in the round (1 to 3 attacks, see
//!   [`CombatStats::attacks_per_round`]); at the end of the round the
//!   attacker's ScriptEndRound fires. The default AI (k_def_combend01 ->
//!   k_ai_master 1003 -> GN_DetermineCombatRound) re-issues ActionAttack from
//!   there, so the game's own scripts drive target choice.
//! - Attack roll: d20 + BAB + ability mod (str melee, dex ranged) + weapon
//!   bonuses vs Defense. Natural 1 misses, natural 20 hits. A roll inside the
//!   weapon's threat range is a critical if a second roll also hits;
//!   damage is multiplied by crithitmult.
//! - Damage: weapon dice + str mod (melee only) + bonuses, minimum 1.
//!   OnDamaged / OnDeath fire, death plays the dead loop, and corpses are
//!   destroyed after a delay unless lootable, plot or SetIsDestroyable(FALSE).
//! - Perception: on top of the world's party perception, creatures with an
//!   OnNotice script notice non-party creatures that are hostile to them
//!   (party members spotting enemies, predator/prey).
//!
//! See docs/notes/combat.md.

use std::collections::{HashMap, HashSet};

use glam::Vec3;
use k2_script::{ObjectId, Value, OBJECT_INVALID};

use crate::factions::{Factions, FRIENDLY_MIN, HOSTILE_MAX};
use crate::items::{ItemInfo, WeaponInfo};
use crate::object::{bearing_towards, Action, ObjectKind};
use crate::party::GET_UP_DELAY;
use crate::projectiles::ProjectileEnd;
use crate::stats::{CombatStats, TemplateInfo, SLOT_LEFTWEAPON};
use crate::{ScriptCtx, TimerKind, World, WorldEvent};
use serde::{Deserialize, Serialize};

/// Seconds per combat round.
pub const ROUND: f32 = 3.0;
/// A creature counts as in combat this long after its last attack or hit.
const COMBAT_LINGER: f64 = 6.0;
/// With no living hostile aware of it, a creature leaves combat this long
/// after its last attack or hit.
const COMBAT_TAIL: f64 = 2.0;
/// Corpses without loot are removed this long after death.
const CORPSE_SECONDS: f32 = 12.0;
const PERCEPTION_INTERVAL: f32 = 0.5;

/// What happened to one attack (ATTACK_RESULT_* values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackResult {
    Hit = 1,
    Critical = 2,
    Miss = 4,
}

/// Per-creature combat memory, read by the GetLast* routines.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    pub last_attacker: ObjectId,
    pub last_damager: ObjectId,
    pub last_hostile_actor: ObjectId,
    pub last_hostile_target: ObjectId,
    pub attack_target: ObjectId,
    pub attempted_target: ObjectId,
    pub last_killer: ObjectId,
    pub last_attack_result: i32,
    pub last_damage: i32,
    /// World time until which the creature counts as in combat.
    pub combat_until: f64,
}

impl Default for Memory {
    fn default() -> Self {
        Self {
            last_attacker: OBJECT_INVALID,
            last_damager: OBJECT_INVALID,
            last_hostile_actor: OBJECT_INVALID,
            last_hostile_target: OBJECT_INVALID,
            attack_target: OBJECT_INVALID,
            attempted_target: OBJECT_INVALID,
            last_killer: OBJECT_INVALID,
            last_attack_result: 0,
            last_damage: 0,
            combat_until: 0.0,
        }
    }
}

/// An attacker's round in progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Round {
    target: ObjectId,
    elapsed: f32,
    /// (time into the round, off-hand) per attack.
    schedule: Vec<(f32, bool)>,
    next: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CombatSystem {
    pub factions: Factions,
    #[serde(skip)]
    pub(crate) templates: HashMap<String, TemplateInfo>,
    #[serde(skip)]
    pub(crate) items: HashMap<String, Option<ItemInfo>>,
    rounds: HashMap<ObjectId, Round>,
    pub memory: HashMap<ObjectId, Memory>,
    /// SetIsDestroyable(FALSE): the corpse stays.
    pub keep_corpse: HashSet<ObjectId>,
    /// SetNPCAIStyle per creature (NPC_AISTYLE_*; unset = 0).
    pub ai_style: HashMap<ObjectId, i32>,
    pub party_ai_style: i32,
    /// GetFirst/NextFactionMember cursor.
    #[serde(skip)]
    faction_iter: (Vec<ObjectId>, usize),
    /// Attackers closing in: seconds without progress.
    approach_stuck: HashMap<ObjectId, f32>,
    rng: u64,
    perceive_in: f32,
    /// World time of the last nudge OnNotice per idle hostile. Saved: a
    /// loaded world must nudge on the same frames as the live one.
    pub(crate) renotice: HashMap<ObjectId, f64>,
    /// Human-readable round log (harness); only filled when `log_enabled`.
    #[serde(skip)]
    pub log: Vec<String>,
    #[serde(skip)]
    pub log_enabled: bool,
    /// Seconds into the current swing of attackers bashing an object (bash.rs).
    #[serde(skip)]
    pub(crate) bash_clock: HashMap<ObjectId, f32>,
    /// Attackers closing in on an object to bash: seconds without progress.
    #[serde(skip)]
    pub(crate) bash_stuck: HashMap<ObjectId, f32>,
    /// Attackers' chosen standing point by the object they bash (None: unreachable).
    #[serde(skip)]
    pub(crate) bash_stand: HashMap<ObjectId, (ObjectId, Option<Vec3>)>,
}

fn int(a: &[Value], n: usize) -> i32 {
    a.get(n).and_then(Value::as_int).unwrap_or(0)
}

fn obj(a: &[Value], n: usize) -> ObjectId {
    a.get(n).and_then(Value::as_object).unwrap_or(OBJECT_INVALID)
}

fn o(v: ObjectId) -> Value {
    Value::Object(v)
}

fn b(v: bool) -> Value {
    Value::Int(i32::from(v))
}

/// How far a SpeakString is heard (m), by TALKVOLUME_* (nwscript 0-4:
/// talk, whisper, shout, silent talk, silent shout). The install does not state
/// the ranges; these are the Neverwinter Nights engine's values for the same
/// constants (a guess, marked as such in docs/notes/combat.md).
fn talk_volume_range(volume: i32) -> f32 {
    match volume {
        1 => 3.0,
        2 | 4 => 40.0,
        _ => 20.0,
    }
}

/// Animation family of a weapon (animations.2da). The weapon's wield class is
/// not the family for blasters: pistol (wield 4) plays the single-blaster
/// family 5, rifle (5) the rifle family 7, heavy (6) the heavy family 9, and
/// two pistols (dual blasters) the dual family 6. Melee wields 1-3 and the
/// unarmed 8 are their own family. From the animations.2da descriptions, not
/// yet checked in game.
pub fn blaster_family(wield: i32, dual_pistols: bool) -> i32 {
    match wield {
        4 if dual_pistols => 6,
        4 => 5,
        5 => 7,
        6 => 9,
        other => other,
    }
}

/// Blaster families tried after a ranged attack's own family: single, rifle
/// and heavy. Their attacks carry no melee events (see docs/notes/combat.md).
const RANGED_FALLBACK_FAMILIES: [i32; 3] = [5, 7, 9];

/// Clip names for a ranged attack in animation family `family`, first choice
/// first. Never a melee family: a blaster that swings its butt looks like a
/// melee attack.
pub fn ranged_attack_clips(family: i32, k: usize) -> Vec<String> {
    let mut v = Vec::new();
    for f in std::iter::once(family).chain(RANGED_FALLBACK_FAMILIES) {
        v.push(format!("b{f}a{k}"));
        v.push(format!("b{f}a1"));
    }
    v
}

/// In-combat stance clips, first choice first, for an animation family: the
/// family's ready clip g<family>r1, then the creature ready clips that creature
/// and droid bodies carry (creadyr, creadyrtw). Unarmed (family 8) tries g10r1
/// (unarmed complex) before the natural g8r1: an inference from the install,
/// where every human chain carries both.
pub fn ready_clips(family: i32) -> Vec<String> {
    let mut v = Vec::new();
    if family == 8 {
        v.push("g10r1".to_string());
    }
    v.push(format!("g{family}r1"));
    v.push("creadyr".to_string());
    v.push("creadyrtw".to_string());
    v
}

/// Every clip an attack may play, first choice first, duplicates dropped.
/// `dual_pistols`: the attacker holds a pistol in each hand (family 6).
/// A ranged attack has no melee or unarmed fallback: a blaster that finds no
/// blaster clip stays on its idle instead of swinging.
pub fn attack_clip_list(wield: i32, ranged: bool, dual_pistols: bool, k: usize, droid: bool) -> Vec<String> {
    let mut list = Vec::new();
    // Droids and beasts: their supermodels carry b0a* / ca* attacks.
    if droid {
        list.push(format!("b0a{k}"));
    }
    match (wield, ranged) {
        (0, _) => list.extend([format!("g8a{k}"), "g8a1".into()]),
        (wield, true) => list.extend(ranged_attack_clips(blaster_family(wield, dual_pistols), k)),
        (wield, false) => list.extend([format!("g{wield}a{k}"), format!("g{wield}a1"), format!("c{wield}a{k}")]),
    }
    list.push(format!("b0a{k}"));
    list.push("b0a1".into());
    if !ranged {
        list.push("g8a1".into());
    }
    let mut out: Vec<String> = Vec::with_capacity(list.len());
    for name in list {
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

impl CombatSystem {
    fn next_u32(&mut self) -> u32 {
        if self.rng == 0 {
            self.rng = 0x2545_f491_4f6c_dd1d;
        }
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32
    }

    pub fn roll(&mut self, die: i32) -> i32 {
        if die <= 1 {
            return die.max(0);
        }
        (self.next_u32() % die as u32) as i32 + 1
    }

    pub fn dice(&mut self, n: i32, die: i32) -> i32 {
        (0..n.max(0)).map(|_| self.roll(die)).sum()
    }

    pub fn seed(&mut self, seed: u64) {
        self.rng = seed;
    }

    fn mem(&mut self, id: ObjectId) -> &mut Memory {
        self.memory.entry(id).or_default()
    }
}

impl World {
    // ---------------------------------------------------------------
    // Factions
    // ---------------------------------------------------------------

    pub(crate) fn ensure_factions(&mut self) {
        if self.combat.factions.is_empty() {
            self.combat.factions = Factions::load(&self.res, &self.tables);
        }
    }

    /// STANDARD_FACTION_* of an object (party members are Friendly_1).
    pub fn faction_of(&self, id: ObjectId) -> i32 {
        if self.party.contains(&id) {
            return 2;
        }
        let Some(o) = self.get(id) else { return 5 };
        match (o.creature(), o.trap.as_deref()) {
            (Some(c), _) => c.faction,
            // Traps carry their own faction (usually 13, Trap).
            (None, Some(t)) => t.faction,
            _ => 5,
        }
    }

    /// How `source` feels about `target`, 0..100.
    pub fn reputation(&mut self, source: ObjectId, target: ObjectId) -> i32 {
        self.ensure_factions();
        if source == target {
            return 100;
        }
        let (fs, ft) = (self.faction_of(source), self.faction_of(target));
        // The Trap faction's own row is all neutral; for a trap as the
        // source the engine goes by the target's row toward the trap
        // (k_trp_generic skips GetIsNeutral(oTarget, trap) targets: the
        // party rows are hostile to Trap, enemy rows neutral).
        if self.get(source).is_some_and(|o| o.trap.is_some() && o.creature().is_none()) {
            return self.combat.factions.reputation(ft, fs);
        }
        self.combat.factions.reputation(fs, ft)
    }

    pub fn is_enemy(&mut self, source: ObjectId, target: ObjectId) -> bool {
        self.is_valid(source) && self.is_valid(target) && self.reputation(source, target) <= HOSTILE_MAX
    }

    pub fn is_friend(&mut self, source: ObjectId, target: ObjectId) -> bool {
        self.is_valid(source) && self.is_valid(target) && self.reputation(source, target) >= FRIENDLY_MIN
    }

    /// Dead or knocked out: no attacks, not a target, no combat round.
    fn is_dead_creature(&self, id: ObjectId) -> bool {
        self.get(id).and_then(|o| o.creature()).is_none_or(|c| c.dead || c.down)
    }

    /// In a round, or within the linger after the last attack or hit. The
    /// full linger only holds while a living hostile is still aware of the
    /// creature; with none left (all dead, or out of sight and range) combat
    /// ends [`COMBAT_TAIL`] s after the last attack.
    pub fn in_combat(&self, id: ObjectId) -> bool {
        if self.combat.rounds.contains_key(&id) {
            return true;
        }
        let Some(left) = self.combat.memory.get(&id).map(|m| m.combat_until - self.time).filter(|l| *l > 0.0) else { return false };
        left > COMBAT_LINGER - COMBAT_TAIL || self.hostile_aware_of(id)
    }

    /// A living hostile that can see `id` (or that `id` can see).
    fn hostile_aware_of(&self, id: ObjectId) -> bool {
        let mine = self.faction_of(id);
        self.iter().any(|o| {
            o.id != id
                && o.kind == ObjectKind::Creature
                && o.creature().is_some_and(|c| !c.dead)
                && self.combat.factions.reputation(self.faction_of(o.id), mine) <= HOSTILE_MAX
                && (self.can_see(o.id, id) || self.can_see(id, o.id))
        })
    }

    // ---------------------------------------------------------------
    // Input
    // ---------------------------------------------------------------

    /// The player orders the party leader to attack `target`. Replaces the
    /// leader's queue; party members join through their own AI scripts.
    pub fn player_attack(&mut self, target: ObjectId) {
        let leader = self.party_leader();
        if let Some(o) = self.get_mut(leader) {
            o.actions.clear();
            o.action_epoch += 1;
        }
        self.bash_reset(leader);
        self.combat.mem(leader).attempted_target = target;
        self.push_action(leader, Action::Attack(target));
    }

    // ---------------------------------------------------------------
    // Rounds
    // ---------------------------------------------------------------

    fn attack_reach(&self, id: ObjectId, target: ObjectId, weapon: &WeaponInfo) -> f32 {
        if weapon.ranged {
            return weapon.range;
        }
        let r = |x: ObjectId| self.get(x).and_then(|o| o.creature()).map_or(0.5, |c| c.radius.clamp(0.2, 1.5));
        1.0 + r(id) + r(target)
    }

    /// One frame of an Attack action. Returns (done, replacement).
    pub(crate) fn step_attack(&mut self, id: ObjectId, target: ObjectId, dt: f32) -> (bool, Option<Action>) {
        if self.is_object_target(target) {
            return self.step_bash(id, target, dt);
        }
        let target_ok = self.get(target).is_some_and(|t| t.kind == ObjectKind::Creature) && !self.is_dead_creature(target);
        if !target_ok || self.is_dead_creature(id) {
            self.finish_round(id, true);
            return (true, None);
        }
        let Some(stats) = self.combat_stats(id) else { return (true, None) };
        let (Some(me), Some(tp)) = (self.get(id).map(|o| o.position), self.get(target).map(|o| o.position)) else {
            return (true, None);
        };
        let reach = self.attack_reach(id, target, &stats.main);
        let dist = (tp - me).truncate().length();
        self.combat.mem(id).attempted_target = target;
        if !self.combat.rounds.contains_key(&id) && dist > reach {
            // Close in along a walkmesh route (nav.rs, via move_towards). When
            // no route or no progress, swing anyway if nearly in reach
            // (KotOR's melee reach is generous), give up after a while and
            // fire OnBlocked so the AI can pick something else.
            // Melee attackers spread over ring slots around the target
            // (crowd.rs) instead of stacking on the route's last leg.
            let slot = if stats.main.ranged { None } else { self.melee_slot(id, target, reach * 0.8) };
            let (moved, arrived) = match slot {
                Some(p) => self.move_towards(id, p, true, 0.25, dt),
                None => self.move_towards(id, tp, true, reach * 0.85, dt),
            };
            if !arrived {
                let stuck = self.combat.approach_stuck.entry(id).or_insert(0.0);
                if moved < 0.05 * dt.max(1e-3) * 30.0 {
                    *stuck += dt;
                } else {
                    *stuck = 0.0;
                }
                let stuck = *stuck;
                let close = dist <= reach + 1.5 && (tp.z - me.z).abs() < 2.0;
                if stuck >= 1.0 && close {
                    self.combat.approach_stuck.remove(&id);
                } else if stuck >= 4.0 {
                    self.combat.approach_stuck.remove(&id);
                    self.queue_event(id, "ScriptOnBlocked", ScriptCtx::default());
                    self.pump();
                    return (true, None);
                } else {
                    return (false, None);
                }
            } else {
                self.combat.approach_stuck.remove(&id);
            }
        }
        if let Some(bearing) = bearing_towards(me, tp)
            && let Some(o) = self.get_mut(id)
        {
            o.bearing = bearing;
        }
        // Drawing a weapon (equip clip) holds the round.
        if self.weapon_busy(id) > 0.0 {
            return (false, None);
        }
        if !self.combat.rounds.contains_key(&id) {
            self.start_round(id, target, &stats);
        }
        let now = self.time;
        let (due, round_over) = {
            let r = self.combat.rounds.get_mut(&id).expect("round started");
            r.elapsed += dt;
            let mut due = Vec::new();
            while r.next < r.schedule.len() && r.elapsed >= r.schedule[r.next].0 {
                due.push((r.schedule[r.next].1, r.next));
                r.next += 1;
            }
            (due, r.elapsed >= ROUND)
        };
        for (offhand, n) in due {
            if self.is_dead_creature(target) || self.is_dead_creature(id) {
                break;
            }
            self.resolve_attack(id, target, &stats, offhand, n);
        }
        self.combat.mem(id).combat_until = now + COMBAT_LINGER;
        let target_dead = self.is_dead_creature(target) || !self.is_valid(target);
        if round_over || target_dead {
            self.finish_round(id, true);
            return (target_dead, None);
        }
        (false, None)
    }

    fn start_round(&mut self, id: ObjectId, target: ObjectId, stats: &CombatStats) {
        let n = stats.attacks_per_round();
        let times: &[f32] = match n {
            1 => &[0.6],
            2 => &[0.6, 1.8],
            _ => &[0.5, 1.4, 2.3],
        };
        let schedule = times.iter().enumerate().map(|(i, &t)| (t, stats.offhand.is_some() && i == 1)).collect();
        self.combat.rounds.insert(id, Round { target, elapsed: 0.0, schedule, next: 0 });
        let m = self.combat.mem(id);
        m.attack_target = target;
        m.last_hostile_target = target;
        if self.combat.log_enabled {
            let line = format!("[{:7.2}] {} starts a round on {}", self.time, self.combat_label(id), self.combat_label(target));
            self.combat.log.push(line);
        }
    }

    /// Ends `id`'s round (if any) and fires its OnEndCombatRound.
    fn finish_round(&mut self, id: ObjectId, fire: bool) {
        if self.combat.rounds.remove(&id).is_none() {
            return;
        }
        if fire && self.is_valid(id) && !self.is_dead_creature(id) {
            self.queue_event(id, "ScriptEndRound", ScriptCtx::default());
            self.pump();
        }
    }

    pub(crate) fn attack_animation(&self, attacker: ObjectId, w: &WeaponInfo, n: usize) -> (String, Vec<String>) {
        let k = n % 2 + 1;
        let is_droid = self.get(attacker).and_then(|o| o.creature()).is_some_and(|c| c.racial_type == 5);
        // Off hand = slot 5 only: the creature weapon slots (14-16) are natural
        // weapons, never a second blaster (see docs/notes/combat.md).
        let dual = self.held_wield(attacker, SLOT_LEFTWEAPON) == Some(4);
        let mut list = attack_clip_list(w.wield, w.ranged, dual, k, is_droid);
        let first = list.remove(0);
        (first, list)
    }

    fn resolve_attack(&mut self, id: ObjectId, target: ObjectId, stats: &CombatStats, offhand: bool, n: usize) {
        let Some(tstats) = self.combat_stats(target) else { return };
        let weapon = if offhand { stats.offhand.clone().unwrap_or_else(|| stats.main.clone()) } else { stats.main.clone() };
        let (anim, fallbacks) = self.attack_animation(id, &weapon, n);
        self.emit(WorldEvent::AttackStarted { attacker: id, target, anim, fallbacks, ranged: weapon.ranged });

        let ab = stats.attack_bonus(offhand);
        let def = tstats.defense;
        let roll = self.combat.roll(20);
        let mut hit = roll == 20 || (roll != 1 && roll + ab >= def);
        // A blaster bolt is turned aside when the target's deflection roll
        // beats the attack roll (feat text: Jedi Defense, Deflect). Winning by
        // 10 sends the bolt back at the attacker; that part is not modelled.
        let mut deflected = false;
        if hit && weapon.ranged && let Some(bonus) = tstats.deflection {
            deflected = self.combat.roll(20) + bonus > roll + ab;
            hit = !deflected;
        }
        let threat = hit && roll >= 21 - weapon.crit_threat.max(1);
        let crit = threat && {
            let c = self.combat.roll(20);
            c == 20 || (c != 1 && c + ab >= def)
        };
        let result = if !hit {
            AttackResult::Miss
        } else if crit {
            AttackResult::Critical
        } else {
            AttackResult::Hit
        };

        {
            let now = self.time;
            let m = self.combat.mem(id);
            m.last_attack_result = result as i32;
            m.last_hostile_target = target;
            m.combat_until = now + COMBAT_LINGER;
            let t = self.combat.mem(target);
            t.last_attacker = id;
            t.last_hostile_actor = id;
            t.combat_until = now + COMBAT_LINGER;
        }

        let mut damage = 0;
        if hit {
            damage = self.combat.dice(weapon.num_dice, weapon.die);
            for &(nd, d) in &weapon.extra_dice {
                damage += self.combat.dice(nd, d);
            }
            if !weapon.ranged {
                damage += stats.str_mod();
            }
            damage += weapon.damage_bonus;
            if crit {
                damage *= weapon.crit_mult.max(2);
            }
            damage = damage.max(1);
        }
        if self.combat.log_enabled {
            let line = format!(
                "[{:7.2}] {} -> {}: d20 {:2} + {} = {:2} vs {} {}{}",
                self.time,
                self.combat_label(id),
                self.combat_label(target),
                roll,
                ab,
                roll + ab,
                def,
                match result {
                    AttackResult::Miss => "miss".to_string(),
                    AttackResult::Hit => format!("hit {damage}"),
                    AttackResult::Critical => format!("CRIT {damage}"),
                },
                if offhand { " (off-hand)" } else { "" }
            );
            self.combat.log.push(line);
        }
        // OnPhysicalAttacked fires for every attack, hit or miss.
        self.queue_event(target, "ScriptAttacked", ScriptCtx::default());
        if weapon.ranged {
            let end = if hit {
                ProjectileEnd::Impact
            } else if deflected {
                ProjectileEnd::Deflect
            } else {
                ProjectileEnd::Miss
            };
            self.emit_bolt(id, target, end, &weapon, offhand);
        }
        if hit {
            self.emit(WorldEvent::AttackHit { attacker: id, target, damage, critical: crit });
            self.apply_damage(target, damage, id);
        } else {
            self.emit(WorldEvent::AttackMissed { attacker: id, target });
            if deflected {
                self.emit(WorldEvent::AttackDeflected { attacker: id, target });
            }
        }
        self.pump();
    }

    /// Damage from outside the combat rules (test hooks, scripted harm).
    pub fn hurt_creature(&mut self, target: ObjectId, amount: i32, by: ObjectId) {
        self.apply_damage(target, amount, by);
        self.pump();
    }

    /// EffectDeath from outside the scripts (test hook): kills outright, a
    /// party member included, knocked out or not. The exception is min1hp and
    /// plot creatures: a standing one stays at 1 HP, a knocked-out one stays
    /// down at 0 HP (not dead).
    pub fn slay_creature(&mut self, target: ObjectId, by: ObjectId) {
        self.harm(target, i32::MAX / 2, by, true);
        self.pump();
    }

    /// Damage from any source (attacks, EffectDamage). A party member taken to
    /// 0 HP is knocked down; anyone else dies. min1hp and plot creatures are
    /// held at 1 HP instead.
    pub(crate) fn apply_damage(&mut self, target: ObjectId, amount: i32, by: ObjectId) {
        self.harm(target, amount, by, false);
    }

    /// Damage from `by` (see [`World::apply_damage`]). `lethal` is EffectDeath:
    /// a party member killed this way does not get up, knocked out or not.
    pub(crate) fn harm(&mut self, target: ObjectId, amount: i32, by: ObjectId, lethal: bool) {
        // The dead take no more damage; a knocked-out creature only takes
        // EffectDeath. Checked before absorb_damage, so a hit that does nothing
        // does not spend temporary hit points.
        if self.get(target).and_then(|o| o.creature()).is_some_and(|c| c.dead || (c.down && !lethal)) {
            return;
        }
        // Temporary hit points and damage resistance first (powers.rs).
        let amount = self.absorb_damage(target, amount);
        let in_party = self.party.contains(&target);
        let Some(o) = self.get_mut(target) else { return };
        let plot = o.plot;
        let Some(c) = o.creature_mut() else { return };
        let before = c.hp;
        c.hp = c.hp.saturating_sub(amount);
        let mut died = false;
        let mut fell = false;
        if c.hp <= 0 {
            if c.min1hp || plot {
                // Held at 1 HP while standing. A knocked-out one stays down
                // at 0 HP: EffectDeath does not stand it up (combat.md).
                c.hp = if c.down { 0 } else { 1 };
            } else if in_party && !lethal {
                c.hp = 0;
                fell = true;
            } else {
                // kill() clears `down` (and reads it first).
                c.hp = 0;
                c.dead = true;
                c.slain = lethal;
                died = true;
            }
        }
        let dealt = before - c.hp;
        {
            let now = self.time;
            let m = self.combat.mem(target);
            m.last_damage = dealt;
            if by != OBJECT_INVALID {
                m.last_damager = by;
                m.last_hostile_actor = by;
                m.combat_until = now + COMBAT_LINGER;
            }
        }
        self.emit(WorldEvent::Damaged { object: target, amount: dealt, by });
        if self.combat.log_enabled {
            let hp = self.get(target).and_then(|o| o.creature()).map_or(0, |c| c.hp);
            let line = format!("[{:7.2}] {} takes {} ({} hp left)", self.time, self.combat_label(target), dealt, hp);
            self.combat.log.push(line);
        }
        self.queue_event(target, "ScriptDamaged", ScriptCtx::default());
        if died {
            self.kill(target, by);
        }
        if fell {
            self.knock_down(target, by);
        }
    }

    fn kill(&mut self, target: ObjectId, by: ObjectId) {
        // A knocked-out party member already queued ScriptDeath and was handed
        // over (or the party defeated) in knock_down; that is not repeated.
        let was_down = self.get(target).and_then(|o| o.creature()).is_some_and(|c| c.down);
        if let Some(c) = self.get_mut(target).and_then(|o| o.creature_mut()) {
            c.down = false;
            c.get_up_in = 0.0;
        }
        if let Some(o) = self.get_mut(target) {
            o.actions.clear();
            o.action_epoch += 1;
        }
        self.combat.rounds.remove(&target);
        self.combat.mem(target).last_killer = by;
        self.emit(WorldEvent::Died { object: target, killer: by });
        self.award_kill_xp(target, by);
        self.emit(WorldEvent::PlayAnimation { object: target, animation: 26, speed: 1.0, duration: -1.0, looping: true });
        if self.combat.log_enabled {
            let line = format!("[{:7.2}] {} dies (killed by {})", self.time, self.combat_label(target), self.combat_label(by));
            self.combat.log.push(line);
        }
        if !was_down {
            self.queue_event(target, "ScriptDeath", ScriptCtx::default());
        }
        if self.party.contains(&target) && !was_down {
            self.party_member_down(target);
        }
        let template = self.get(target).map(|o| o.template.clone()).unwrap_or_default();
        let info = self.template_info(&template);
        let keep = info.lootable
            || info.disable_fade
            || self.combat.keep_corpse.contains(&target)
            || self.party.contains(&target)
            || self.get(target).is_some_and(|o| o.plot);
        if !keep {
            self.add_timer(CORPSE_SECONDS, TimerKind::Destroy(target));
        }
    }

    /// A party member reached 0 HP: knocked out, not dead (K2 hint: "Party
    /// members who go down in combat will be revived at the end of the
    /// battle"). It gets up in party.rs once the battle is over.
    fn knock_down(&mut self, target: ObjectId, by: ObjectId) {
        if let Some(o) = self.get_mut(target) {
            o.actions.clear();
            o.action_epoch += 1;
        }
        self.combat.rounds.remove(&target);
        let Some(c) = self.get_mut(target).and_then(|o| o.creature_mut()) else { return };
        c.down = true;
        c.hp = 0;
        c.get_up_in = GET_UP_DELAY;
        self.emit(WorldEvent::KnockedDown { object: target, by });
        if self.combat.log_enabled {
            let line = format!("[{:7.2}] {} is knocked down (by {})", self.time, self.combat_label(target), self.combat_label(by));
            self.combat.log.push(line);
        }
        // ScriptDeath is Atton's OnDeath hook (a_attonspirit: the Spirit feat
        // may get him up during the fight).
        self.queue_event(target, "ScriptDeath", ScriptCtx::default());
        self.party_member_down(target);
    }

    /// Per-tick combat housekeeping: hostile perception, dropping rounds of
    /// attackers whose action went away.
    pub(crate) fn run_combat(&mut self, dt: f32) {
        // A round only lives while its Attack action is at the front.
        let stale: Vec<ObjectId> = self
            .combat
            .rounds
            .iter()
            .filter(|(id, r)| {
                !self
                    .get(**id)
                    .and_then(|o| o.actions.front())
                    .is_some_and(|a| matches!(a, Action::Attack(t) if *t == r.target))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            self.combat.rounds.remove(&id);
        }
        self.run_party_revival(dt);
        self.combat.perceive_in -= dt;
        if self.combat.perceive_in <= 0.0 {
            self.combat.perceive_in = PERCEPTION_INTERVAL;
            self.perceive_hostiles();
        }
    }

    /// Creatures with an OnNotice script notice hostile non-party creatures
    /// (the world's own perception covers party members as targets).
    /// SpeakString from `speaker`: every listening creature within the volume's
    /// range whose listen pattern matches the text (ignoring case) fires its
    /// OnDialogue script with GetListenPatternNumber = the pattern's number and
    /// GetLastSpeaker = the speaker (k_def_dialogue01 / k_hen_*dlg then run
    /// GN_RespondToShout). Returns the listeners and their numbers.
    pub fn hear_speech(&mut self, speaker: ObjectId, text: &str, volume: i32) -> Vec<(ObjectId, i32)> {
        let Some(from) = self.get(speaker).map(|o| o.position) else { return Vec::new() };
        let range = talk_volume_range(volume);
        let heard: Vec<(ObjectId, i32)> = self
            .iter()
            .filter(|o| o.id != speaker && o.kind == ObjectKind::Creature && o.position.distance(from) <= range)
            .filter_map(|o| o.creature().filter(|c| c.listening && !c.dead && !c.down).map(|c| (o.id, c)))
            .filter_map(|(id, c)| c.listen_patterns.iter().find(|(p, _)| p.eq_ignore_ascii_case(text)).map(|(_, n)| (id, *n)))
            .collect();
        for &(id, number) in &heard {
            let ctx = ScriptCtx { last_speaker: speaker, listen_pattern: number, ..Default::default() };
            self.queue_event(id, "ScriptDialogue", ctx);
        }
        heard
    }

    /// True when `id` is attacking a party member right now: its running Attack
    /// action, else its combat round.
    fn attacking_party(&self, id: ObjectId, party: &HashSet<ObjectId>) -> bool {
        let target = match self.get(id).and_then(|o| o.actions.front()) {
            Some(Action::Attack(t)) => *t,
            _ => self.combat.rounds.get(&id).map_or(OBJECT_INVALID, |r| r.target),
        };
        party.contains(&target)
    }

    fn perceive_hostiles(&mut self) {
        self.ensure_factions();
        let live: Vec<(ObjectId, Vec3, bool)> = self
            .iter()
            .filter(|o| o.kind == ObjectKind::Creature && o.creature().is_some_and(|c| !c.dead))
            .map(|o| (o.id, o.position, o.scripts.contains_key("ScriptOnNotice")))
            .collect();
        let party: HashSet<ObjectId> = self.party.iter().copied().collect();
        let mut notices = Vec::new();
        for &(observer, opos, has_script) in &live {
            if !has_script {
                continue;
            }
            let range = self.get(observer).and_then(|o| o.creature()).map_or(20.0, |c| c.perception_range);
            for &(other, pos, _) in &live {
                if other == observer || party.contains(&other) {
                    continue;
                }
                let hostile = {
                    let (fo, ft) = (self.faction_of(observer), self.faction_of(other));
                    // A party member also notices what is hostile to the party and
                    // whatever is attacking one of its members; the script then
                    // checks GetIsEnemy as usual.
                    self.combat.factions.reputation(fo, ft) <= HOSTILE_MAX
                        || (party.contains(&observer) && (self.combat.factions.reputation(ft, fo) <= HOSTILE_MAX || self.attacking_party(other, &party)))
                };
                let near = hostile && opos.distance(pos) <= range && self.can_see(observer, other);
                let seen = self.get(observer).and_then(|o| o.creature()).is_some_and(|c| c.seen.contains(&other));
                if near && seen && self.renotice_due(observer) {
                    notices.push((observer, other, true));
                    continue;
                }
                let Some(c) = self.get_mut(observer).and_then(|o| o.creature_mut()) else { continue };
                if near && !seen {
                    c.seen.insert(other);
                    notices.push((observer, other, true));
                } else if !near && seen {
                    c.seen.remove(&other);
                    notices.push((observer, other, false));
                }
            }
        }
        for (observer, other, seen) in notices {
            self.queue_event(
                observer,
                "ScriptOnNotice",
                ScriptCtx { last_perceived: other, perception_seen: seen, ..Default::default() },
            );
        }
        self.pump();
    }

    // ---------------------------------------------------------------
    // Engine routines
    // ---------------------------------------------------------------

    fn self_or(a: &[Value], n: usize, me: ObjectId) -> ObjectId {
        let v = obj(a, n);
        if v == OBJECT_INVALID && a.len() <= n { me } else { v }
    }

    /// Whether `observer` sees `other`: within its perception range, not
    /// hidden by stealth and with a clear line of sight. Perception events
    /// (OnNotice), GetObjectSeen and the PERCEPTION_SEEN searches all use
    /// this one test; a range-only notice made hostiles "see" the party
    /// through walls and then refuse to act because GetObjectSeen said no.
    pub fn can_see(&self, observer: ObjectId, other: ObjectId) -> bool {
        let (Some(a), Some(b)) = (self.get(observer), self.get(other)) else { return false };
        let range = a.creature().map_or(20.0, |c| c.perception_range);
        a.position.distance(b.position) <= range
            && !self.hidden_from(observer, other)
            && self.walk.line_of_sight(a.position + Vec3::Z * 1.5, b.position + Vec3::Z * 1.5)
    }

    /// A hostile that still sees its enemy but is idle (no actions, not in
    /// combat) gets a fresh OnNotice every few seconds. The engine's
    /// perception only reports transitions, so a notice that did not lead to
    /// an attack (target out of sight when the delayed combat round ran, a
    /// save made mid-reaction) would otherwise leave the creature standing
    /// there for good.
    pub(crate) fn renotice_due(&mut self, observer: ObjectId) -> bool {
        const RENOTICE: f64 = 3.0;
        if self.conversation.is_some() || self.in_combat(observer) {
            return false;
        }
        // A companion's standing ActionFollowLeader is not work: the AI may still
        // notice enemies while it follows.
        let idle = self.get(observer).is_some_and(|o| o.commandable && o.actions.iter().all(|a| matches!(a, Action::Follow { .. })));
        let last = self.combat.renotice.get(&observer).copied().unwrap_or(f64::MIN);
        if !idle || self.time - last < RENOTICE {
            return false;
        }
        self.combat.renotice.insert(observer, self.time);
        true
    }

    fn perceives(&self, observer: ObjectId, other: ObjectId, perception: i32) -> bool {
        let seen = self.can_see(observer, other);
        match perception {
            0 | 3 | 5 | 7 => seen,
            2 => false,
            1 | 4 | 6 => !seen,
            _ => true,
        }
    }

    /// GetNearestCreature: the creatures matching `criteria` around `from`,
    /// nearest first. Criterion 3 (reputation) is how `from` feels about each.
    pub(crate) fn nearest_creatures(&mut self, from: ObjectId, criteria: &[(i32, i32)]) -> Vec<ObjectId> {
        self.ensure_factions();
        let Some(p) = self.get(from).map(|o| o.position) else { return Vec::new() };
        let mut v: Vec<(f32, ObjectId)> = self
            .iter()
            .filter(|o| o.id != from && o.kind == ObjectKind::Creature)
            .map(|o| (o.position.distance_squared(p), o.id))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let leader = self.party_leader();
        let explicit_alive = criteria.iter().any(|&(t, _)| t == 4);
        let mut out = Vec::new();
        for (_, id) in v {
            let dead = self.is_dead_creature(id);
            let mut ok = true;
            for &(ty, val) in criteria {
                ok &= match ty {
                    0 => self.get(id).and_then(|o| o.creature()).is_some_and(|c| c.racial_type == val),
                    1 => (id == leader || self.party.contains(&id)) == (val != 0),
                    2 => self.get(id).and_then(|o| o.creature()).is_some_and(|c| c.classes.iter().any(|&(cl, _)| cl == val)),
                    3 => {
                        // How `from` feels about the candidate.
                        let r = self.reputation(from, id);
                        let rep_ok = match val {
                            0 => r >= FRIENDLY_MIN,
                            1 => r <= HOSTILE_MAX,
                            2 => r > HOSTILE_MAX && r < FRIENDLY_MIN,
                            _ => true,
                        };
                        // Reputation searches skip corpses unless asked.
                        rep_ok && (explicit_alive || !dead)
                    }
                    4 => dead != (val != 0),
                    7 => self.perceives(from, id, val),
                    _ => true,
                };
                if !ok {
                    break;
                }
            }
            if ok {
                out.push(id);
            }
        }
        out
    }

    /// Log label: kind, tag and id (several creatures share a tag).
    pub fn combat_label(&self, id: ObjectId) -> String {
        let d = self.describe(id);
        if d.contains('#') { d } else { format!("{d}#{id}") }
    }

    fn mem_of(&self, id: ObjectId) -> Memory {
        self.combat.memory.get(&id).cloned().unwrap_or_default()
    }

    fn valid_or_invalid(&self, id: ObjectId) -> ObjectId {
        if self.is_valid(id) { id } else { OBJECT_INVALID }
    }

    /// Combat routines. Called before the general dispatch, so these
    /// override its explicit defaults.
    pub(crate) fn combat_call(&mut self, name: &str, me: ObjectId, a: &[Value]) -> Option<Value> {
        Some(match name {
            "GetIsEnemy" => {
                let src = Self::self_or(a, 1, me);
                b(self.is_enemy(src, obj(a, 0)))
            }
            "GetIsFriend" => {
                let src = Self::self_or(a, 1, me);
                b(self.is_friend(src, obj(a, 0)))
            }
            "GetIsNeutral" => {
                let (src, t) = (Self::self_or(a, 1, me), obj(a, 0));
                let valid = self.is_valid(src) && self.is_valid(t);
                let r = self.reputation(src, t);
                b(valid && r > HOSTILE_MAX && r < FRIENDLY_MIN)
            }
            "GetReputation" => {
                let (src, t) = (obj(a, 0), obj(a, 1));
                Value::Int(self.reputation(src, t))
            }
            "AdjustReputation" => {
                self.ensure_factions();
                let (t, src, delta) = (obj(a, 0), obj(a, 1), int(a, 2));
                let (fs, ft) = (self.faction_of(src), self.faction_of(t));
                self.combat.factions.adjust(fs, ft, delta);
                Value::Void
            }
            "GetNearestCreature" => {
                let from = Self::self_or(a, 2, me);
                let nth = int(a, 3).max(1) as usize;
                let mut criteria = vec![(int(a, 0), int(a, 1))];
                for (t, v) in [(4, 5), (6, 7)] {
                    let ty = if a.len() > t { int(a, t) } else { -1 };
                    if ty >= 0 {
                        criteria.push((ty, int(a, v)));
                    }
                }
                let list = self.nearest_creatures(from, &criteria);
                o(list.get(nth - 1).copied().unwrap_or(OBJECT_INVALID))
            }
            "ActionAttack" => {
                let t = obj(a, 0);
                self.combat.mem(me).attempted_target = t;
                self.push_action(me, Action::Attack(t));
                Value::Void
            }
            "GetLastAttacker" => {
                let id = Self::self_or(a, 0, me);
                o(self.valid_or_invalid(self.mem_of(id).last_attacker))
            }
            "GetLastDamager" => {
                let id = Self::self_or(a, 0, me);
                o(self.valid_or_invalid(self.mem_of(id).last_damager))
            }
            "GetLastHostileActor" => {
                let id = Self::self_or(a, 0, me);
                o(self.valid_or_invalid(self.mem_of(id).last_hostile_actor))
            }
            "GetLastHostileTarget" => {
                let id = Self::self_or(a, 0, me);
                o(self.valid_or_invalid(self.mem_of(id).last_hostile_target))
            }
            "GetLastKiller" => o(self.valid_or_invalid(self.mem_of(me).last_killer)),
            "GetAttackTarget" => {
                // What the creature is attacking: its running Attack action (from
                // the first frame, while it closes in), else its combat round.
                let id = Self::self_or(a, 0, me);
                let front = self.get(id).and_then(|x| match x.actions.front() {
                    Some(Action::Attack(t)) => Some(*t),
                    _ => None,
                });
                let t = match front {
                    Some(t) if self.is_valid(t) && self.get(t).and_then(|x| x.creature()).is_none_or(|c| !(c.dead || c.down)) => t,
                    Some(_) => OBJECT_INVALID,
                    None => {
                        let t = self.combat.rounds.get(&id).map(|r| r.target).unwrap_or_else(|| self.mem_of(id).attack_target);
                        if self.in_combat(id) && !self.is_dead_creature(t) { t } else { OBJECT_INVALID }
                    }
                };
                o(self.valid_or_invalid(t))
            }
            "GetAttemptedAttackTarget" => {
                let t = self.get(me).and_then(|x| match x.actions.front() {
                    Some(Action::Attack(t)) => Some(*t),
                    _ => None,
                });
                o(self.valid_or_invalid(t.unwrap_or(OBJECT_INVALID)))
            }
            "GetIsInCombat" => {
                let id = Self::self_or(a, 0, me);
                b(self.in_combat(id))
            }
            "GetLastAttackResult" => {
                let id = Self::self_or(a, 0, me);
                Value::Int(self.mem_of(id).last_attack_result)
            }
            "GetTotalDamageDealt" | "GetDamageDealtByType" => {
                // NWN semantics: on OnDamaged, the damage just taken by self.
                Value::Int(self.mem_of(me).last_damage)
            }
            "GetCombatActionsPending" => {
                let id = Self::self_or(a, 0, me);
                b(self.get(id).is_some_and(|x| x.actions.iter().any(|q| matches!(q, Action::Attack(_)))))
            }
            "CancelCombat" => {
                let id = Self::self_or(a, 0, me);
                self.combat.rounds.remove(&id);
                self.combat.mem(id).combat_until = 0.0;
                Value::Void
            }
            "SetIsDestroyable" => {
                let destroyable = int(a, 0) != 0;
                if destroyable {
                    self.combat.keep_corpse.remove(&me);
                } else {
                    self.combat.keep_corpse.insert(me);
                }
                Value::Void
            }
            "SetNPCAIStyle" => {
                self.combat.ai_style.insert(obj(a, 0), int(a, 1));
                Value::Void
            }
            "GetNPCAIStyle" => {
                let id = obj(a, 0);
                Value::Int(self.combat.ai_style.get(&id).copied().unwrap_or_else(|| self.default_npc_ai_style(id)))
            }
            "SetPartyAIStyle" => {
                self.combat.party_ai_style = int(a, 0);
                Value::Void
            }
            "GetPartyAIStyle" => Value::Int(self.combat.party_ai_style),
            "GetLastAttackAction" => {
                let id = Self::self_or(a, 0, me);
                let attacked = self.mem_of(id).last_attack_result != 0;
                Value::Int(if attacked { 3 } else { 65535 })
            }
            "GetGoingToBeAttackedBy" => {
                let t = obj(a, 0);
                let by = self
                    .iter()
                    .filter(|x| x.kind == ObjectKind::Creature)
                    .find(|x| matches!(x.actions.front(), Some(Action::Attack(q)) if *q == t))
                    .map(|x| x.id);
                o(by.unwrap_or(OBJECT_INVALID))
            }
            "GetWeaponRanged" => {
                let item = obj(a, 0);
                let ranged = self.item_info(item).and_then(|i| i.weapon(&self.res, &self.tables)).is_some_and(|w| w.ranged);
                b(ranged)
            }
            "GetIsWeaponEffective" => b(true),
            "GetAC" => {
                let id = obj(a, 0);
                Value::Int(self.combat_stats(id).map_or(10, |s| s.defense))
            }
            "GetFactionWeakestMember" | "GetFactionStrongestMember" | "GetFactionMostDamagedMember"
            | "GetFactionLeastDamagedMember" | "GetFactionWorstAC" | "GetFactionBestAC" => {
                let member = Self::self_or(a, 0, me);
                let visible = if a.len() > 1 { int(a, 1) != 0 } else { true };
                let mut list: Vec<(i32, ObjectId)> = Vec::new();
                for id in self.faction_members(member, false) {
                    if visible && member != id && !self.perceives(member, id, 7) {
                        continue;
                    }
                    let key = match name {
                        "GetFactionWeakestMember" | "GetFactionStrongestMember" => {
                            self.get(id).and_then(|x| x.creature()).map_or(0, |c| c.classes.iter().map(|&(_, l)| l).sum())
                        }
                        "GetFactionWorstAC" | "GetFactionBestAC" => self.combat_stats(id).map_or(10, |s| s.defense),
                        _ => self.get(id).and_then(|x| x.creature()).map_or(0, |c| c.max_hp - c.hp),
                    };
                    list.push((key, id));
                }
                let pick = match name {
                    "GetFactionWeakestMember" | "GetFactionLeastDamagedMember" | "GetFactionWorstAC" => {
                        list.iter().min_by_key(|(k, id)| (*k, *id))
                    }
                    _ => list.iter().max_by_key(|(k, id)| (*k, std::cmp::Reverse(*id))),
                };
                o(pick.map_or(OBJECT_INVALID, |p| p.1))
            }
            "GetFactionAverageLevel" => {
                let members = self.faction_members(obj(a, 0), false);
                let levels: Vec<i32> = members
                    .iter()
                    .filter_map(|&id| self.get(id).and_then(|x| x.creature()).map(|c| c.classes.iter().map(|&(_, l)| l).sum::<i32>().max(1)))
                    .collect();
                Value::Int(if levels.is_empty() { 0 } else { levels.iter().sum::<i32>() / levels.len() as i32 })
            }
            "GetFirstFactionMember" | "GetNextFactionMember" => {
                if name == "GetFirstFactionMember" {
                    let pc_only = if a.len() > 1 { int(a, 1) != 0 } else { true };
                    let list = self.faction_members(obj(a, 0), pc_only);
                    self.combat.faction_iter = (list, 0);
                }
                let (list, idx) = &mut self.combat.faction_iter;
                let v = list.get(*idx).copied().unwrap_or(OBJECT_INVALID);
                *idx += 1;
                o(v)
            }
            _ => return None,
        })
    }

    /// Live creatures sharing `member`'s faction (the party counts as one
    /// faction), in creation order. `pc_only` keeps party members only.
    fn faction_members(&self, member: ObjectId, pc_only: bool) -> Vec<ObjectId> {
        if !self.is_valid(member) {
            return Vec::new();
        }
        let in_party = self.party.contains(&member);
        let fac = self.faction_of(member);
        self.iter()
            .filter(|x| x.kind == ObjectKind::Creature && x.creature().is_some_and(|c| !c.dead))
            .filter(|x| if in_party || pc_only { self.party.contains(&x.id) } else { self.faction_of(x.id) == fac && !self.party.contains(&x.id) })
            .map(|x| x.id)
            .collect()
    }
}

/// Outcome of a [`fight`] harness run.
#[derive(Debug, Clone)]
pub struct FightReport {
    pub player: ObjectId,
    pub opponent: ObjectId,
    pub seconds: f32,
    pub winner: Option<ObjectId>,
    pub forced_hostile: bool,
    pub log: Vec<String>,
}

/// Headless fight: put the party leader next to the nearest hostile creature
/// (forcing the nearest creature hostile if the module has none), let the
/// AI scripts react and the player attack back, until one of them dies.
pub fn fight(world: &mut World, max_seconds: f32, dt: f32) -> Option<FightReport> {
    world.ensure_factions();
    world.combat.log_enabled = true;
    world.combat.seed(0x6b32_f19e);
    let player = world.party_leader();
    let ppos = world.get(player)?.position;
    let creatures: Vec<(ObjectId, f32)> = world
        .iter()
        .filter(|o| o.kind == ObjectKind::Creature && o.id != player && !world.party.contains(&o.id))
        .filter(|o| o.creature().is_some_and(|c| !c.dead))
        .map(|o| (o.id, o.position.distance(ppos)))
        .collect();
    let mut hostiles: Vec<(ObjectId, f32)> = Vec::new();
    for &(id, d) in &creatures {
        if world.is_enemy(id, player) {
            hostiles.push((id, d));
        }
    }
    hostiles.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut forced = false;
    let opponent = match hostiles.first() {
        Some(&(id, _)) => id,
        None => {
            let mut all = creatures.clone();
            all.sort_by(|a, b| a.1.total_cmp(&b.1));
            let id = all.first()?.0;
            if let Some(c) = world.get_mut(id).and_then(|o| o.creature_mut()) {
                c.faction = 1;
            }
            forced = true;
            id
        }
    };
    // Stand 4 m away from it, on the walkmesh near its position.
    let (opos, obearing) = world.get(opponent).map(|o| (o.position, o.bearing))?;
    let dir = Vec3::new(-obearing.sin(), obearing.cos(), 0.0);
    let spot = opos + dir * 4.0;
    world.place(player, spot);
    if let Some(p) = world.get_mut(player) {
        p.bearing = bearing_towards(spot, opos).unwrap_or(0.0);
    }
    let log_line = format!(
        "fight: {} ({} hp) vs {} ({} hp){}",
        world.combat_label(player),
        world.get(player).and_then(|o| o.creature()).map_or(0, |c| c.hp),
        world.combat_label(opponent),
        world.get(opponent).and_then(|o| o.creature()).map_or(0, |c| c.hp),
        if forced { " [forced hostile]" } else { "" }
    );
    world.combat.log.push(log_line);
    for id in [player, opponent] {
        if let Some(s) = world.combat_stats(id) {
            let line = format!(
                "  {}: level {} bab {} defense {} weapon base {} {}d{}{} ab {:+} attacks/round {}",
                world.combat_label(id),
                s.level,
                s.bab,
                s.defense,
                s.main.base_item as i64,
                s.main.num_dice,
                s.main.die,
                if s.main.ranged { " ranged" } else { "" },
                s.attack_bonus(false),
                s.attacks_per_round()
            );
            world.combat.log.push(line);
        }
    }
    let mut t = 0.0;
    let mut winner = None;
    world.player_attack(opponent);
    while t < max_seconds {
        // The player keeps attacking (a player would click again).
        let leader = world.party_leader();
        let attacking = world.get(leader).is_some_and(|o| o.actions.iter().any(|a| matches!(a, Action::Attack(_))));
        if !attacking && world.conversation.is_none() && !world.is_dead_creature(opponent) && !world.is_dead_creature(leader) {
            world.player_attack(opponent);
        }
        world.tick(dt);
        if world.conversation.is_some() {
            if let Some(v) = world.conversation_view() {
                if v.replies.is_empty() {
                    world.advance_conversation();
                } else {
                    world.choose_reply(0);
                }
            } else {
                world.end_conversation(true);
            }
        }
        let _ = world.drain_events();
        t += dt;
        if world.is_dead_creature(opponent) {
            winner = Some(player);
            break;
        }
        if world.is_dead_creature(player) {
            winner = Some(opponent);
            break;
        }
    }
    if winner.is_none() {
        let leader = world.party_leader();
        let d = |id: ObjectId| {
            world.get(id).map(|o| {
                format!(
                    "{} at ({:.1},{:.1},{:.1}) commandable={} actions={:?} hp={:?}",
                    world.combat_label(id),
                    o.position.x,
                    o.position.y,
                    o.position.z,
                    o.commandable,
                    o.actions.iter().map(Action::constant_name).collect::<Vec<_>>(),
                    o.creature().map(|c| (c.hp, c.min1hp, o.plot))
                )
            })
        };
        let lines = [d(leader), d(opponent)];
        for l in lines.into_iter().flatten() {
            world.combat.log.push(format!("  timeout state: {l}"));
        }
    }
    Some(FightReport {
        player,
        opponent,
        seconds: t,
        winner,
        forced_hostile: forced,
        log: std::mem::take(&mut world.combat.log),
    })
}

impl World {
    /// AI style when no script set one. Party members (and party NPC slots)
    /// use K2's party styles: Jedi NPC_AISTYLE_PARTY_SUPPORT (13, buffs,
    /// heals and Force powers via GN_RunJediSupportAIRoutine), everyone else
    /// NPC_AISTYLE_PARTY_AGGRO (9). Others keep 0, the default routine.
    /// The original's default for companions is not documented; this is
    /// chosen so Jedi companions use their powers, as they do in the game.
    pub fn default_npc_ai_style(&self, id: ObjectId) -> i32 {
        let in_party = self.party.contains(&id) || self.available_npcs.values().any(|n| n.object == Some(id));
        if !in_party || id == self.player {
            return 0;
        }
        let jedi = self.get(id).and_then(|o| o.creature()).is_some_and(|c| c.classes.iter().any(|&(cl, l)| l > 0 && (3..=5).contains(&cl) || (11..=16).contains(&cl)));
        if jedi { 13 } else { 9 }
    }
}
