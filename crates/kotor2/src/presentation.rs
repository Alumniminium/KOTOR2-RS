//! Ranged hits are shown when their bolt lands.
//!
//! The world applies a blaster hit when the attack resolves: the rules and the
//! AI depend on that timing, so it stays. But the bolt takes its launch and its
//! flight to reach the target, and nothing should show before it does. So a
//! bolt's own outcome (the AttackHit, AttackMissed or AttackDeflected the same
//! tick as its Projectile) is held here, with the damage, knock-down or death
//! that the same tick does to that target: the damage numbers, the miss text,
//! the pain voice, the attack's log line, and a death (animate_creatures keeps a
//! dying creature in its pose meanwhile). The impact sound and the flash follow
//! the bolt on their own (sfx.rs, projectiles.rs).
//!
//! Nothing else is held: melee, spells, grenades and any other damage show as
//! they happen, even from a shooter whose bolt is in the air. Not held: the
//! health bars, which read the world's state and drop at once.

use bevy::prelude::*;
use k2_script::ObjectId;
use k2_world::WorldEvent;

pub struct PresentationPlugin;

impl Plugin for PresentationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Gate>();
    }
}

#[derive(Resource, Default)]
pub struct Gate {
    /// Held events, each with the client time it is shown (always after it was held).
    held: Vec<(f32, WorldEvent)>,
}

impl Gate {
    /// This frame's events, drained from the world at client time `now` (the
    /// virtual clock, which the bolts fly on). The held events that are due come
    /// first. A bolt's own outcome is held until the bolt lands, and so are the
    /// Damaged, KnockedDown and Died that its hit does to the same target in this
    /// batch; everything else passes. A new area drops whatever is held.
    pub fn filter(&mut self, now: f32, events: Vec<WorldEvent>) -> Vec<WorldEvent> {
        if events.iter().any(|e| matches!(e, WorldEvent::ModuleEntered { .. })) {
            self.held.clear();
        }
        let (due, held): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held).into_iter().partition(|(at, _)| *at <= now);
        self.held = held;
        let mut out: Vec<WorldEvent> = due.into_iter().map(|(_, ev)| ev).collect();
        // The bolt whose outcome is next in this batch, and the outcomes held in it: (shooter, target, landing).
        let mut bolt: Option<(ObjectId, f32)> = None;
        let mut outcomes: Vec<(ObjectId, ObjectId, f32)> = Vec::new();
        for ev in events {
            if let WorldEvent::Projectile { shooter, bolt: true, launch, flight, .. } = &ev {
                bolt = Some((*shooter, now + launch + flight));
                out.push(ev);
                continue;
            }
            let outcome = match &ev {
                WorldEvent::AttackHit { attacker, target, .. } | WorldEvent::AttackMissed { attacker, target } | WorldEvent::AttackDeflected { attacker, target } => Some((*attacker, *target)),
                _ => None,
            };
            if let Some((attacker, target)) = outcome
                && let Some((shooter, at)) = bolt
                && shooter == attacker
            {
                bolt = None;
                outcomes.push((attacker, target, at));
                self.held.push((at, ev));
                continue;
            }
            let hit = match &ev {
                WorldEvent::Damaged { object, by, .. } | WorldEvent::KnockedDown { object, by } => Some((*by, *object)),
                WorldEvent::Died { object, killer } => Some((*killer, *object)),
                _ => None,
            };
            let landing = hit.and_then(|(source, target)| outcomes.iter().find(|(s, t, _)| *s == source && *t == target).map(|o| o.2));
            match landing {
                Some(at) => self.held.push((at, ev)),
                None => out.push(ev),
            }
        }
        out
    }

    /// Whether a held death or knock-down keeps `object` in its pose: the body
    /// stays and animate_creatures holds the pose until the hit lands.
    pub fn holds_pose(&self, object: ObjectId) -> bool {
        self.held.iter().any(|(_, ev)| matches!(ev, WorldEvent::Died { object: o, .. } | WorldEvent::KnockedDown { object: o, .. } if *o == object))
    }
}

#[cfg(test)]
mod tests;
