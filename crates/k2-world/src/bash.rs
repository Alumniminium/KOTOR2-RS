//! Attacking doors and placeables ("Bash" in KotOR).
//!
//! An attack on a bashable object always connects (objects have no
//! Defense). Each hit deals the weapon roll plus the strength modifier minus
//! the object's Hardness; weapons with the DoorCutting property (the plasma
//! torch) and lightsabers ignore Hardness; the torch also burns through a
//! bashable door or placeable in a single swing (first attack, whatever its
//! HP). Every hit fires OnDamaged on the object. At 0 HP the object is
//! destroyed: a door opens for good (walkmesh to Open1) and OnDeath fires; a
//! placeable unlocks and OnDeath fires. A trapped one sets its trap off
//! first, locked or not (inferred, see traps.rs). It does not open by
//! itself and its walkmesh stays (not checked against the original).
//!
//! Bashable: not Plot, not Static, and for doors not NotBlastable. The
//! shipped Peragus doors (sw_door_per00x: 60 HP, Hardness 20) can only be
//! cut with the torch or a lightsaber; the 001ebo prologue doors are all
//! Plot + NotBlastable and open by Security, mines or scripts instead.

use glam::{Vec2, Vec3};
use k2_script::ObjectId;

use crate::combat::ROUND;
use crate::object::{Action, Detail, Hp, ObjectKind, bearing_towards};
use crate::{ScriptCtx, World, WorldEvent};

/// Seconds without progress on the way to a door before giving up.
const BASH_STUCK_ABORT: f32 = 2.0;

pub(crate) fn door_hp(bp: &k2_data::Door) -> Hp {
    let max = bp.hp.max(bp.current_hp);
    Hp {
        current: bp.current_hp.max(1),
        max: max.max(1),
        hardness: bp.hardness.max(0),
        bashable: !bp.plot && !bp.is_static && !bp.not_blastable,
        destroyed: false,
    }
}

pub(crate) fn placeable_hp(bp: &k2_data::Placeable) -> Hp {
    let max = bp.hp.max(bp.current_hp);
    Hp {
        current: bp.current_hp.max(1),
        max: max.max(1),
        hardness: bp.hardness.max(0),
        bashable: !bp.plot && !bp.is_static,
        destroyed: false,
    }
}

impl World {
    /// The object's hit points, when it is a door or placeable.
    pub fn object_hp(&self, id: ObjectId) -> Option<Hp> {
        match &self.get(id)?.detail {
            Detail::Door(d) => Some(d.hp),
            Detail::Placeable(p) => Some(p.hp),
            _ => None,
        }
    }

    fn object_hp_mut(&mut self, id: ObjectId) -> Option<&mut Hp> {
        match &mut self.get_mut(id)?.detail {
            Detail::Door(d) => Some(&mut d.hp),
            Detail::Placeable(p) => Some(&mut p.hp),
            _ => None,
        }
    }

    /// A door or placeable as an attack target (bashable or not).
    pub(crate) fn is_object_target(&self, id: ObjectId) -> bool {
        self.get(id).is_some_and(|o| matches!(o.kind, ObjectKind::Door | ObjectKind::Placeable))
    }

    /// Whether attacking `id` would do anything: a bashable door that is
    /// still closed, or a bashable placeable that is still locked or shut.
    pub fn can_bash(&self, id: ObjectId) -> bool {
        let Some(o) = self.get(id) else { return false };
        match &o.detail {
            Detail::Door(d) => d.hp.bashable && !d.hp.destroyed && !d.open && d.hp.current > 0,
            Detail::Placeable(p) => p.hp.bashable && !p.hp.destroyed && p.hp.current > 0 && (p.locked || !p.open),
            _ => false,
        }
    }

    /// One frame of an Attack action on a door or placeable.
    pub(crate) fn step_bash(&mut self, id: ObjectId, target: ObjectId, dt: f32) -> (bool, Option<Action>) {
        if !self.can_bash(target) {
            self.bash_reset(id);
            return (true, None);
        }
        let Some(stats) = self.combat_stats(id) else { return (true, None) };
        let (Some(me), Some(tp)) = (self.get(id).map(|o| o.position), self.get(target).map(|o| o.position)) else {
            return (true, None);
        };
        let radius = self.get(id).and_then(|o| o.creature()).map_or(0.5, |c| c.radius.clamp(0.2, 1.5));
        // Door/placeable origins sit inside their blockers: swing from about
        // a metre out (ranged weapons from their range).
        let reach = if stats.main.ranged { stats.main.range.min(8.0) } else { 1.3 + radius };
        let dist = (tp - me).truncate().length();
        if !self.combat.bash_clock.contains_key(&id) {
            // Walk (along a walkmesh path) to a standing point on the side of
            // the object we can reach; swing once in range on that side.
            let stand = self.bash_stand(id, target, me, radius);
            let in_reach = dist <= reach && stand.is_none_or(|s| self.bash_same_side(target, me, s));
            let at_stand = stand.is_some_and(|s| (s - me).truncate().length() <= 0.5);
            if stand.is_none() && !in_reach {
                return self.bash_abort(id);
            }
            if !at_stand && !in_reach {
                let (moved, arrived) = self.move_towards(id, stand.unwrap_or(tp), true, 0.3, dt);
                if arrived {
                    self.combat.bash_stuck.remove(&id);
                } else {
                    let stuck = self.combat.bash_stuck.entry(id).or_insert(0.0);
                    if moved < dt.max(1e-3) {
                        *stuck += dt;
                    } else {
                        *stuck = 0.0;
                    }
                    if *stuck >= BASH_STUCK_ABORT {
                        return self.bash_abort(id);
                    }
                    return (false, None);
                }
            }
            self.combat.bash_stuck.remove(&id);
        }
        if let Some(b) = bearing_towards(me, tp)
            && let Some(o) = self.get_mut(id)
        {
            o.bearing = b;
        }
        // Swings at 0.6 s into each slot; slots split the 3 s round.
        let per_round = stats.attacks_per_round().max(1) as f32;
        let slot = ROUND / per_round;
        let clock = self.combat.bash_clock.entry(id).or_insert(slot - 0.6);
        let before = *clock;
        *clock += dt;
        let swings = ((*clock / slot).floor() - (before / slot).floor()).max(0.0) as usize;
        let n = (*clock / slot).floor() as usize;
        for k in 0..swings {
            if !self.can_bash(target) {
                break;
            }
            self.bash_swing(id, target, &stats, n + k);
        }
        if !self.can_bash(target) {
            self.bash_reset(id);
            return (true, None);
        }
        (false, None)
    }

    pub(crate) fn bash_reset(&mut self, id: ObjectId) {
        self.combat.bash_clock.remove(&id);
        self.combat.bash_stuck.remove(&id);
        self.combat.bash_stand.remove(&id);
    }

    /// Whether `id` is on its way to (or busy with) an attack on a door or
    /// placeable, i.e. the front action is an Attack on one.
    pub fn is_bashing(&self, id: ObjectId) -> bool {
        self.get(id).is_some_and(|o| matches!(o.actions.front(), Some(Action::Attack(t)) if self.is_object_target(*t)))
    }

    /// Gives up on an approach that cannot finish: the attack action and the
    /// rest of the queue are dropped so the player (or the AI) is free again.
    fn bash_abort(&mut self, id: ObjectId) -> (bool, Option<Action>) {
        self.bash_reset(id);
        self.paths.forget(id);
        if let Some(o) = self.get_mut(id) {
            o.actions.clear();
            o.action_epoch += 1;
        }
        if self.party.contains(&id) {
            self.emit(WorldEvent::Bark { object: id, text: "I can't reach that.".into() });
        }
        (true, None)
    }

    /// Whether `from` is on the same side of a door as the standing point.
    fn bash_same_side(&self, target: ObjectId, from: Vec3, stand: Vec3) -> bool {
        let Some(o) = self.get(target) else { return true };
        if o.kind != ObjectKind::Door {
            return true;
        }
        let n = Vec2::new(-o.bearing.sin(), o.bearing.cos());
        let p = o.position.truncate();
        n.dot(from.truncate() - p) * n.dot(stand.truncate() - p) > 0.0
    }

    /// A walkable standing point about a metre from the object that `id` can
    /// reach by path (the nearest such by path length), cached per attack.
    fn bash_stand(&mut self, id: ObjectId, target: ObjectId, me: Vec3, radius: f32) -> Option<Vec3> {
        if let Some(&(t, p)) = self.combat.bash_stand.get(&id)
            && t == target
        {
            return p;
        }
        let (tp, bearing, kind) = self.get(target).map(|o| (o.position, o.bearing, o.kind))?;
        let mut cands: Vec<Vec3> = Vec::new();
        if kind == ObjectKind::Door {
            let n = Vec2::new(-bearing.sin(), bearing.cos());
            for s in [1.2f32, -1.2] {
                cands.push((tp.truncate() + n * s).extend(tp.z));
            }
        } else {
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                cands.push((tp.truncate() + Vec2::new(a.cos(), a.sin()) * 1.4).extend(tp.z));
            }
        }
        let r = radius.min(0.5);
        let mut best: Option<(f32, Vec3)> = None;
        for c in cands {
            let Some(g) = self.walk.ground(c.x, c.y, tp.z + 1.0) else { continue };
            let c = Vec3::new(c.x, c.y, g.z);
            let Some(path) = self.walk.find_path(me, c, r) else { continue };
            let end = path.last().copied().unwrap_or(me);
            if end.truncate().distance(c.truncate()) > 0.5 {
                continue;
            }
            let len: f32 = std::iter::once(me).chain(path).collect::<Vec<_>>().windows(2).map(|w| (w[1] - w[0]).truncate().length()).sum();
            if best.is_none_or(|(l, _)| len < l) {
                best = Some((len, c));
            }
        }
        let stand = best.map(|(_, c)| c);
        self.combat.bash_stand.insert(id, (target, stand));
        stand
    }

    fn bash_swing(&mut self, id: ObjectId, target: ObjectId, stats: &crate::stats::CombatStats, n: usize) {
        let weapon = stats.main.clone();
        let (anim, fallbacks) = self.attack_animation(id, &weapon, n);
        self.emit(WorldEvent::AttackStarted { attacker: id, target, anim, fallbacks, ranged: weapon.ranged });
        let mut damage = self.combat.dice(weapon.num_dice, weapon.die);
        for &(nd, d) in &weapon.extra_dice {
            damage += self.combat.dice(nd, d);
        }
        if !weapon.ranged {
            damage += stats.str_mod();
        }
        damage += weapon.damage_bonus;
        let hp = self.object_hp(target).unwrap_or_default();
        let cuts = weapon.door_cutting || weapon.lightsaber;
        // The plasma torch burns through in one swing, whatever the HP.
        let burns_through = weapon.door_cutting;
        let dealt = if burns_through {
            hp.current.max(1)
        } else if cuts {
            damage.max(1)
        } else {
            (damage - hp.hardness).max(0)
        };
        if self.combat.log_enabled {
            let line = format!(
                "[{:7.2}] {} bashes {}: {} {}{} -> {} ({} hp left)",
                self.time,
                self.combat_label(id),
                self.describe(target),
                damage,
                if burns_through {
                    "cutting through in one swing"
                } else if cuts {
                    "cutting, hardness ignored"
                } else {
                    "minus hardness "
                },
                if cuts { String::new() } else { hp.hardness.to_string() },
                dealt,
                (hp.current - dealt).max(0)
            );
            self.combat.log.push(line);
        }
        {
            let now = self.time;
            let m = self.combat.memory.entry(target).or_default();
            m.last_attacker = id;
            m.last_hostile_actor = id;
            let a = self.combat.memory.entry(id).or_default();
            a.attack_target = target;
            a.attempted_target = target;
            a.combat_until = now + 6.0;
        }
        self.queue_event(target, "OnMeleeAttacked", ScriptCtx::default());
        self.emit(WorldEvent::AttackHit { attacker: id, target, damage: dealt, critical: false });
        if dealt > 0 {
            self.damage_object(target, dealt, id);
        }
        self.pump();
    }

    /// Damage to a door or placeable from any source.
    pub(crate) fn damage_object(&mut self, target: ObjectId, amount: i32, by: ObjectId) {
        let Some(hp) = self.object_hp_mut(target) else { return };
        if hp.destroyed {
            return;
        }
        let before = hp.current;
        hp.current = (hp.current - amount).max(0);
        let dealt = before - hp.current;
        let dead = hp.current == 0;
        {
            let m = self.combat.memory.entry(target).or_default();
            m.last_damage = dealt;
            m.last_damager = by;
        }
        self.emit(WorldEvent::Damaged { object: target, amount: dealt, by });
        self.queue_event(target, "OnDamaged", ScriptCtx::default());
        if dead {
            self.destroy_object_by_damage(target, by);
        }
    }

    fn destroy_object_by_damage(&mut self, target: ObjectId, by: ObjectId) {
        if let Some(hp) = self.object_hp_mut(target) {
            hp.destroyed = true;
        }
        // Bashing a trapped door or placeable open sets its trap off (inferred, traps.rs).
        self.trigger_object_trap(target, by);
        let kind = self.get(target).map(|o| o.kind);
        match kind {
            Some(ObjectKind::Door) => {
                if let Some(d) = self.get_mut(target).and_then(|o| o.door_mut()) {
                    d.locked = false;
                }
                self.sync_door_passable();
                self.set_door_open(target, true, by);
            }
            Some(ObjectKind::Placeable) => {
                if let Some(p) = self.get_mut(target).and_then(|o| o.placeable_mut()) {
                    p.locked = false;
                }
            }
            _ => {}
        }
        if self.combat.log_enabled {
            let line = format!("[{:7.2}] {} is destroyed", self.time, self.describe(target));
            self.combat.log.push(line);
        }
        self.combat.memory.entry(target).or_default().last_killer = by;
        self.queue_event(target, "OnDeath", ScriptCtx::default());
        self.pump();
    }
}
