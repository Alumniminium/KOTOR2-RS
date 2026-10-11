//! Creature-creature collision: personal space, separation, local avoidance,
//! melee ring slots, and the player's own movement against NPCs.
//!
//! Radii come from appearance.2da `perspace` (personal space, used for
//! creature-creature collision); `hitradius` stays the melee reach radius
//! (combat.rs). Separation only touches pairs where at least one creature was
//! moved by the simulation this tick, so authored placements (crowds, NPCs
//! seated close together, cutscene marks) are never disturbed. Pushes go
//! through `move_slide`, so nobody leaves the walkmesh.

use std::collections::HashMap;

use glam::{Vec2, Vec3};
use k2_script::ObjectId;

use crate::World;
use crate::object::ObjectKind;
use serde::{Deserialize, Serialize};

/// Fallback personal space when appearance.2da has no value.
const DEFAULT_SPACE: f32 = 0.35;
/// Personal space is clamped so outliers (rancor, 3 m) don't wedge corridors.
const MAX_SPACE: f32 = 1.5;
/// Vertical separation beyond which two creatures don't collide (stairs,
/// ledges, different floors).
const MAX_DZ: f32 = 1.5;
/// Relaxation passes per tick.
const PASSES: usize = 3;
/// How far ahead (beyond the sum of radii) a mover looks for others.
const LOOKAHEAD: f32 = 0.8;
/// Blocked this long by a creature: side-step harder around it.
const BLOCKED_SIDESTEP: f32 = 1.0;
/// A creature stays "active" this long after it last moved or fought.
const ACTIVE_FOR: f64 = 3.0;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Crowd {
    /// appearance row -> personal space radius.
    space: HashMap<u32, f32>,
    /// Seconds each mover has been held up by another creature.
    blocked_for: HashMap<ObjectId, f32>,
    /// Position after the last separation pass, to spot moves made by
    /// anything else (actions, jumps, the player).
    last_pos: HashMap<ObjectId, Vec3>,
    /// World time until which a creature counts as active (moved, jumped or
    /// fought recently). Pairs of inactive creatures are never separated.
    active_until: HashMap<ObjectId, f64>,
    /// Smallest pairwise gap (distance - radii) seen in the last tick, for
    /// the harness. Positive = no overlap.
    pub min_gap: f32,
}

impl Crowd {
    pub fn clear(&mut self) {
        self.blocked_for.clear();
        self.last_pos.clear();
        self.active_until.clear();
    }
}

#[derive(Clone, Copy)]
struct Body {
    id: ObjectId,
    pos: Vec3,
    r: f32,
    moving: bool,
    active: bool,
    pinned: bool,
}

impl World {
    /// Personal space radius of a creature (appearance.2da perspace).
    pub fn personal_space(&mut self, id: ObjectId) -> f32 {
        let Some(app) = self.get(id).and_then(|o| o.creature()).map(|c| c.appearance) else { return DEFAULT_SPACE };
        if let Some(&r) = self.crowd.space.get(&app) {
            return r;
        }
        let r = self
            .tables
            .get(&self.res, "appearance")
            .ok()
            .and_then(|t| t.float(app as usize, "perspace"))
            .filter(|r| *r > 0.0)
            .unwrap_or(DEFAULT_SPACE)
            .min(MAX_SPACE);
        self.crowd.space.insert(app, r);
        r
    }

    fn bodies(&mut self) -> Vec<Body> {
        let leader = self.party_leader();
        let now = self.time;
        let ids: Vec<(ObjectId, Vec3)> = self
            .iter()
            .filter(|o| o.kind == ObjectKind::Creature && o.creature().is_some_and(|c| !c.dead))
            .map(|o| (o.id, o.position))
            .collect();
        ids.into_iter()
            .map(|(id, pos)| {
                let moving = self.motion.contains_key(&id);
                // First sighting counts as settled: authored placement.
                let shifted = self.crowd.last_pos.get(&id).is_some_and(|p| p.distance_squared(pos) > 1e-6);
                if moving || shifted || self.in_combat(id) {
                    self.crowd.active_until.insert(id, now + ACTIVE_FOR);
                }
                Body {
                    id,
                    pos,
                    r: self.personal_space(id),
                    moving,
                    active: self.crowd.active_until.get(&id).is_some_and(|&t| t > now),
                    // The player-driven leader is moved by input only; NPCs
                    // give way to it (player_move keeps it out of them).
                    pinned: id == leader && !moving,
                }
            })
            .collect()
    }

    /// Pushes apart overlapping creatures after this tick's moves.
    pub(crate) fn separate_creatures(&mut self) {
        let mut bodies = self.bodies();
        // Cutscenes put actors on marks deliberately; leave them be.
        let hold = self.conversation.is_some();
        let n = bodies.len();
        let mut min_gap = f32::MAX;
        for pass in 0..PASSES {
            let mut push = vec![Vec2::ZERO; n];
            for i in 0..n {
                for j in i + 1..n {
                    let (a, b) = (bodies[i], bodies[j]);
                    if (a.pos.z - b.pos.z).abs() > MAX_DZ {
                        continue;
                    }
                    let d = (b.pos - a.pos).truncate();
                    let dist = d.length();
                    let gap = dist - (a.r + b.r);
                    if pass == 0 {
                        min_gap = min_gap.min(gap);
                    }
                    if gap >= 0.0 || hold || !(a.active || b.active) {
                        continue;
                    }
                    // Movers yield, idle creatures less, the leader not at all.
                    let w = |x: &Body| {
                        if x.pinned {
                            0.0
                        } else if x.moving {
                            1.0
                        } else {
                            0.35
                        }
                    };
                    let (wa, wb) = (w(&a), w(&b));
                    if wa + wb <= 0.0 {
                        continue;
                    }
                    // Coincident centres: split along a stable direction.
                    let dir = if dist > 1e-4 { d / dist } else { Vec2::from_angle((a.id ^ b.id) as f32 * 2.39996) };
                    let depth = -gap;
                    push[i] -= dir * depth * wa / (wa + wb);
                    push[j] += dir * depth * wb / (wa + wb);
                }
            }
            let mut any = false;
            for (k, body) in bodies.iter_mut().enumerate() {
                if push[k].length_squared() < 1e-8 {
                    continue;
                }
                any = true;
                let new = if self.walk.face_count() > 0 && self.walk.ground(body.pos.x, body.pos.y, body.pos.z + 0.5).is_some() {
                    self.walk.move_slide(body.pos, push[k], body.r.min(0.5))
                } else {
                    body.pos + push[k].extend(0.0)
                };
                body.pos = new;
                if let Some(o) = self.get_mut(body.id) {
                    o.position = new;
                }
            }
            if !any {
                break;
            }
        }
        for b in &bodies {
            self.crowd.last_pos.insert(b.id, b.pos);
        }
        self.crowd.min_gap = if min_gap == f32::MAX { 0.0 } else { min_gap };
    }

    /// Bends a mover's step around creatures ahead of it. `delta` is the
    /// planar step about to be taken; returns the adjusted step (same length).
    pub(crate) fn avoid_creatures(&mut self, id: ObjectId, pos: Vec3, delta: Vec2, dt: f32) -> Vec2 {
        let len = delta.length();
        if len < 1e-5 {
            return delta;
        }
        let dir = delta / len;
        let me = self.personal_space(id);
        let others: Vec<(ObjectId, Vec3)> = self
            .iter()
            .filter(|o| o.id != id && o.kind == ObjectKind::Creature && o.creature().is_some_and(|c| !c.dead))
            .filter(|o| (o.position - pos).truncate().length_squared() < 16.0 && (o.position.z - pos.z).abs() <= MAX_DZ)
            .map(|o| (o.id, o.position))
            .collect();
        let mut steer = Vec2::ZERO;
        let mut held = false;
        for (oid, op) in others {
            let rel = (op - pos).truncate();
            let ahead = rel.dot(dir);
            if ahead <= 0.0 {
                continue;
            }
            let reach = me + self.personal_space(oid) + LOOKAHEAD;
            let lateral = rel - dir * ahead;
            let lat = lateral.length();
            if ahead > reach + len || lat > reach {
                continue;
            }
            held |= lat < me + self.personal_space(oid);
            // Steer to the side the obstacle is not on; dead ahead -> right.
            let away = if lat > 1e-3 { -lateral / lat } else { Vec2::new(dir.y, -dir.x) };
            let strength = (1.0 - ahead / (reach + len)).clamp(0.0, 1.0);
            steer += away * strength;
        }
        let blocked = self.crowd.blocked_for.entry(id).or_insert(0.0);
        *blocked = if held { *blocked + dt } else { 0.0 };
        if steer == Vec2::ZERO {
            return delta;
        }
        // Held up for a while: commit to a side-step instead of a nudge.
        let gain = if *blocked > BLOCKED_SIDESTEP { 3.0 } else { 1.2 };
        (dir + steer * gain).normalize_or(dir) * len
    }

    /// Moves the player-controlled leader by `delta` with walkmesh sliding,
    /// blocked by other living creatures (KotOR: the player slides around
    /// NPCs, it does not push them).
    pub fn player_move(&mut self, from: Vec3, delta: Vec2, radius: f32) -> Vec3 {
        let leader = self.party_leader();
        let to = self.walk.move_slide(from, delta, radius);
        self.block_creatures(leader, from, to, radius)
    }

    /// Takes a step `from` -> `to` back out of any living creature it would
    /// walk into, sliding along their personal space. Overlap that already
    /// existed at `from` may stay but never gets deeper.
    pub(crate) fn block_creatures(&mut self, id: ObjectId, from: Vec3, to: Vec3, radius: f32) -> Vec3 {
        if self.conversation.is_some() {
            return to;
        }
        let me = self.personal_space(id).max(radius);
        let others: Vec<(ObjectId, Vec3)> = self
            .iter()
            .filter(|o| o.id != id && o.kind == ObjectKind::Creature && o.creature().is_some_and(|c| !c.dead))
            .filter(|o| (o.position - from).truncate().length_squared() < 25.0 && (o.position.z - from.z).abs() <= MAX_DZ)
            .map(|o| (o.id, o.position))
            .collect();
        if others.is_empty() {
            return to;
        }
        let mut pos = to;
        for _ in 0..2 {
            let mut fix = Vec2::ZERO;
            for &(oid, op) in &others {
                let min = me + self.personal_space(oid);
                let d = (pos - op).truncate();
                let dist = d.length();
                let was = (from - op).truncate().length();
                let target = if was < min { was } else { min };
                if dist >= target {
                    continue;
                }
                let n = if dist > 1e-4 { d / dist } else { (from - op).truncate().normalize_or(Vec2::X) };
                fix += n * (target - dist);
            }
            if fix == Vec2::ZERO {
                break;
            }
            pos = if self.walk.face_count() > 0 { self.walk.move_slide(pos, fix, radius) } else { pos + fix.extend(0.0) };
        }
        pos
    }

    /// Melee approach point for `id` around `target`: a slot on a ring of
    /// radius `ring`, spread so attackers don't stack. None = go straight at
    /// the target (no room, or no walkmesh under the slot).
    pub(crate) fn melee_slot(&mut self, id: ObjectId, target: ObjectId, ring: f32) -> Option<Vec3> {
        let tp = self.get(target)?.position;
        let me = self.get(id)?.position;
        // Other melee attackers already closing on / fighting the same target.
        let mut rivals: Vec<(ObjectId, Vec3)> = self
            .iter()
            .filter(|o| o.id != id && o.id != target && o.kind == ObjectKind::Creature && o.creature().is_some_and(|c| !c.dead))
            .filter(|o| self.combat.memory.get(&o.id).is_some_and(|m| m.attempted_target == target))
            .filter(|o| (o.position - tp).truncate().length() < ring + 3.0)
            .map(|o| (o.id, o.position))
            .collect();
        if rivals.is_empty() {
            return None;
        }
        rivals.sort_by_key(|r| r.0);
        let my_r = self.personal_space(id);
        // Angular width one attacker needs on the ring.
        let width = |r: f32| 2.0 * ((r + 0.1) / ring.max(r + 0.2)).clamp(0.0, 1.0).asin();
        let taken: Vec<(f32, f32)> = rivals
            .iter()
            .map(|&(rid, rp)| {
                let a = (rp - tp).truncate();
                (a.y.atan2(a.x), width(self.personal_space(rid)))
            })
            .collect();
        let mine = (me - tp).truncate();
        let base = mine.y.atan2(mine.x);
        let w = width(my_r);
        let free = |ang: f32| taken.iter().all(|&(t, tw)| angle_diff(ang, t).abs() >= (w + tw) * 0.5);
        // Nearest free angle to where we already stand, searching outwards.
        let step = w.max(0.15) * 0.5;
        let mut chosen = None;
        for k in 0..=((std::f32::consts::PI / step) as i32) {
            for s in [1.0, -1.0] {
                let ang = base + s * k as f32 * step;
                if free(ang) {
                    chosen = Some(ang);
                    break;
                }
            }
            if chosen.is_some() {
                break;
            }
        }
        let ang = chosen?;
        let p = tp + Vec2::from_angle(ang).extend(0.0) * ring;
        let h = self.walk.ground(p.x, p.y, tp.z + 1.0)?;
        if (h.z - tp.z).abs() > MAX_DZ {
            return None;
        }
        Some(Vec3::new(p.x, p.y, h.z))
    }
}

fn angle_diff(a: f32, b: f32) -> f32 {
    (a - b + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}
