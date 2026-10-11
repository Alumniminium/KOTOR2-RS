//! Party management: the party selection screen's roster and its result,
//! and switching the controlled character (party leader).
//!
//! K2 keeps up to 12 NPC slots (available_npcs, 0..=11); at most two of them
//! are in the field with the leader. Members left behind keep their state:
//! their object is stored on the slot and comes back on the next selection.

use k2_data::blueprint::Creature;
use k2_script::{ObjectId, OBJECT_INVALID};

use crate::object::Action;
use crate::{ScriptCtx, World, WorldEvent};

/// Party members in the field besides the leader (K2: two).
pub const MAX_FOLLOWERS: usize = 2;

/// HP a knocked-out party member gets up with after the battle. No source gives
/// the number. The game's hint says only "revived at the end of the battle".
/// UT_RestorePartyToOneHealth (k_inc_utility.nss, EffectResurrection then
/// EffectHeal(1)) would give 1, but no shipped script calls it. 1 is a choice
/// (it is below Atton's 10% Spirit revive). See docs/notes/combat.md.
pub const GET_UP_HP: i32 = 1;

/// Seconds after the battle ends before a knocked-out party member gets up
/// (the combat tail is 2 s more, so about 4 s after the last hit).
pub(crate) const GET_UP_DELAY: f32 = 2.0;

/// What ShowPartySelectionGUI asked for; the GUI answers with
/// [`World::apply_party_selection`] or [`World::cancel_party_selection`].
#[derive(Debug, Clone)]
pub struct PartySelectRequest {
    /// Runs after the screen closes (either way).
    pub exit_script: String,
    /// NPC slots that must be in the party (-1 = none).
    pub forced: [i32; 2],
    pub allow_cancel: bool,
}

impl Default for PartySelectRequest {
    fn default() -> Self {
        Self { exit_script: String::new(), forced: [-1, -1], allow_cancel: true }
    }
}

/// One roster slot as the selection screen shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct RosterEntry {
    pub npc: i32,
    pub name: String,
    pub appearance: u32,
    /// portraits.2da row from the blueprint (UTC PortraitId), if any.
    pub portrait: Option<u32>,
    pub level: i32,
    pub in_party: bool,
    pub selectable: bool,
    pub forced: bool,
}

impl World {
    /// Available NPC slots, in slot order.
    pub fn party_roster(&mut self) -> Vec<RosterEntry> {
        let forced = self.party_select.as_ref().map_or([-1, -1], |r| r.forced);
        let mut slots: Vec<i32> = self.available_npcs.keys().copied().collect();
        slots.sort();
        let mut out = Vec::new();
        for npc in slots {
            let Some(slot) = self.available_npcs.get(&npc).cloned() else { continue };
            let live = slot.object.filter(|&id| self.is_valid(id));
            let (name, appearance, level) = if let Some(o) = live.and_then(|id| self.get(id)).or(slot.stored.as_deref()) {
                let c = o.creature();
                (o.name.clone(), c.map_or(0, |c| c.appearance), c.map_or(1, |c| c.classes.iter().map(|&(_, l)| l.max(0)).sum::<i32>().max(1)))
            } else if let Ok(bp) = Creature::load(&self.res, &slot.template) {
                let name = format!("{} {}", self.resolve(&bp.first_name), self.resolve(&bp.last_name)).trim().to_owned();
                (name, bp.appearance, bp.classes.iter().map(|&(_, l)| l.max(0)).sum::<i32>().max(1))
            } else {
                (slot.template.clone(), 0, 1)
            };
            let portrait = Creature::load(&self.res, &slot.template).ok().map(|bp| bp.portrait).filter(|&p| p > 0);
            out.push(RosterEntry {
                npc,
                name: self.substitute(&name),
                appearance,
                portrait,
                level,
                in_party: live.is_some_and(|id| self.party.contains(&id)),
                selectable: slot.selectable,
                forced: forced.contains(&npc),
            });
        }
        out
    }

    /// Puts exactly `npcs` (plus forced slots, at most two) in the field with
    /// the leader, then runs the request's exit script.
    pub fn apply_party_selection(&mut self, npcs: &[i32]) -> Result<(), String> {
        let req = self.party_select.clone().unwrap_or_default();
        let mut want: Vec<i32> = req.forced.iter().copied().filter(|&n| n >= 0).collect();
        for &n in npcs {
            if !want.contains(&n) {
                want.push(n);
            }
        }
        if want.len() > MAX_FOLLOWERS {
            return Err(format!("{} members picked, {MAX_FOLLOWERS} allowed", want.len()));
        }
        for &n in &want {
            match self.available_npcs.get(&n) {
                None => return Err(format!("npc {n} is not available")),
                Some(s) if !s.selectable && !req.forced.contains(&n) => return Err(format!("npc {n} is not selectable")),
                _ => {}
            }
        }
        let leader = self.party_leader();
        // Leave behind whoever is not wanted (the leader always stays).
        let mut slots: Vec<i32> = self.available_npcs.keys().copied().collect();
        slots.sort();
        for n in slots {
            if want.contains(&n) {
                continue;
            }
            let Some(id) = self.available_npcs.get(&n).and_then(|s| s.object).filter(|&id| self.is_valid(id)) else { continue };
            if id == leader || id == self.player || !self.party.contains(&id) {
                continue;
            }
            self.party.retain(|&p| p != id);
            if let Some(o) = self.objects.remove(&id) {
                self.emit(WorldEvent::ObjectDestroyed(id));
                if let Some(s) = self.available_npcs.get_mut(&n) {
                    s.stored = Some(Box::new(o));
                    s.object = None;
                }
            }
        }
        // Bring the wanted ones in next to the leader.
        let loc = self.location_of(leader);
        for (i, &n) in want.iter().enumerate() {
            let id = self.bring_npc(n, loc, i);
            if id != OBJECT_INVALID && !self.party.contains(&id) {
                self.party.push(id);
            }
        }
        self.finish_party_selection(req);
        Ok(())
    }

    /// Closes the screen without changes (when the request allows it).
    pub fn cancel_party_selection(&mut self) {
        let req = self.party_select.clone().unwrap_or_default();
        self.finish_party_selection(req);
    }

    fn finish_party_selection(&mut self, req: PartySelectRequest) {
        self.party_select = None;
        if !req.exit_script.is_empty() {
            let leader = self.party_leader();
            self.run_script(&req.exit_script, leader, ScriptCtx::default());
        }
    }

    /// The slot's creature in the area: the stored one if it was left
    /// behind, else spawned (or moved) by SpawnAvailableNPC rules.
    fn bring_npc(&mut self, npc: i32, loc: crate::engine::Location, i: usize) -> ObjectId {
        let side = if i.is_multiple_of(2) { -1.2 } else { 1.2 };
        let offset = glam::Quat::from_rotation_z(crate::object::facing_to_bearing(loc.facing)) * glam::Vec3::new(side, -1.5, 0.0);
        let mut at = loc;
        at.position += offset;
        if let Some(mut o) = self.available_npcs.get_mut(&npc).and_then(|s| s.stored.take()) {
            let id = o.id;
            o.position = at.position;
            self.objects.insert(id, *o);
            self.place(id, at.position);
            if let Some(s) = self.available_npcs.get_mut(&npc) {
                s.object = Some(id);
            }
            self.emit(WorldEvent::ObjectSpawned(id));
            return id;
        }
        self.spawn_npc(npc, at)
    }

    /// Makes `id` (a party member in the field) the controlled character.
    pub fn set_party_leader(&mut self, id: ObjectId) -> bool {
        if self.conversation.is_some() || !self.party.contains(&id) || !self.is_standing(id) {
            return false;
        }
        let old = self.party_leader();
        if old == id {
            return true;
        }
        self.party.retain(|&p| p != id);
        self.party.insert(0, id);
        // The new leader stops what its AI was doing; the old one goes back
        // to following (party AI).
        self.clear_new_leader_queue(id);
        self.emit(WorldEvent::LeaderChanged { old, new: id });
        true
    }

    /// The player's own switches (portrait, Tab) empty the new leader's queue,
    /// as they did before the follow actions existed. Heartbeat follows cannot
    /// hold control afterwards: a leader cannot queue Follow(Leader) (nav.rs
    /// queue_follow) and one that is queued completes at once (tick.rs).
    fn clear_new_leader_queue(&mut self, id: ObjectId) {
        if let Some(o) = self.get_mut(id) {
            o.actions.clear();
            // A talk that waited on the queue must see the clear even if the queue
            // refills to its old length (lib.rs talk_to).
            o.action_epoch += 1;
        }
    }

    /// Removes the follow entries from a new leader's queue and keeps its other
    /// actions. Used where the switch is not the player's: a full clear would
    /// drop an ActionDoCommand or a conversation resume and could leave a
    /// conversation paused. Every follow goes, not only Follow(Leader): a hold
    /// (ActionForceFollowObject, ActionFollowOwner) on the new leader would lock
    /// control until ClearAllActions.
    pub(crate) fn drop_follows(&mut self, id: ObjectId) {
        if let Some(o) = self.get_mut(id) {
            o.actions.retain(|a| !matches!(a, Action::Follow { .. }));
        }
    }

    /// Tab: control rotates to the next standing party member in the field.
    pub fn cycle_party_leader(&mut self) -> Option<ObjectId> {
        if self.party.len() < 2 || self.solo_mode || self.conversation.is_some() {
            return None;
        }
        let old = self.party_leader();
        let k = (1..self.party.len()).find(|&k| self.is_standing(self.party[k]))?;
        self.party.rotate_left(k);
        let new = self.party_leader();
        self.clear_new_leader_queue(new);
        self.emit(WorldEvent::LeaderChanged { old, new });
        Some(new)
    }

    /// A party member in the field that is neither dead nor knocked out.
    pub fn is_standing(&self, id: ObjectId) -> bool {
        self.get(id).and_then(|o| o.creature()).is_some_and(|c| !c.dead && !c.down)
    }

    /// Knocked out in the field: at 0 HP, not dead, waiting to get up.
    pub fn is_down(&self, id: ObjectId) -> bool {
        self.get(id).and_then(|o| o.creature()).is_some_and(|c| c.down && !c.dead)
    }

    /// Every party member is dead or knocked out: the game is over.
    pub fn party_defeated(&self) -> bool {
        !self.party.is_empty() && !self.party.iter().any(|&p| self.is_standing(p))
    }

    /// A party member just lost its last hit points (dead or knocked out).
    /// A fallen leader hands control to the next standing member; with nobody
    /// standing the party is defeated.
    pub(crate) fn party_member_down(&mut self, id: ObjectId) {
        if !self.party.iter().any(|&p| self.is_standing(p)) {
            self.emit(WorldEvent::PartyDefeated);
            return;
        }
        if self.party_leader() == id
            && let Some((old, new)) = self.hand_over_if_leader_down()
        {
            self.emit(WorldEvent::LeaderChanged { old, new });
        }
    }

    /// The leader is not standing and another member is: that member takes
    /// control. The hand-over party_member_down makes when a leader falls, and
    /// settle_loaded_party makes on load. Returns (old, new) when control moved.
    fn hand_over_if_leader_down(&mut self) -> Option<(ObjectId, ObjectId)> {
        let old = self.party_leader();
        if self.is_standing(old) {
            return None;
        }
        let next = self.party.iter().copied().find(|&p| self.is_standing(p))?;
        self.party.retain(|&p| p != next);
        self.party.insert(0, next);
        self.drop_follows(next);
        Some((old, next))
    }

    /// Gets a knocked-out creature up with `hp` (at least [`GET_UP_HP`]).
    /// Its followers AI brings it back to the leader (party_follow). A raise
    /// with the leader still down hands control over (hand_over_if_leader_down).
    pub(crate) fn get_up(&mut self, id: ObjectId, hp: i32) {
        let Some(c) = self.get_mut(id).and_then(|o| o.creature_mut()) else { return };
        if !c.down || c.dead {
            return;
        }
        c.down = false;
        c.get_up_in = 0.0;
        c.hp = hp.max(GET_UP_HP);
        self.emit(WorldEvent::GotUp { object: id });
        self.emit_hand_over();
    }

    /// LeaderChanged for a hand-over made after a raise (see get_up).
    fn emit_hand_over(&mut self) {
        if let Some((old, new)) = self.hand_over_if_leader_down() {
            self.emit(WorldEvent::LeaderChanged { old, new });
        }
    }

    /// EffectResurrection(percent): a dead or knocked-out creature stands again
    /// with `percent` of its max HP (integer division). A party member that
    /// would get up with no HP stays (or goes) down at 0 HP: a heal (EffectHeal,
    /// the Revitalize scripts) or the battle-end revive raises it. Anyone else
    /// stands with at least [`GET_UP_HP`]. A slain creature (EffectDeath) does
    /// not come back at all (combat.md).
    pub(crate) fn resurrect(&mut self, id: ObjectId, percent: i32) {
        let in_party = self.party.contains(&id);
        let Some(c) = self.get_mut(id).and_then(|o| o.creature_mut()) else { return };
        if c.slain || (!c.dead && !c.down) {
            return;
        }
        let hp = c.max_hp * percent.clamp(0, 100) / 100;
        c.dead = false;
        if in_party && hp <= 0 {
            c.down = true;
            c.hp = 0;
            c.get_up_in = GET_UP_DELAY;
            return;
        }
        c.down = false;
        c.get_up_in = 0.0;
        c.hp = hp.max(GET_UP_HP);
        self.emit(WorldEvent::GotUp { object: id });
        self.emit_hand_over();
    }

    /// EffectResurrection(percent) from outside the scripts (test hook).
    pub fn resurrect_creature(&mut self, id: ObjectId, percent: i32) {
        self.resurrect(id, percent);
        self.pump();
    }

    /// The battle is over once no standing party member is in combat. Then
    /// each downed member gets up after [`GET_UP_DELAY`] seconds. This revive
    /// raises nobody while the party is defeated; a scripted raise can, and
    /// then hands control over (get_up).
    pub(crate) fn run_party_revival(&mut self, dt: f32) {
        if !self.party.iter().any(|&p| self.is_down(p)) || self.party_defeated() {
            return;
        }
        let fighting = self.party.iter().any(|&p| self.is_standing(p) && self.in_combat(p));
        for id in self.party.clone() {
            if !self.is_down(id) {
                continue;
            }
            let Some(c) = self.get_mut(id).and_then(|o| o.creature_mut()) else { continue };
            if fighting {
                c.get_up_in = GET_UP_DELAY;
                continue;
            }
            c.get_up_in -= dt;
            if c.get_up_in <= 0.0 {
                self.get_up(id, GET_UP_HP);
            }
        }
    }

    /// Saves written before fallen party members got up stored them as dead
    /// at 0 HP (not slain). They load knocked out instead, and a leader that
    /// lies hands control to a standing member.
    pub(crate) fn settle_loaded_party(&mut self) {
        for id in self.party.clone() {
            let Some(c) = self.get_mut(id).and_then(|o| o.creature_mut()) else { continue };
            if c.dead && !c.slain && c.hp <= 0 {
                c.dead = false;
                c.down = true;
                c.get_up_in = GET_UP_DELAY;
            }
        }
        // On load nobody is listening for LeaderChanged: the presentation takes
        // the leader from party_leader() when the module is set up.
        let _ = self.hand_over_if_leader_down();
    }

    /// The portrait texture (portraits.2da baseresref) for a creature: the
    /// PC's chargen pick, else its blueprint's PortraitId, else the first
    /// row whose appearance columns match.
    pub fn portrait_resref(&self, id: ObjectId) -> Option<String> {
        let t = self.tables.get(&self.res, "portraits").ok()?;
        let o = self.get(id).or(self.stashed_pc.as_ref().filter(|p| p.id == id))?;
        let c = o.creature()?;
        let row = if c.is_pc && o.template.is_empty() {
            self.player_spec.portrait
        } else {
            Creature::load(&self.res, &o.template).ok().map(|bp| bp.portrait).filter(|&p| p > 0)
        };
        if let Some(r) = row
            && let Some(name) = t.str(r as usize, "baseresref")
        {
            return Some(name.to_ascii_lowercase());
        }
        let a = c.appearance as i32;
        (0..t.rows())
            .find(|&r| ["appearancenumber", "appearance_s", "appearance_l"].iter().any(|col| t.int(r, col) == Some(a)))
            .and_then(|r| t.str(r, "baseresref").map(str::to_ascii_lowercase))
    }
}

#[cfg(test)]
mod revive_tests;
