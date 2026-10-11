//! GIT encounters: polygons that spawn creatures when hostile creatures
//! enter them. Format and semantics: docs/notes/world.md (Encounters).
//!
//! Each encounter is an object of kind `Encounter` whose detail is an
//! [`EncounterState`]. Every tick (`World::run_encounters`) it:
//!   1. fires OnEntered / OnExit for qualifying creatures crossing the polygon,
//!   2. fires OnHeartbeat every 6 s,
//!   3. prunes dead spawns and ends a spawn cycle when none are left: with
//!      Reset and cycles left the encounter goes inactive and re-arms after
//!      ResetTime; otherwise it goes inactive for good and fires OnExhausted,
//!   4. starts a spawn cycle (or, for SpawnOption 0, refills) while a hostile
//!      qualifying creature stands inside and the encounter is Active.

use std::collections::{HashMap, HashSet};
use std::f32::consts::TAU;

use glam::{Vec2, Vec3};
use k2_data::module::{point_in_polygon, EncounterInstance};
use k2_script::{ObjectId, Value, OBJECT_INVALID};
use serde::{Deserialize, Serialize};

use crate::factions::HOSTILE_MAX;
use crate::object::{bearing_towards, Detail, Object, ObjectKind};
use crate::{ScriptCtx, World, WorldEvent};

/// Seconds between OnHeartbeat runs (as for creatures and triggers).
const HEARTBEAT: f32 = 6.0;

/// Distances (metres) and directions tried when a spawn slot has no ground
/// under it.
const SLOT_NUDGE: [f32; 3] = [0.5, 1.0, 2.0];

/// One CreatureList entry, copied from the blueprint at spawn time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SpawnEntry {
    pub resref: String,
    pub cr: f32,
    pub single_spawn: bool,
    /// GuaranteedCount: copies spawned per cycle before the CR budget applies.
    pub guaranteed: i32,
    /// Guaranteed copies still owed in the running cycle (reset when a cycle begins).
    pub owed: i32,
    /// The blueprint could not be loaded; never spawned again.
    pub broken: bool,
}

/// Runtime state of one GIT encounter. Saved with the object, so a
/// spent encounter stays spent across save and load.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EncounterState {
    /// Active (GetEncounterActive). An inactive encounter fires its scripts
    /// but spawns nothing. An encounter with no cycles left can be set
    /// active, but it spawns nothing until SpawnsMax or SpawnsCurrent change.
    pub active: bool,
    /// encdifficulty.2da row.
    pub difficulty_index: i32,
    /// Only creatures hostile to this faction fire the encounter.
    pub faction: i32,
    pub max_creatures: i32,
    pub rec_creatures: i32,
    pub player_only: bool,
    pub reset: bool,
    pub reset_time: f32,
    /// SpawnOption 1: one cycle per trigger; 0: refill while occupied.
    pub single_shot: bool,
    /// World-space polygon (geometry plus position).
    pub polygon: Vec<[f32; 3]>,
    /// Absolute spawn points with orientation (radians, CCW from north).
    pub spawn_points: Vec<([f32; 3], f32)>,
    /// Spawn pool, sorted by ascending CR (the engine's order).
    pub creatures: Vec<SpawnEntry>,
    /// Spawn positions with the ground under them, computed on first use:
    /// (position, bearing). Bearing None faces the triggering creature.
    pub slots: Option<Vec<([f32; 3], Option<f32>)>>,
    /// Qualifying creatures inside the polygon at the last tick.
    pub inside: HashSet<ObjectId>,
    /// Living creatures this encounter spawned (pruned each tick).
    pub spawned: Vec<ObjectId>,
    /// Spawn cycles allowed (GetEncounterSpawnsMax): Respawns + 1, -1 = unlimited.
    pub spawns_max: i32,
    /// Spawn cycles begun (GetEncounterSpawnsCurrent).
    pub spawns_current: i32,
    /// A spawn cycle is running: its creatures are alive or being refilled.
    pub running: bool,
    /// Seconds until the encounter re-arms after a cycle; None = not waiting.
    /// The encounter is inactive while it waits. Deactivating it by script
    /// cancels the wait.
    pub reset_in: Option<f32>,
    /// OnExhausted fired and no cycle has started since.
    pub exhausted: bool,
    /// How many times OnExhausted has fired.
    pub exhaustions: u32,
}

impl EncounterState {
    /// Builds the runtime state from the UTE blueprint and the GIT instance.
    pub fn from_blueprint(bp: &k2_data::Encounter, inst: &EncounterInstance) -> Self {
        let p = inst.position;
        let mut creatures: Vec<SpawnEntry> = bp
            .creatures
            .iter()
            .map(|c| SpawnEntry {
                resref: c.resref.clone(),
                cr: c.cr,
                single_spawn: c.single_spawn,
                guaranteed: c.guaranteed_count,
                owed: 0,
                broken: false,
            })
            .collect();
        // The engine bubble-sorts the pool by ascending CR (rakata's UTE
        // page). The sort is stable, so file order holds among equal CRs.
        creatures.sort_by(|a, b| a.cr.total_cmp(&b.cr));
        Self {
            active: bp.active,
            difficulty_index: bp.difficulty_index,
            faction: bp.faction as i32,
            max_creatures: bp.max_creatures,
            rec_creatures: bp.rec_creatures,
            player_only: bp.player_only,
            reset: bp.reset,
            reset_time: bp.reset_time as f32,
            single_shot: bp.spawn_option != 0,
            polygon: inst.geometry.iter().map(|g| [g[0] + p[0], g[1] + p[1], g[2] + p[2]]).collect(),
            spawn_points: inst.spawn_points.clone(),
            creatures,
            spawns_max: if bp.respawns < 0 { -1 } else { bp.respawns + 1 },
            ..Default::default()
        }
    }

    /// Creatures alive at once that the encounter aims for: RecCreatures,
    /// never more than MaxCreatures.
    pub fn target(&self) -> usize {
        self.rec_creatures.clamp(1, self.cap() as i32) as usize
    }

    /// Hard limit on creatures alive from the encounter at once.
    pub fn cap(&self) -> usize {
        self.max_creatures.max(1) as usize
    }

    pub fn may_spawn_more(&self) -> bool {
        self.spawns_max < 0 || self.spawns_current < self.spawns_max
    }

    /// Next pool entry to spawn, as (index, is a guaranteed copy): the first
    /// owed guaranteed copy, else (while below the target) the first entry in
    /// CR order that fits the level. SingleSpawn entries already alive, and
    /// broken blueprints, are skipped.
    fn next_spawn(&self, alive: &[String], level: f32, below_target: bool) -> Option<(usize, bool)> {
        let blocked = |c: &SpawnEntry| c.broken || (c.single_spawn && alive.iter().any(|t| *t == c.resref));
        if let Some(i) = self.creatures.iter().position(|c| c.owed > 0 && !blocked(c)) {
            return Some((i, true));
        }
        if !below_target {
            return None;
        }
        self.creatures.iter().position(|c| !blocked(c) && c.cr <= level).map(|i| (i, false))
    }
}

/// A living creature as the encounter pass sees it this tick.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    id: ObjectId,
    pos: Vec3,
    /// Party member or PC (what PlayerOnly admits).
    pc: bool,
    spawned_by: Option<ObjectId>,
}

fn flat(v: Vec3) -> f32 {
    Vec2::new(v.x, v.y).length()
}

impl World {
    pub fn encounter(&self, id: ObjectId) -> Option<&EncounterState> {
        self.get(id).and_then(|o| o.encounter())
    }

    pub(crate) fn encounter_mut(&mut self, id: ObjectId) -> Option<&mut EncounterState> {
        self.get_mut(id).and_then(|o| o.encounter_mut())
    }

    /// SetEncounterActive. A script's setting replaces a re-arm wait in
    /// progress: the wait would have switched the encounter on at its end, so
    /// it is cancelled. Setting Active on an encounter with no cycles left
    /// changes the flag only; it spawns nothing until SpawnsMax or
    /// SpawnsCurrent change.
    pub fn set_encounter_active(&mut self, id: ObjectId, active: bool) {
        if let Some(st) = self.encounter_mut(id) {
            st.active = active;
            st.reset_in = None;
        }
    }

    /// Fills an encounter object from its UTE blueprint and GIT instance.
    pub(crate) fn fill_encounter(&self, o: &mut Object, inst: &EncounterInstance) -> Result<(), k2_data::DataError> {
        let bp = k2_data::Encounter::load(&self.res, &inst.template)?;
        o.tag = bp.tag.clone();
        o.scripts = bp.scripts.iter().cloned().collect::<HashMap<_, _>>();
        o.detail = Detail::Encounter(Box::new(EncounterState::from_blueprint(&bp, inst)));
        Ok(())
    }

    /// Objects saved before encounters had state (Detail::None) get it back
    /// from the module's GIT, matched by template and position. Each GIT
    /// instance is used once, so encounters that share a template (506ond's
    /// ls_vaklu_enc, for one) each get their own.
    pub(crate) fn rebuild_encounter_details(&mut self, module: &k2_data::Module) {
        let stale: Vec<ObjectId> = self
            .objects
            .values()
            .filter(|o| o.kind == ObjectKind::Encounter && matches!(o.detail, Detail::None))
            .map(|o| o.id)
            .collect();
        let mut used = vec![false; module.git.encounters.len()];
        for id in stale {
            let Some(o) = self.objects.get(&id) else { continue };
            let (template, position) = (o.template.clone(), o.position);
            let found = module.git.encounters.iter().enumerate().find(|(i, e)| {
                !used[*i] && e.template.eq_ignore_ascii_case(&template) && Vec3::from(e.position).distance(position) < 0.01
            });
            let Some((i, inst)) = found.map(|(i, e)| (i, e.clone())) else {
                self.warnings.push(format!("encounter {template}: no GIT instance to rebuild from"));
                continue;
            };
            used[i] = true;
            let Some(mut o) = self.objects.remove(&id) else { continue };
            if let Err(e) = self.fill_encounter(&mut o, &inst) {
                self.warnings.push(format!("encounter {template}: {e}"));
            }
            self.objects.insert(id, o);
        }
    }

    /// Living, standing creatures, once per tick, for every encounter to test
    /// against. Dead and knocked-down creatures do not count: neither entering
    /// a polygon nor standing inside it.
    fn encounter_candidates(&self) -> Vec<Candidate> {
        self.iter()
            .filter(|o| o.kind == ObjectKind::Creature)
            .filter_map(|o| {
                let c = o.creature().filter(|c| !c.dead && !c.down)?;
                Some(Candidate {
                    id: o.id,
                    pos: o.position,
                    pc: c.is_pc || self.party.contains(&o.id),
                    spawned_by: c.encounter,
                })
            })
            .collect()
    }

    /// Advances every encounter by `dt` seconds. Scripts fired here run
    /// immediately, so a script can change the encounter it belongs to.
    /// Idle cost: one pass over the objects for the encounters and one over
    /// the creatures, shared by all encounters.
    pub(crate) fn run_encounters(&mut self, dt: f32) {
        let ids: Vec<ObjectId> = self.iter().filter(|o| o.encounter().is_some()).map(|o| o.id).collect();
        if ids.is_empty() {
            return;
        }
        self.ensure_factions();
        let candidates = self.encounter_candidates();
        for id in ids {
            if self.is_valid(id) {
                self.tick_encounter(id, dt, &candidates);
            }
        }
        self.pump();
    }

    fn tick_encounter(&mut self, id: ObjectId, dt: f32, candidates: &[Candidate]) {
        let people = self.encounter_people(id, candidates);
        let (entered, exited) = self.encounter_crossings(id, &people);
        for cid in exited {
            self.fire(id, "OnExit", ScriptCtx { exiting: cid, ..Default::default() });
        }
        for cid in entered {
            self.fire(id, "OnEntered", ScriptCtx { entering: cid, clicking: cid, ..Default::default() });
        }
        if !self.is_valid(id) {
            return;
        }
        self.encounter_heartbeat(id, dt);
        self.prune_encounter(id);
        self.finish_encounter_cycle(id);
        self.count_encounter_reset(id, dt);
        let faction = self.encounter(id).map_or(0, |e| e.faction);
        if let Some(near) = people.into_iter().find(|&p| self.encounter_hostile(p, faction)) {
            self.encounter_spawn(id, near);
        }
    }

    /// Creatures inside the polygon that may fire this encounter (party
    /// members and PCs only when PlayerOnly; never its own spawns).
    fn encounter_people(&self, id: ObjectId, candidates: &[Candidate]) -> Vec<ObjectId> {
        let Some(st) = self.encounter(id) else { return Vec::new() };
        let (lo, hi) = st.polygon.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(lo, hi), p| {
            (lo.min(Vec2::new(p[0], p[1])), hi.max(Vec2::new(p[0], p[1])))
        });
        candidates
            .iter()
            .filter(|c| c.spawned_by != Some(id) && (!st.player_only || c.pc))
            .filter(|c| c.pos.x >= lo.x && c.pos.x <= hi.x && c.pos.y >= lo.y && c.pos.y <= hi.y)
            .filter(|c| point_in_polygon(&st.polygon, c.pos.x, c.pos.y))
            .map(|c| c.id)
            .collect()
    }

    /// Updates the inside set; returns (entered, exited), each sorted by id.
    fn encounter_crossings(&mut self, id: ObjectId, people: &[ObjectId]) -> (Vec<ObjectId>, Vec<ObjectId>) {
        let now: HashSet<ObjectId> = people.iter().copied().collect();
        let Some(st) = self.encounter_mut(id) else { return (Vec::new(), Vec::new()) };
        let mut entered: Vec<ObjectId> = now.difference(&st.inside).copied().collect();
        let mut exited: Vec<ObjectId> = st.inside.difference(&now).copied().collect();
        entered.sort_unstable();
        exited.sort_unstable();
        st.inside = now;
        (entered, exited)
    }

    /// Revisit: the qualifying creatures standing inside an encounter count as
    /// already inside, so arriving does not fire OnEntered again (triggers do
    /// the same for the party).
    pub(crate) fn prime_encounters(&mut self) {
        let candidates = self.encounter_candidates();
        let ids: Vec<ObjectId> = self.iter().filter(|o| o.encounter().is_some()).map(|o| o.id).collect();
        for id in ids {
            let people = self.encounter_people(id, &candidates);
            if let Some(st) = self.encounter_mut(id) {
                st.inside.extend(people);
            }
        }
    }

    fn encounter_hostile(&self, creature: ObjectId, faction: i32) -> bool {
        self.combat.factions.reputation(self.faction_of(creature), faction) <= HOSTILE_MAX
    }

    fn encounter_heartbeat(&mut self, id: ObjectId, dt: f32) {
        let beat = match self.objects.get_mut(&id) {
            Some(o) => {
                o.heartbeat_in -= dt;
                let due = o.heartbeat_in <= 0.0;
                if due {
                    o.heartbeat_in += HEARTBEAT;
                }
                due
            }
            None => false,
        };
        if beat {
            self.fire(id, "OnHeartbeat", ScriptCtx::default());
        }
    }

    /// Forgets spawns that died or were destroyed.
    fn prune_encounter(&mut self, id: ObjectId) {
        let Some(st) = self.encounter(id) else { return };
        let alive: Vec<ObjectId> = st
            .spawned
            .iter()
            .copied()
            .filter(|&c| self.get(c).and_then(|o| o.creature()).is_some_and(|c| !c.dead))
            .collect();
        if let Some(st) = self.encounter_mut(id) {
            st.spawned = alive;
        }
    }

    /// A cycle whose creatures are all gone ends. With Reset and cycles left,
    /// the encounter goes inactive and re-arms after ResetTime (the wait
    /// counts down in count_encounter_reset). Otherwise it is exhausted:
    /// inactive, and OnExhausted fires. Continuous encounters without Reset
    /// are exhausted here (format document: Reset 0 means no respawn).
    fn finish_encounter_cycle(&mut self, id: ObjectId) {
        let exhausted = match self.encounter_mut(id) {
            Some(st) if st.running && st.spawned.is_empty() => {
                st.running = false;
                st.active = false;
                if st.reset && st.may_spawn_more() {
                    st.reset_in = Some(st.reset_time.max(0.0));
                    false
                } else {
                    st.exhausted = true;
                    st.exhaustions += 1;
                    true
                }
            }
            _ => false,
        };
        if exhausted {
            self.fire(id, "OnExhausted", ScriptCtx::default());
        }
    }

    /// Counts down the re-arm wait; the encounter goes active again when it ends.
    fn count_encounter_reset(&mut self, id: ObjectId, dt: f32) {
        if let Some(st) = self.encounter_mut(id)
            && let Some(t) = st.reset_in
        {
            let t = t - dt;
            if t > 0.0 {
                st.reset_in = Some(t);
            } else {
                st.reset_in = None;
                st.active = true;
            }
        }
    }

    /// Starts a spawn cycle, or for a continuous encounter in a running cycle
    /// refills it, while `near` stands inside and fires it.
    fn encounter_spawn(&mut self, id: ObjectId, near: ObjectId) {
        let Some(st) = self.encounter(id) else { return };
        if !st.active {
            return;
        }
        if st.running {
            // Single shot waits for its creatures to die; continuous refills.
            if !st.single_shot {
                self.encounter_fill(id, near);
            }
            return;
        }
        if st.reset_in.is_some() || !st.may_spawn_more() {
            return;
        }
        if let Some(st) = self.encounter_mut(id) {
            for c in &mut st.creatures {
                c.owed = c.guaranteed;
            }
        }
        if self.encounter_fill(id, near) > 0
            && let Some(st) = self.encounter_mut(id)
        {
            st.running = true;
            st.exhausted = false;
            st.spawns_current += 1;
        }
    }

    /// Creature level of the firing party (average class levels, at least 1)
    /// plus the encdifficulty.2da VALUE of the encounter's difficulty.
    fn encounter_level(&self, difficulty: i32) -> f32 {
        let levels: Vec<i32> = self
            .party
            .iter()
            .filter_map(|&id| self.get(id)?.creature().map(|c| c.classes.iter().map(|&(_, l)| l).sum()))
            .collect();
        let average = if levels.is_empty() { 1 } else { levels.iter().sum::<i32>() / levels.len() as i32 };
        let value = self
            .tables
            .get(&self.res, "encdifficulty")
            .ok()
            .and_then(|t| t.int(difficulty.max(0) as usize, "value"))
            .unwrap_or(0);
        (average.max(1) + value) as f32
    }

    /// The point itself if it has ground, else the first ground found within
    /// SLOT_NUDGE of it.
    fn ground_point(&self, p: Vec3) -> Option<Vec3> {
        let at = |q: Vec3| self.walk.ground(q.x, q.y, q.z + 1.0).map(|h| Vec3::new(q.x, q.y, h.z));
        at(p).or_else(|| {
            SLOT_NUDGE
                .iter()
                .flat_map(|&r| (0..8).map(move |k| p + Vec3::new((k as f32 * TAU / 8.0).cos(), (k as f32 * TAU / 8.0).sin(), 0.0) * r))
                .find_map(at)
        })
    }

    /// Computes and caches the spawn slots of an encounter on first use:
    /// its spawn points, or the polygon vertices without any. A slot with no
    /// walkable ground within 2 m is skipped (logged once, when the cache is
    /// built).
    fn ensure_slots(&mut self, id: ObjectId) {
        let Some(st) = self.encounter(id) else { return };
        if st.slots.is_some() {
            return;
        }
        let raw: Vec<(Vec3, Option<f32>)> = if st.spawn_points.is_empty() {
            st.polygon.iter().map(|p| (Vec3::from(*p), None)).collect()
        } else {
            st.spawn_points.iter().map(|(p, b)| (Vec3::from(*p), Some(*b))).collect()
        };
        let mut slots = Vec::new();
        let mut skipped = 0;
        for (p, bearing) in raw {
            match self.ground_point(p) {
                Some(g) => slots.push((g.to_array(), bearing)),
                None => skipped += 1,
            }
        }
        let tag = self.get(id).map(|o| o.template.clone()).unwrap_or_default();
        if skipped > 0 {
            self.warnings.push(format!("encounter {tag}: {skipped} spawn slot(s) have no walkable ground within 2 m; skipped"));
        }
        if let Some(st) = self.encounter_mut(id) {
            st.slots = Some(slots);
        }
    }

    /// Spawns creatures until the encounter has its target alive (or
    /// MaxCreatures). Guaranteed copies owed in this cycle come first and
    /// skip the CR budget; the rest must have CR at or below the encounter
    /// level, taken from the front of the CR-sorted pool. Returns the number
    /// spawned.
    fn encounter_fill(&mut self, id: ObjectId, near: ObjectId) -> usize {
        self.ensure_slots(id);
        let Some(st) = self.encounter(id) else { return 0 };
        if st.creatures.is_empty() || st.spawned.len() >= st.cap() {
            return 0;
        }
        let (difficulty, target, cap) = (st.difficulty_index, st.target(), st.cap());
        let level = self.encounter_level(difficulty);
        let from = self.get(near).map_or(Vec3::ZERO, |o| o.position);
        let mut slots: Vec<(Vec3, Option<f32>)> = self
            .encounter(id)
            .and_then(|st| st.slots.as_ref())
            .map(|s| s.iter().map(|(p, b)| (Vec3::from(*p), *b)).collect())
            .unwrap_or_default();
        if slots.is_empty() {
            return 0;
        }
        // Furthest from the creature that fired the encounter first. A community
        // wiki note gives this order (guess: not in the format document).
        slots.sort_by(|a, b| flat(b.0 - from).total_cmp(&flat(a.0 - from)));
        let mut alive: Vec<String> = self
            .encounter(id)
            .map(|st| st.spawned.iter().filter_map(|&c| self.get(c).map(|o| o.template.clone())).collect())
            .unwrap_or_default();
        let mut made: Vec<ObjectId> = Vec::new();
        loop {
            let (picked, spawned_so_far) = match self.encounter(id) {
                Some(st) => (st.next_spawn(&alive, level, st.spawned.len() + made.len() < target), st.spawned.len() + made.len()),
                None => break,
            };
            if spawned_so_far >= cap {
                break;
            }
            let Some((i, owed)) = picked else { break };
            let Some(resref) = self.encounter(id).map(|st| st.creatures[i].resref.clone()) else { break };
            if owed && let Some(st) = self.encounter_mut(id) {
                st.creatures[i].owed -= 1;
            }
            let (pos, bearing) = slots[made.len() % slots.len()];
            let bearing = bearing.unwrap_or_else(|| bearing_towards(pos, from).unwrap_or(0.0));
            match self.spawn_creature(&resref, pos, bearing) {
                Ok(cid) => {
                    if let Some(c) = self.get_mut(cid).and_then(|o| o.creature_mut()) {
                        c.encounter = Some(id);
                    }
                    alive.push(resref);
                    made.push(cid);
                }
                Err(e) => {
                    let tag = self.get(id).map(|o| o.template.clone()).unwrap_or_default();
                    self.warnings.push(format!("encounter {tag}: creature {resref}: {e}"));
                    if let Some(st) = self.encounter_mut(id)
                        && let Some(c) = st.creatures.get_mut(i)
                    {
                        c.broken = true;
                    }
                }
            }
        }
        for &cid in &made {
            self.emit(WorldEvent::ObjectSpawned(cid));
            self.queue_event(cid, "ScriptSpawn", ScriptCtx::default());
        }
        if let Some(st) = self.encounter_mut(id) {
            st.spawned.extend(made.iter().copied());
        }
        made.len()
    }

    fn encounter_read(&self, id: ObjectId, f: impl FnOnce(&EncounterState) -> i32) -> i32 {
        self.encounter(id).map_or(0, f)
    }

    fn encounter_write(&mut self, id: ObjectId, f: impl FnOnce(&mut EncounterState)) {
        if let Some(st) = self.encounter_mut(id) {
            f(st);
        }
    }

    /// Encounter routines (nwscript 276-281, 296-297, 409).
    pub(crate) fn encounter_call(&mut self, name: &str, _me: ObjectId, a: &[Value]) -> Option<Value> {
        let obj = |n: usize| match a.get(n) {
            Some(Value::Object(o)) => *o,
            _ => OBJECT_INVALID,
        };
        let int = |n: usize| match a.get(n) {
            Some(Value::Int(i)) => *i,
            _ => 0,
        };
        let v = match name {
            "GetEncounterActive" => Value::Int(self.encounter_read(obj(0), |e| i32::from(e.active))),
            "SetEncounterActive" => {
                self.set_encounter_active(obj(1), int(0) != 0);
                Value::Void
            }
            "GetEncounterSpawnsMax" => Value::Int(self.encounter_read(obj(0), |e| e.spawns_max)),
            "SetEncounterSpawnsMax" => {
                self.encounter_write(obj(1), |e| e.spawns_max = int(0));
                Value::Void
            }
            "GetEncounterSpawnsCurrent" => Value::Int(self.encounter_read(obj(0), |e| e.spawns_current)),
            "SetEncounterSpawnsCurrent" => {
                self.encounter_write(obj(1), |e| e.spawns_current = int(0));
                Value::Void
            }
            "GetEncounterDifficulty" => Value::Int(self.encounter_read(obj(0), |e| e.difficulty_index)),
            "SetEncounterDifficulty" => {
                self.encounter_write(obj(1), |e| e.difficulty_index = int(0));
                Value::Void
            }
            "GetIsEncounterCreature" => {
                Value::Int(i32::from(self.get(obj(0)).and_then(|o| o.creature()).is_some_and(|c| c.encounter.is_some())))
            }
            _ => return None,
        };
        Some(v)
    }
}
