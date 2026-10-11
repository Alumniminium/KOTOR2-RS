//! Save games and module memory.
//!
//! [`World::save`] captures everything needed to resume exactly: objects,
//! globals, party, journal, timers with their deferred script states, the
//! engine-structure store, RNG states and the per-subsystem state (combat,
//! paths, crowd, stores, crafting). Derived data (resources, tables, compiled
//! scripts, walkmesh, item definitions) is rebuilt from the install on
//! [`World::load`]. Deferred script states store their script by name and are
//! re-linked to the compiled script on load (k2_script::saved).
//!
//! Module memory: when the party leaves a module its objects are kept in
//! [`World::module_memory`]; re-entering restores them instead of spawning
//! the GIT again, so the dead stay dead and opened doors stay open. OnSpawn
//! does not run again; OnModLoad, OnClientEnter and the area OnEnter do.
//!
//! On disk: one directory per slot holding `save.bin` ([`SaveData`]: magic,
//! format version, module name, then the world state as CBOR),
//! `meta.json` ([`SaveMeta`]) and, from the game, `thumb.png`.
//!
//! CBOR keys struct fields by name, so a save survives added fields (they
//! take their default: every saved struct carries `#[serde(default)]` or
//! the new field does) and removed fields (unknown keys are ignored).
//! Renamed or removed enum variants still break saves that contain them.
//! See docs/notes/saves.md.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::Hasher;
use std::path::{Path, PathBuf};

use k2_data::Module;
use k2_res::Install;
use k2_script::{Ncs, ObjectId};
use serde::{Deserialize, Serialize};

use crate::engine::{EngineStore, Location};
use crate::object::{Detail, Object, ObjectKind};
use crate::store::StoreInfo;
use crate::{AvailableNpc, PlayerSpec, Timer, World, WorldError, WorldEvent};

/// Container format version. Bump only for a change that the field-keyed
/// encoding cannot absorb (a new container layout, renamed enum variants).
/// Version 2 was positional postcard; those saves are refused.
pub const SAVE_VERSION: u32 = 3;

/// First bytes of every save.bin since format 3.
pub const SAVE_MAGIC: &[u8; 8] = b"K2RSAVE\0";

/// A saved game: the module to mount, then the world state.
#[derive(Debug, Clone)]
pub struct SaveData {
    pub version: u32,
    /// Current module resref.
    pub module: String,
    /// CBOR-encoded world state.
    pub state: Vec<u8>,
}

impl SaveData {
    /// magic, u32 LE format version, u32 LE module length, module bytes,
    /// CBOR state.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.module.len() + self.state.len());
        out.extend_from_slice(SAVE_MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&(self.module.len() as u32).to_le_bytes());
        out.extend_from_slice(self.module.as_bytes());
        out.extend_from_slice(&self.state);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Self> {
        check_header(bytes)?;
        let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let version = u32_at(8).unwrap_or_default();
        let len = u32_at(12).ok_or_else(|| WorldError::Other("save: truncated header".into()))? as usize;
        let module = bytes
            .get(16..16 + len)
            .and_then(|m| std::str::from_utf8(m).ok())
            .ok_or_else(|| WorldError::Other("save: bad module name".into()))?
            .to_owned();
        Ok(SaveData { version, module, state: bytes[16 + len..].to_vec() })
    }
}

/// Checks the magic and format version without decoding the state. Saves
/// from before format 3 (positional postcard) have no magic.
pub fn check_header(bytes: &[u8]) -> crate::Result<()> {
    if bytes.len() < 16 || &bytes[..8] != SAVE_MAGIC {
        return Err(WorldError::IncompatibleSave("saved by an older incompatible build".into()));
    }
    let version = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    if version != SAVE_VERSION {
        let why = if version > SAVE_VERSION { "saved by a newer build" } else { "saved by an older incompatible build" };
        return Err(WorldError::IncompatibleSave(format!("{why} (format {version}, this build reads {SAVE_VERSION})")));
    }
    Ok(())
}

/// Whether a slot's save.bin can be loaded by this build (header check
/// only; reads the first 16 bytes).
pub fn slot_compatibility(dir: &Path) -> crate::Result<()> {
    use std::io::Read;
    let mut head = [0u8; 16];
    let mut f = std::fs::File::open(dir.join("save.bin")).map_err(|e| WorldError::Other(format!("{e}")))?;
    let n = f.read(&mut head).map_err(|e| WorldError::Other(format!("{e}")))?;
    check_header(&head[..n])
}

fn encode_cbor<T: Serialize>(value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).expect("world state encoding");
    out
}

/// Human-readable slot description (meta.json).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveMeta {
    pub name: String,
    pub module: String,
    pub area_name: String,
    /// Game time in seconds.
    pub play_time: f64,
    /// Unix seconds when saved.
    pub timestamp: u64,
    /// Party members, leader first: (name, appearance row).
    pub party: Vec<(String, u32)>,
}

/// One module's objects as the party left them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleSnapshot {
    pub module_id: ObjectId,
    pub area_id: ObjectId,
    pub objects: Vec<Object>,
    pub stores: Vec<(ObjectId, StoreInfo)>,
    pub keep_corpse: Vec<ObjectId>,
    pub ai_style: Vec<(ObjectId, i32)>,
    /// World time when the party left.
    pub left_at: f64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct SavedGlobals {
    numbers: BTreeMap<String, i32>,
    booleans: BTreeMap<String, bool>,
    strings: BTreeMap<String, String>,
    locations: BTreeMap<String, Location>,
}

/// Serialized by reference (no clone on save). Field names and types must
/// match [`WorldState`]; order does not matter (fields are keyed by name).
#[derive(Serialize)]
struct WorldStateRef<'a> {
    objects: &'a BTreeMap<ObjectId, Object>,
    next_id: ObjectId,
    module_id: ObjectId,
    area_id: ObjectId,
    player: ObjectId,
    party: &'a Vec<ObjectId>,
    available_npcs: &'a HashMap<i32, AvailableNpc>,
    globals: SavedGlobals,
    journal: &'a HashMap<String, i32>,
    journal_updated: &'a HashMap<String, f64>,
    tokens: &'a HashMap<i32, String>,
    influence: &'a HashMap<i32, i32>,
    time: f64,
    timers: &'a Vec<Timer>,
    engine: &'a EngineStore,
    rng: u64,
    player_spec: &'a PlayerSpec,
    perception_in: f32,
    input_blocked_for: f32,
    solo_mode: bool,
    area_unescapable_override: Option<bool>,
    transit_disabled: bool,
    last_pazaak: i32,
    stashed_pc: &'a Option<Object>,
    planets: &'a HashMap<i32, (bool, bool)>,
    selected_planet: i32,
    combat: &'a crate::combat::CombatSystem,
    paths: &'a crate::nav::Paths,
    crowd: &'a crate::crowd::Crowd,
    inventory: &'a crate::inventory::Inventories,
    craft: &'a crate::craft::CraftState,
    progression: &'a crate::progression::ProgressionState,
    powers: &'a crate::powers::PowersState,
    module_memory: &'a HashMap<String, ModuleSnapshot>,
    systems: &'a crate::traps::SystemsState,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WorldState {
    objects: BTreeMap<ObjectId, Object>,
    next_id: ObjectId,
    module_id: ObjectId,
    area_id: ObjectId,
    player: ObjectId,
    party: Vec<ObjectId>,
    available_npcs: HashMap<i32, AvailableNpc>,
    globals: SavedGlobals,
    journal: HashMap<String, i32>,
    journal_updated: HashMap<String, f64>,
    tokens: HashMap<i32, String>,
    influence: HashMap<i32, i32>,
    time: f64,
    timers: Vec<Timer>,
    engine: EngineStore,
    rng: u64,
    player_spec: PlayerSpec,
    perception_in: f32,
    input_blocked_for: f32,
    solo_mode: bool,
    area_unescapable_override: Option<bool>,
    /// Saves before the override existed stored the script flag as a bool
    /// (only ever true when a script had set it).
    area_unescapable: bool,
    transit_disabled: bool,
    last_pazaak: i32,
    stashed_pc: Option<Object>,
    planets: HashMap<i32, (bool, bool)>,
    selected_planet: i32,
    combat: crate::combat::CombatSystem,
    paths: crate::nav::Paths,
    crowd: crate::crowd::Crowd,
    inventory: crate::inventory::Inventories,
    craft: crate::craft::CraftState,
    progression: crate::progression::ProgressionState,
    powers: crate::powers::PowersState,
    module_memory: HashMap<String, ModuleSnapshot>,
    systems: crate::traps::SystemsState,
}

impl World {
    /// Why saving is not allowed right now, if it is not. Refused during
    /// conversations, area changes, minigames and open menus. Combat does
    /// not block it: rounds, attack queues, AI timers and the combat RNG
    /// are saved and resume exactly (tests/save.rs).
    pub fn save_blocked(&self) -> Option<&'static str> {
        self.save_blocked_with(false)
    }

    /// For the save made when the game exits: like [`World::save_blocked`]
    /// except that a conversation does not block. Conversations are not part
    /// of a save, so the loaded world resumes with the conversation gone
    /// (better than losing everything since the last save).
    pub fn exit_save_blocked(&self) -> Option<&'static str> {
        self.save_blocked_with(true)
    }

    fn save_blocked_with(&self, allow_conversation: bool) -> Option<&'static str> {
        if self.module.is_none() {
            return Some("no module loaded");
        }
        // A dead leader or a fully knocked-out party means the party is down
        // (a standing member takes over otherwise): saving now would write
        // the game over state into the slot the player reloads from.
        if self.party_defeated() || self.get(self.party_leader()).and_then(|o| o.creature()).is_some_and(|c| c.dead) {
            return Some("the party is dead");
        }
        if self.conversation.is_some() && !allow_conversation {
            return Some("in conversation");
        }
        if self.pending_transition.is_some() {
            return Some("changing area");
        }
        if self.minigame.is_some() {
            return Some("in a minigame");
        }
        if self.pazaak.is_some() {
            return Some("playing pazaak");
        }
        if !self.pending.is_empty() || !self.ctx.is_empty() {
            return Some("scripts running");
        }
        if self.craft.screen.is_some() || self.inventory.open_container.is_some() || self.inventory.open_store.is_some() {
            // A container opened under a talk is held by the client until the talk
            // ends (gui.md): no window is shown, so the reason is the talk.
            if self.conversation.is_some() {
                return Some("in conversation");
            }
            return Some("a menu is open");
        }
        None
    }

    /// Captures the whole game state. Check [`World::save_blocked`] first;
    /// this saves regardless (tests save mid-fight).
    pub fn save(&self) -> SaveData {
        let module = self.module.as_ref().map(|m| m.name.clone()).unwrap_or_default();
        let st = WorldStateRef {
            objects: &self.objects,
            next_id: self.next_id,
            module_id: self.module_id,
            area_id: self.area_id,
            player: self.player,
            party: &self.party,
            available_npcs: &self.available_npcs,
            globals: SavedGlobals {
                numbers: self.globals.numbers.clone(),
                booleans: self.globals.booleans.clone(),
                strings: self.globals.strings.clone(),
                locations: self.globals.locations.clone(),
            },
            journal: &self.journal,
            journal_updated: &self.sheet.journal_updated,
            tokens: &self.tokens,
            influence: &self.influence,
            time: self.time,
            timers: &self.timers,
            engine: &self.engine,
            rng: self.builtins.rng_state(),
            player_spec: &self.player_spec,
            perception_in: self.perception_in,
            input_blocked_for: self.input_blocked_for,
            solo_mode: self.solo_mode,
            area_unescapable_override: self.area_unescapable_override,
            transit_disabled: self.transit_disabled,
            last_pazaak: self.last_pazaak,
            stashed_pc: &self.stashed_pc,
            planets: &self.planets,
            selected_planet: self.selected_planet,
            combat: &self.combat,
            paths: &self.paths,
            crowd: &self.crowd,
            inventory: &self.inventory,
            craft: &self.craft,
            progression: &self.progression,
            powers: &self.powers,
            module_memory: &self.module_memory,
            systems: &self.systems,
        };
        SaveData { version: SAVE_VERSION, module, state: encode_cbor(&st) }
    }

    /// Rebuilds a world from a save: mounts the module, restores the state,
    /// rebuilds the walkmesh. No scripts run; the presentation layer gets
    /// ModuleEntered and ObjectSpawned events as after a module load.
    pub fn load(install: Install, data: &SaveData) -> crate::Result<World> {
        let mut w = World::new(install)?;
        let module = Module::load(&mut w.res, &data.module)?;
        let st = w.decode_state(&data.state)?;

        w.objects = st.objects;
        // A save from before DoorState::locked_at_load: the lock state at load is the
        // template's Locked field, read as spawn.rs reads it.
        let unfilled: Vec<(ObjectId, String)> = w
            .objects
            .values()
            .filter_map(|o| match &o.detail {
                Detail::Door(d) if d.locked_at_load.is_none() => Some((o.id, o.template.clone())),
                _ => None,
            })
            .collect();
        for (id, template) in unfilled {
            let locked = k2_data::blueprint::Door::load(&w.res, &template).is_ok_and(|bp| bp.locked);
            if let Some(d) = w.get_mut(id).and_then(|o| o.door_mut()) {
                d.locked_at_load = Some(locked);
            }
        }
        w.next_id = st.next_id;
        w.module_id = st.module_id;
        w.area_id = st.area_id;
        w.player = st.player;
        w.party = st.party;
        w.available_npcs = st.available_npcs;
        w.globals.numbers = st.globals.numbers;
        w.globals.booleans = st.globals.booleans;
        w.globals.strings = st.globals.strings;
        w.globals.locations = st.globals.locations;
        w.journal = st.journal;
        w.sheet.journal_updated = st.journal_updated;
        w.tokens = st.tokens;
        w.influence = st.influence;
        w.time = st.time;
        w.timers = st.timers;
        w.engine = st.engine;
        w.builtins.set_rng_state(st.rng);
        w.player_spec = st.player_spec;
        w.perception_in = st.perception_in;
        w.input_blocked_for = st.input_blocked_for;
        w.solo_mode = st.solo_mode;
        w.area_unescapable_override = st.area_unescapable_override.or(st.area_unescapable.then_some(true));
        w.transit_disabled = st.transit_disabled;
        w.last_pazaak = st.last_pazaak;
        w.stashed_pc = st.stashed_pc;
        w.planets = st.planets;
        w.selected_planet = st.selected_planet;
        w.combat = st.combat;
        w.paths = st.paths;
        w.crowd = st.crowd;
        w.inventory = st.inventory;
        w.craft = st.craft;
        w.progression = st.progression;
        w.powers = st.powers;
        w.module_memory = st.module_memory;
        w.systems = st.systems;
        w.settle_loaded_party();

        w.rebuild_encounter_details(&module);
        w.refresh_all_bodies();
        w.repair_all_weapon_slots();
        w.build_walkmesh(&module);
        w.attach_object_walks();
        let name = module.name.clone();
        w.module = Some(module);
        w.walk.prepare_nav();
        w.emit(WorldEvent::ModuleEntered { module: name });
        let all: Vec<ObjectId> = w.iter().map(|o| o.id).collect();
        for id in all {
            w.emit(WorldEvent::ObjectSpawned(id));
        }
        Ok(w)
    }

    /// Decodes the state, linking saved script states to compiled scripts.
    /// Names not cached yet are loaded and the state is decoded once more.
    fn decode_state(&mut self, bytes: &[u8]) -> crate::Result<WorldState> {
        let decode = |w: &World| {
            let table: HashMap<String, std::sync::Arc<Ncs>> =
                w.scripts.iter().filter_map(|(k, v)| v.clone().map(|s| (k.clone(), s))).collect();
            k2_script::saved::with_scripts(table, || ciborium::from_reader::<WorldState, _>(bytes))
        };
        let (first, missing) = decode(self);
        if missing.is_empty() {
            return first.map_err(|e| WorldError::Other(format!("save state: {e}")));
        }
        for name in &missing {
            if self.load_script(name).is_none() {
                self.warnings.push(format!("save: script {name} not found; its deferred actions will not run"));
            }
        }
        let (second, _) = decode(self);
        second.map_err(|e| WorldError::Other(format!("save state: {e}")))
    }

    /// Slot description for meta.json.
    pub fn save_meta(&self, name: &str) -> SaveMeta {
        let party = self
            .party
            .iter()
            .filter_map(|&id| self.get(id))
            .map(|o| (o.name.clone(), o.creature().map_or(0, |c| c.appearance)))
            .collect();
        SaveMeta {
            name: name.to_owned(),
            module: self.module.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
            area_name: self.get(self.area_id).map(|a| a.name.clone()).unwrap_or_default(),
            play_time: self.time,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            party,
        }
    }

    // ---------------------------------------------------------------
    // Module memory
    // ---------------------------------------------------------------

    /// Keeps the current module's objects (everything but the party and the
    /// party's items, which the caller has already taken out) for re-entry.
    pub(crate) fn snapshot_module(&mut self) {
        let Some(name) = self.module.as_ref().map(|m| m.name.to_ascii_lowercase()) else { return };
        let mut objects = Vec::new();
        for o in self.objects.values().filter(|o| !o.destroyed) {
            let mut o = o.clone();
            // Queued actions and trigger occupancy belong to the visit that
            // ended; re-entry starts clean (deferred script states could also
            // point at this module's scripts, which will not be mounted).
            o.actions.clear();
            if let Detail::Trigger(t) = &mut o.detail {
                t.inside.clear();
            }
            if let Detail::Encounter(e) = &mut o.detail {
                e.inside.clear();
            }
            objects.push(o);
        }
        let ids: HashSet<ObjectId> = objects.iter().map(|o| o.id).collect();
        let snap = ModuleSnapshot {
            module_id: self.module_id,
            area_id: self.area_id,
            stores: self.inventory.stores.iter().filter(|(id, _)| ids.contains(id)).map(|(id, s)| (*id, s.clone())).collect(),
            keep_corpse: self.combat.keep_corpse.iter().copied().filter(|id| ids.contains(id)).collect(),
            ai_style: self.combat.ai_style.iter().filter(|(id, _)| ids.contains(id)).map(|(id, s)| (*id, *s)).collect(),
            objects,
            left_at: self.time,
        };
        self.module_memory.insert(name, snap);
    }

    /// Restores a remembered module's objects and builds its walkmesh.
    /// Returns false when the module was never visited (spawn the GIT).
    pub(crate) fn restore_module(&mut self, module: &Module) -> bool {
        let Some(snap) = self.module_memory.remove(&module.name.to_ascii_lowercase()) else { return false };
        for o in snap.objects {
            self.objects.insert(o.id, o);
        }
        self.module_id = snap.module_id;
        self.area_id = snap.area_id;
        self.inventory.stores.extend(snap.stores);
        self.rebuild_encounter_details(module);
        self.refresh_all_bodies();
        self.repair_all_weapon_slots();
        self.combat.keep_corpse.extend(snap.keep_corpse);
        self.combat.ai_style.extend(snap.ai_style);
        self.build_walkmesh(module);
        self.attach_object_walks();
        true
    }

    // ---------------------------------------------------------------
    // Determinism check
    // ---------------------------------------------------------------

    /// Order-independent digest of the simulation state (objects, globals,
    /// party, time, RNG). Two worlds that will behave the same have the same
    /// digest; used by the save round-trip test.
    pub fn state_digest(&self) -> u64 {
        let mut h = Fnv::default();
        h.u64(self.time.to_bits());
        h.u64(self.builtins.rng_state());
        for &p in &self.party {
            h.u32(p);
        }
        for o in self.objects.values() {
            h.u32(o.id);
            h.u32(o.kind as u32);
            h.str(&o.tag);
            h.str(&o.template);
            h.u32(u32::from(o.destroyed));
            for v in o.position.to_array() {
                h.u32(v.to_bits());
            }
            h.u32(o.bearing.to_bits());
            h.u32(o.actions.len() as u32);
            h.u32(u32::from(o.commandable));
            for &i in &o.inventory {
                h.u32(i);
            }
            let mut bools: Vec<_> = o.local_bools.iter().collect();
            bools.sort();
            for (k, v) in bools {
                h.u32(*k as u32);
                h.u32(u32::from(*v));
            }
            let mut nums: Vec<_> = o.local_numbers.iter().collect();
            nums.sort();
            for (k, v) in nums {
                h.u32(*k as u32);
                h.u32(*v as u32);
            }
            match &o.detail {
                Detail::Creature(c) => {
                    h.u32(c.hp as u32);
                    h.u32(u32::from(c.dead));
                    h.u32(u32::from(c.down));
                    h.u32(c.faction as u32);
                    h.u32(u32::from(c.weapon_set));
                    let mut eq: Vec<_> = c.equipped.iter().collect();
                    eq.sort();
                    for (s, i) in eq {
                        h.u32(*s as u32);
                        h.u32(*i);
                    }
                }
                Detail::Door(d) => {
                    h.u32(u32::from(d.open));
                    h.u32(u32::from(d.locked));
                }
                Detail::Placeable(p) => {
                    h.u32(u32::from(p.open));
                    h.u32(u32::from(p.locked));
                }
                Detail::Item(i) => {
                    h.u32(i.stack_size as u32);
                    h.u32(i.possessor.unwrap_or(u32::MAX));
                }
                Detail::Encounter(e) => {
                    h.u32(u32::from(e.active));
                    h.u32(u32::from(e.running));
                    h.u32(u32::from(e.exhausted));
                    h.u32(e.exhaustions);
                    h.u32(e.spawns_max as u32);
                    h.u32(e.spawns_current as u32);
                    h.u32(e.reset_in.unwrap_or(-1.0).to_bits());
                    for c in &e.creatures {
                        h.u32(c.owed as u32);
                        h.u32(u32::from(c.broken));
                    }
                    let mut spawned = e.spawned.clone();
                    spawned.sort_unstable();
                    for c in spawned {
                        h.u32(c);
                    }
                    let mut inside: Vec<ObjectId> = e.inside.iter().copied().collect();
                    inside.sort_unstable();
                    for c in inside {
                        h.u32(c);
                    }
                }
                _ => {}
            }
        }
        for (k, v) in &self.globals.numbers {
            h.str(k);
            h.u32(*v as u32);
        }
        for (k, v) in &self.globals.booleans {
            h.str(k);
            h.u32(u32::from(*v));
        }
        h.finish()
    }

    /// Live creatures of the current area that are not in the party, for
    /// tests and tools.
    pub fn area_creatures(&self) -> Vec<ObjectId> {
        self.iter().filter(|o| o.kind == ObjectKind::Creature && !self.party.contains(&o.id)).map(|o| o.id).collect()
    }
}

#[derive(Default)]
struct Fnv(u64);

impl Fnv {
    fn u32(&mut self, v: u32) {
        self.write(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.write(&v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.write(s.as_bytes());
        self.write(&[0xff]);
    }
}

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        if self.0 == 0 {
            self.0 = 0xcbf2_9ce4_8422_2325;
        }
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

// -------------------------------------------------------------------
// Slots on disk
// -------------------------------------------------------------------

/// `$XDG_DATA_HOME/kotor2-rs/saves` (default `~/.local/share/kotor2-rs/saves`).
/// Never inside the game folder.
pub fn default_save_root() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("kotor2-rs").join("saves")
}

/// A slot directory with its description.
#[derive(Debug, Clone)]
pub struct SlotInfo {
    pub slot: String,
    pub dir: PathBuf,
    pub meta: SaveMeta,
    pub thumbnail: Option<PathBuf>,
    /// Why this build cannot load the slot (older/newer save format).
    pub incompatible: Option<String>,
}

/// Writes save.bin and meta.json into `root/slot`, creating it.
pub fn write_slot(root: &Path, slot: &str, data: &SaveData, meta: &SaveMeta) -> std::io::Result<PathBuf> {
    let dir = root.join(slot);
    std::fs::create_dir_all(&dir)?;
    // Write then rename, so a crash never leaves a half-written save.
    let tmp = dir.join("save.bin.tmp");
    std::fs::write(&tmp, data.to_bytes())?;
    std::fs::rename(&tmp, dir.join("save.bin"))?;
    std::fs::write(dir.join("meta.json"), serde_json::to_vec_pretty(meta).map_err(std::io::Error::other)?)?;
    Ok(dir)
}

pub fn read_slot(root: &Path, slot: &str) -> crate::Result<SaveData> {
    let bytes = std::fs::read(root.join(slot).join("save.bin")).map_err(|e| WorldError::Other(format!("{slot}: {e}")))?;
    SaveData::from_bytes(&bytes)
}

/// All slots under `root`, newest first.
pub fn list_slots(root: &Path) -> Vec<SlotInfo> {
    let mut out: Vec<SlotInfo> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("save.bin").is_file())
        .map(|e| {
            let dir = e.path();
            let meta = std::fs::read(dir.join("meta.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<SaveMeta>(&b).ok())
                .unwrap_or_default();
            let thumb = dir.join("thumb.png");
            SlotInfo {
                slot: e.file_name().to_string_lossy().into_owned(),
                thumbnail: thumb.is_file().then_some(thumb),
                incompatible: slot_compatibility(&dir).err().map(|e| e.to_string()),
                dir,
                meta,
            }
        })
        .collect();
    out.sort_by(|a, b| b.meta.timestamp.cmp(&a.meta.timestamp).then_with(|| a.slot.cmp(&b.slot)));
    out
}
