//! Per-frame simulation: timers, heartbeats, action queues, triggers and
//! perception.

use glam::{Vec2, Vec3};
use k2_script::ObjectId;
use tracing::info_span;

use crate::object::{bearing_towards, Action, Detail, FollowTarget, ObjectKind, Target};
use crate::{ScriptCtx, Timer, TimerKind, World, WorldEvent};

const HEARTBEAT: f32 = 6.0;
const PERCEPTION_INTERVAL: f32 = 0.5;
/// Fire-and-forget animations have no known length here (the Bevy side
/// knows the real one); assume this.
const DEFAULT_ANIM_SECONDS: f32 = 1.5;

/// How an object is moving this frame (for walk/run animation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Motion {
    Still,
    Walk,
    Run,
}

impl World {
    /// Advances the simulation by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        let _span = info_span!("world_tick").entered();
        self.time += f64::from(dt);
        self.input_blocked_for = (self.input_blocked_for - dt).max(0.0);
        self.run_timers();
        self.tick_doors(dt);
        self.run_heartbeats(dt);
        self.paths.tick(dt);
        self.run_actions(dt);
        self.run_combat(dt);
        self.party_follow(dt);
        self.separate_creatures();
        self.run_triggers();
        self.run_encounters(dt);
        self.run_traps(dt);
        self.run_stealth(dt);
        self.perception_in -= dt;
        if self.perception_in <= 0.0 {
            self.perception_in = PERCEPTION_INTERVAL;
            self.run_perception();
        }
        self.run_powers(dt);
        self.tick_minigame(dt);
        self.tick_conversation(dt);
        self.pump();
    }

    fn run_timers(&mut self) {
        let now = self.time;
        let mut due = Vec::new();
        self.timers.retain(|t| {
            if t.due <= now {
                due.push(t.clone());
                false
            } else {
                true
            }
        });
        due.sort_by(|a, b| a.due.total_cmp(&b.due));
        for t in due {
            match t.kind {
                TimerKind::Resume(state) => {
                    let caller = state.caller;
                    if self.is_valid(caller) {
                        let queued = self.timers.len();
                        self.resume(&state, caller);
                        self.pump();
                        // A script that queued its own next resume is a loop. A turn
                        // that queues none ends the loop, unless another resume of the
                        // script is still pending (the loop's next turn, queued earlier).
                        // Defensive: a timer list cleared during the resume leaves nothing to see.
                        let same = |t: &Timer| matches!(&t.kind, TimerKind::Resume(s) if s.caller == caller && s.script.name == state.script.name);
                        let again = self.timers.get(queued..).unwrap_or(&[]).iter().any(same);
                        let pending = again || self.timers.iter().any(same);
                        if caller == self.area_id && (again || !pending) {
                            self.record_area_turn(&state.script.name, again);
                        }
                    }
                }
                TimerKind::Destroy(id) => self.destroy(id),
            }
        }
    }

    fn run_heartbeats(&mut self, dt: f32) {
        let mut fire = Vec::new();
        for o in self.objects.values_mut() {
            if o.destroyed {
                continue;
            }
            let label = match o.kind {
                ObjectKind::Module => "Mod_OnHeartbeat",
                ObjectKind::Creature | ObjectKind::Trigger => "ScriptHeartbeat",
                ObjectKind::Area | ObjectKind::Door | ObjectKind::Placeable => "OnHeartbeat",
                _ => continue,
            };
            if o.script(label).is_none() {
                continue;
            }
            o.heartbeat_in -= dt;
            if o.heartbeat_in <= 0.0 {
                o.heartbeat_in += HEARTBEAT;
                fire.push((o.id, label));
            }
        }
        for (id, label) in fire {
            self.queue_event(id, label, ScriptCtx::default());
        }
        self.pump();
    }

    fn run_actions(&mut self, dt: f32) {
        // Only objects that moved this tick have an entry (missing = still).
        self.motion.clear();
        let ids: Vec<ObjectId> = self.objects.values().filter(|o| !o.destroyed && !o.actions.is_empty()).map(|o| o.id).collect();
        for id in ids {
            // Several instant actions can complete in one frame.
            for _ in 0..16 {
                let Some((action, epoch)) = self.get(id).and_then(|o| o.actions.front().cloned().map(|a| (a, o.action_epoch)))
                else {
                    break;
                };
                let (done, updated) = self.step_action(id, action, dt);
                let Some(o) = self.get_mut(id) else { break };
                if o.action_epoch != epoch {
                    // ClearAllActions ran during the step; the queue is new.
                    continue;
                }
                if done {
                    o.actions.pop_front();
                    self.pump();
                } else {
                    if let Some(a) = updated
                        && let Some(front) = o.actions.front_mut()
                    {
                        *front = a;
                    }
                    break;
                }
            }
        }
    }

    fn target_pos(&self, t: Target) -> Option<Vec3> {
        match t {
            Target::Point(p) => Some(p),
            Target::Object(o) => self.get(o).map(|o| o.position),
        }
    }

    /// Runs one step of an action. Returns (done, replacement state).
    fn step_action(&mut self, id: ObjectId, action: Action, dt: f32) -> (bool, Option<Action>) {
        // Stunned, paralyzed, choked, ...: the queue waits (powers.rs).
        if self.is_debilitated(id) {
            return (false, None);
        }
        match action {
            Action::MoveTo { target, run, range, timeout, elapsed, stuck } => {
                let Some(dest) = self.target_pos(target) else { return (true, None) };
                let elapsed = elapsed + dt;
                if timeout.is_some_and(|t| elapsed >= t) {
                    // Forced moves end at the destination.
                    self.place(id, dest);
                    return (true, None);
                }
                let (moved, arrived) = self.move_towards(id, dest, run, range, dt);
                // Unforced moves also end where their route ran out (nearest
                // reachable spot); forced moves wait for their timeout.
                let pos = self.get(id).map_or(dest, |o| o.position);
                let arrived = arrived || (timeout.is_none() && self.paths.at_end(id, pos));
                // A wait for a door to swing is not stuck: the move resumes when the door is open.
                let stuck = if self.door_waiting {
                    stuck
                } else if moved < 1e-3 {
                    stuck + dt
                } else {
                    0.0
                };
                // Routes come from nav.rs; a move with no route or no progress
                // for 1.5 s gives up.
                if arrived || stuck > 1.5 {
                    if stuck > 1.5 && timeout.is_some() {
                        self.place(id, dest);
                    }
                    self.paths.forget(id);
                    return (true, None);
                }
                (false, Some(Action::MoveTo { target, run, range, timeout, elapsed, stuck }))
            }
            Action::MoveAway { from, run, range, elapsed } => {
                let (Some(me), Some(other)) = (self.get(id).map(|o| o.position), self.get(from).map(|o| o.position)) else {
                    return (true, None);
                };
                let away = (me - other).truncate().normalize_or(Vec2::X);
                if me.distance(other) >= range || elapsed > 10.0 {
                    return (true, None);
                }
                let dest = me + away.extend(0.0) * 2.0;
                let (moved, _) = self.move_towards(id, dest, run, 0.0, dt);
                if moved < 1e-3 {
                    return (true, None);
                }
                (false, Some(Action::MoveAway { from, run, range, elapsed: elapsed + dt }))
            }
            Action::Follow { of, range } => {
                // Working interpretation for ActionFollowLeader (233 call sites,
                // many in heartbeats): it yields to the next queued action.
                // nwscript (ActionFollowOwner, ActionForceFollowObject): once
                // queued, the follow is the only thing the creature does until
                // ClearAllActions, so it never yields.
                let target = match of {
                    FollowTarget::Leader => Some(self.party_leader()),
                    FollowTarget::Owner => self.get(id).and_then(|o| o.owner),
                    FollowTarget::Object(t) => Some(t),
                };
                // Done at once: a follow of the follower itself, and any follow but
                // ActionFollowLeader on the party leader, which would hold control.
                // A leader follow also yields to the next queued action.
                let done = match of {
                    FollowTarget::Leader => target == Some(id) || self.get(id).is_some_and(|o| o.actions.len() > 1),
                    _ => target == Some(id) || id == self.party_leader(),
                };
                if done {
                    self.paths.forget(id);
                    return (true, None);
                }
                match target.filter(|&t| self.is_valid(t)) {
                    // Party members hold still during dialog, as party_follow does.
                    Some(target) if self.conversation.is_none() => self.follow_towards(id, target, range, dt),
                    // No owner set, or in dialog: stand still and keep waiting.
                    _ => self.paths.forget(id),
                }
                (false, Some(Action::Follow { of, range }))
            }
            Action::JumpTo { target, bearing } => {
                if let Some(p) = self.target_pos(target) {
                    self.place(id, p);
                    let b = match (bearing, target) {
                        (Some(b), _) => Some(b),
                        (None, Target::Object(o)) => self.get(o).map(|o| o.bearing),
                        _ => None,
                    };
                    if let (Some(b), Some(o)) = (b, self.get_mut(id)) {
                        o.bearing = b;
                    }
                }
                (true, None)
            }
            Action::Wait { remaining } => {
                let r = remaining - dt;
                if r <= 0.0 { (true, None) } else { (false, Some(Action::Wait { remaining: r })) }
            }
            Action::PlayAnimation { anim, speed, duration, started, remaining } => {
                if !started {
                    let looping = is_looping_anim(anim);
                    let d = if looping { duration } else { DEFAULT_ANIM_SECONDS / speed.max(0.05) };
                    self.emit(WorldEvent::PlayAnimation { object: id, animation: anim, speed, duration, looping });
                    // Looping with no duration: fire and continue (engine
                    // keeps it playing until something else replaces it).
                    if looping && duration <= 0.0 {
                        return (true, None);
                    }
                    return (false, Some(Action::PlayAnimation { anim, speed, duration, started: true, remaining: d - dt }));
                }
                let r = remaining - dt;
                if r <= 0.0 {
                    (true, None)
                } else {
                    (false, Some(Action::PlayAnimation { anim, speed, duration, started, remaining: r }))
                }
            }
            Action::OpenDoor(door) | Action::CloseDoor(door) => {
                let open = matches!(action, Action::OpenDoor(_));
                let kind = self.get(door).map(|o| o.kind);
                match kind {
                    Some(ObjectKind::Door) => {
                        let locked = self.get(door).and_then(|o| o.door()).is_some_and(|d| d.locked);
                        if open && locked {
                            self.queue_event(door, "OnFailToOpen", ScriptCtx { clicking: id, ..Default::default() });
                        } else {
                            self.set_door_open(door, open, id);
                        }
                    }
                    Some(ObjectKind::Placeable) => {
                        if let Some(p) = self.get_mut(door).and_then(|o| o.placeable_mut()) {
                            p.open = open;
                        }
                        self.emit(WorldEvent::PlaceableChanged { placeable: door, open });
                        self.queue_event(door, if open { "OnOpen" } else { "OnClosed" }, ScriptCtx {
                            last_opened_by: id,
                            last_closed_by: id,
                            ..Default::default()
                        });
                    }
                    _ => {}
                }
                (true, None)
            }
            Action::Lock(target, lock) => {
                self.set_locked(target, lock);
                self.queue_event(target, if lock { "OnLock" } else { "OnUnlock" }, ScriptCtx::default());
                (true, None)
            }
            Action::StartConversation { with, dialog, bark } => {
                if self.conversation.is_none() {
                    let dlg = if dialog.is_empty() {
                        self.get(id).and_then(|o| o.conversation.clone()).unwrap_or_default()
                    } else {
                        dialog
                    };
                    // The non-party participant owns the conversation: a PC
                    // told to converse with an NPC talks through the NPC's
                    // dialog, an NPC (or placeable) starting one owns it.
                    let in_party = |w: &World, o: ObjectId| w.party.contains(&o);
                    let (owner, pc) = if in_party(self, id) && self.is_valid(with) && !in_party(self, with) {
                        (with, id)
                    } else {
                        (id, if self.is_valid(with) && in_party(self, with) { with } else { self.party_leader() })
                    };
                    let dlg = if dlg.is_empty() { self.get(owner).and_then(|o| o.conversation.clone()).unwrap_or_default() } else { dlg };
                    if !dlg.is_empty() {
                        self.start_conversation_at(owner, pc, &dlg, bark);
                    }
                }
                (true, None)
            }
            Action::DoCommand(state) => {
                self.resume(&state, id);
                (true, None)
            }
            Action::Speak { text, volume } => {
                self.emit(WorldEvent::Speak { object: id, text: text.clone() });
                self.hear_speech(id, &text, volume);
                (true, None)
            }
            Action::Attack(target) => self.step_attack(id, target, dt),
            Action::PauseConversation => {
                if let Some(c) = &mut self.conversation {
                    c.paused = true;
                }
                (true, None)
            }
            Action::ResumeConversation => {
                if let Some(c) = &mut self.conversation {
                    c.paused = false;
                }
                (true, None)
            }
            Action::Noop(_) => (true, None),
            Action::SwitchWeapons => {
                // Not in dialogs (nwscript: "Does not work in dialogs"). Only to a
                // set that holds a weapon: the AI's switch never leaves the creature
                // unarmed (k_inc_generic GN_CheckRangedPreferred swaps back and forth).
                if self.conversation.is_none() && self.other_weapon_set_armed(id) {
                    self.swap_weapon_sets(id);
                }
                (true, None)
            }
            Action::Cast(c) => self.step_cast(id, *c, dt),
        }
    }

    /// Moves `id` towards `dest` for one frame. Returns (distance moved,
    /// arrived within `range`).
    pub(crate) fn move_towards(&mut self, id: ObjectId, dest: Vec3, run: bool, range: f32, dt: f32) -> (f32, bool) {
        self.door_waiting = false;
        let Some(o) = self.get(id) else { return (0.0, true) };
        let pos = o.position;
        let (speed, radius) = o.creature().map_or((1.75, 0.3), |c| (if run { c.run_speed } else { c.walk_speed }, c.radius));
        let speed = speed * self.speed_factor(id);
        let dist = (dest - pos).truncate().length();
        let range = range.max(0.1);
        if dist <= range {
            self.paths.forget(id);
            return (0.0, true);
        }
        // Steer along a walkmesh path (nav.rs); None = no route, stand still
        // so the caller's stuck handling ends the action.
        let Some(aim) = self.steer(id, pos, dest, radius.min(0.5), None) else {
            return (0.0, false);
        };
        // A closed door on the leg: a mover in reach opens it as a walker does, and waits for the swing.
        if let Some(door) = self.door_across(pos, aim)
            && self.in_use_reach(pos, door)
        {
            if self.get(door).and_then(|o| o.door()).is_some_and(|d| d.locked) {
                // A locked door ends the move at the door (the use gives the locked-door feedback).
                self.use_object(id, door);
                return (0.0, true);
            }
            if !self.get(door).and_then(|o| o.door()).is_some_and(|d| d.open) {
                self.use_object(id, door);
            }
            // Waits for the swing only: a door that is open and has swung is walked through, and one that did not open stalls the move.
            let (open, swinging) = self.get(door).and_then(|o| o.door()).map_or((false, false), |d| (d.open, d.opening > 0.0));
            if swinging {
                self.door_waiting = true;
                return (0.0, false);
            }
            if !open {
                return (0.0, false);
            }
        }
        let to = (aim - pos).truncate();
        let aim_dist = to.length().max(1e-4);
        let step = (speed * dt).min(dist - range * 0.5).min(aim_dist.max(0.05));
        let delta = self.avoid_creatures(id, pos, to / aim_dist * step, dt);
        let new = if self.walk.face_count() > 0 && self.walk.ground(pos.x, pos.y, pos.z + 0.5).is_some() {
            self.walk.move_slide(pos, delta, radius.min(0.5))
        } else {
            let mut p = pos + delta.extend(0.0);
            if let Some(h) = self.walk.ground(p.x, p.y, p.z + 1.0) {
                p.z = h.z;
            }
            p
        };
        let new = self.block_creatures(id, pos, new, radius.min(0.5));
        let moved = (new - pos).truncate().length();
        self.note_progress(id, moved, step);
        if let Some(o) = self.get_mut(id) {
            o.position = new;
            if let Some(b) = bearing_towards(pos, aim) {
                o.bearing = b;
            }
        }
        self.motion.insert(id, if run { Motion::Run } else { Motion::Walk });
        (moved, (dest - new).truncate().length() <= range)
    }

    /// Teleports an object, snapping to ground when there is one.
    pub fn place(&mut self, id: ObjectId, mut p: Vec3) {
        if let Some(h) = self.walk.ground(p.x, p.y, p.z + 1.0) {
            p.z = h.z;
        }
        if let Some(o) = self.get_mut(id) {
            o.position = p;
        }
    }

    pub(crate) fn set_locked(&mut self, target: ObjectId, lock: bool) {
        if let Some(o) = self.get_mut(target) {
            match &mut o.detail {
                Detail::Door(d) => d.locked = lock,
                Detail::Placeable(p) => p.locked = lock,
                _ => {}
            }
        }
        self.sync_door_passable();
    }

    /// Trigger enter/exit for creatures, plus transitions for the leader.
    pub(crate) fn run_triggers(&mut self) {
        let creatures: Vec<(ObjectId, Vec3)> = self
            .iter()
            .filter(|o| o.kind == ObjectKind::Creature && o.creature().is_some_and(|c| !c.dead))
            .map(|o| (o.id, o.position))
            .collect();
        let leader = self.party_leader();
        let mut enters = Vec::new();
        let mut exits = Vec::new();
        for o in self.objects.values_mut() {
            if o.destroyed {
                continue;
            }
            let Detail::Trigger(t) = &mut o.detail else { continue };
            for &(cid, p) in &creatures {
                let inside = k2_data::module::point_in_polygon(&t.polygon, p.x, p.y);
                let was = t.inside.contains(&cid);
                if inside && !was {
                    t.inside.insert(cid);
                    enters.push((o.id, cid, t.is_transition, t.linked_to_module.clone(), t.linked_to.clone()));
                } else if !inside && was {
                    t.inside.remove(&cid);
                    exits.push((o.id, cid));
                }
            }
            t.inside.retain(|id| creatures.iter().any(|(c, _)| c == id));
        }
        for (trig, cid, transition, module, linked_to) in enters {
            if self.trace {
                println!("  trigger enter {} by {}", self.describe(trig), self.describe(cid));
            }
            self.queue_event(trig, "ScriptOnEnter", ScriptCtx { entering: cid, clicking: cid, ..Default::default() });
            if transition && cid == leader && self.conversation.is_none() {
                match module {
                    Some(m) => {
                        if !self.transit_blocked() {
                            self.request_transition(m, linked_to, Vec::new());
                        }
                    }
                    None => {
                        if let Some(dest) = linked_to.as_deref().and_then(|t| self.by_tag(t, 0)) {
                            let (p, b) = self.get(dest).map(|o| (o.position, o.bearing)).unwrap();
                            self.place(cid, p);
                            if let Some(o) = self.get_mut(cid) {
                                o.bearing = b;
                            }
                        }
                    }
                }
            }
        }
        for (trig, cid) in exits {
            self.queue_event(trig, "ScriptOnExit", ScriptCtx { exiting: cid, ..Default::default() });
        }
        self.pump();
    }

    /// Creatures with an OnNotice script notice party members they can see
    /// (range, line of sight, stealth); see `World::can_see`.
    fn run_perception(&mut self) {
        let party: Vec<ObjectId> = self.party.iter().copied().filter(|&id| self.get(id).is_some()).collect();
        let observers: Vec<(ObjectId, Vec3, f32)> = self
            .objects
            .values()
            .filter(|o| !o.destroyed && o.kind == ObjectKind::Creature && o.scripts.contains_key("ScriptOnNotice"))
            .filter_map(|o| o.creature().filter(|c| !c.dead).map(|c| (o.id, o.position, c.perception_range)))
            .collect();
        let mut notices = Vec::new();
        for (id, pos, range) in observers {
            for &pid in &party {
                if pid == id {
                    continue;
                }
                let Some(ppos) = self.get(pid).map(|o| o.position) else { continue };
                let near = pos.distance(ppos) <= range && self.can_see(id, pid);
                let seen = self.get(id).and_then(|o| o.creature()).is_some_and(|c| c.seen.contains(&pid));
                if near && seen && self.is_enemy(id, pid) && self.renotice_due(id) {
                    notices.push((id, pid, true));
                    continue;
                }
                let Some(c) = self.get_mut(id).and_then(|o| o.creature_mut()) else { continue };
                if near && !seen {
                    c.seen.insert(pid);
                    notices.push((id, pid, true));
                } else if !near && seen {
                    c.seen.remove(&pid);
                    notices.push((id, pid, false));
                }
            }
        }
        for (id, pid, seen) in notices {
            self.queue_event(id, "ScriptOnNotice", ScriptCtx { last_perceived: pid, perception_seen: seen, ..Default::default() });
        }
        self.pump();
    }
}

/// ANIMATION_LOOPING_* values are below 100 (KotOR convention: looping
/// 0..99, fire-and-forget 100..199, placeable 200+).
pub fn is_looping_anim(anim: i32) -> bool {
    (0..100).contains(&anim) || (10000..10100).contains(&anim)
}
