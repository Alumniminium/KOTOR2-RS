//! Module entry: spawning objects from k2-data, building the walkmesh,
//! placing the party and firing the load events.

use std::collections::HashMap;

use glam::{Affine3A, Quat, Vec3};
use k2_data::blueprint::{Creature, Door, Item, Placeable, SoundBlueprint, Trigger};
use k2_data::{Module, Tables};
use k2_res::{ResKey, Resources};
use k2_script::{ObjectId, OBJECT_INVALID};
use k2_walk::{Materials, WalkMeshBuilder};
use rakata_core::ResourceType;

use crate::object::{
    CreatureState, Detail, DoorState, ItemState, Object, ObjectKind, PlaceableState, SoundState, TriggerState,
};
use crate::{PlayerSpec, ScriptCtx, World, WorldEvent};

/// Default PC appearance: a medium male human (appearance.2da label
/// P_MAL_A_MED_01), looked up by label so no row number is hardcoded.
pub(crate) fn default_player_spec(res: &Resources, tables: &Tables) -> PlayerSpec {
    let appearance = tables
        .get(res, "appearance")
        .ok()
        .and_then(|t| (0..t.rows()).find(|&r| t.str(r, "label").is_some_and(|l| l.eq_ignore_ascii_case("P_MAL_A_MED_01"))))
        .unwrap_or(0) as u32;
    PlayerSpec { appearance, gender: 0, name: "Exile".into(), ..Default::default() }
}

fn scripts_map(list: &[(String, String)]) -> HashMap<String, String> {
    list.iter().filter(|(_, r)| !r.is_empty()).map(|(l, r)| (l.clone(), r.to_ascii_lowercase())).collect()
}

fn bwm(res: &Resources, name: &str, ty: ResourceType) -> Option<rakata_formats::Bwm> {
    let bytes = res.get(&ResKey::new(name, ty)).ok()??;
    rakata_formats::read_bwm_from_bytes(&bytes).ok()
}

fn object_xf(position: Vec3, bearing: f32) -> Affine3A {
    Affine3A::from_rotation_translation(Quat::from_rotation_z(bearing), position)
}

impl World {
    /// Loads a module and makes it current. The player and party carry over
    /// (with their ids); everything else is rebuilt. Places the party at the
    /// object tagged `waypoint_tag` when given and found, otherwise at the
    /// module entry point, then fires OnModLoad, OnSpawn, OnClientEnter and
    /// the area OnEnter.
    pub fn enter_module(&mut self, name: &str, waypoint_tag: Option<&str>) -> crate::Result<()> {
        let module = Module::load(&mut self.res, name)?;

        // Keep the party, drop everything else.
        let keep: Vec<ObjectId> = self.party.clone();
        let mut kept = Vec::new();
        for id in &keep {
            if let Some(mut o) = self.objects.remove(id) {
                o.actions.clear();
                let items: Vec<ObjectId> = o.inventory.clone();
                kept.push(o);
                for i in items {
                    if let Some(it) = self.objects.remove(&i) {
                        kept.push(it);
                    }
                }
                if let Some(c) = kept.iter().rev().find(|k| k.id == *id).and_then(|k| k.creature()) {
                    for (_, i) in c.equipped.clone() {
                        if let Some(it) = self.objects.remove(&i) {
                            kept.push(it);
                        }
                    }
                }
            }
        }
        // The party inventory lives on the PC, also while it is stashed by
        // SwitchPlayerCharacter (then it is not in the party list).
        let pc_items: Vec<ObjectId> = self
            .stashed_pc
            .as_ref()
            .map(|p| p.inventory.clone())
            .into_iter()
            .flatten()
            .chain(self.stashed_pc.as_ref().and_then(|p| p.creature()).map(|c| c.equipped.values().copied().collect::<Vec<_>>()).unwrap_or_default())
            .collect();
        for i in pc_items {
            if let Some(it) = self.objects.remove(&i) {
                kept.push(it);
            }
        }
        // Remember the module being left (save.rs): its objects come back on
        // re-entry instead of a fresh GIT spawn.
        self.snapshot_module();
        // Store stock is rebuilt when the next store opens.
        self.inventory.stores.clear();
        self.inventory.open_store = None;
        self.inventory.open_container = None;
        self.objects.clear();
        for o in kept {
            self.objects.insert(o.id, o);
        }
        self.timers.clear();
        self.looping_scripts.clear();
        self.pending.clear();
        self.conversation = None;
        self.pending_transition = None;
        self.area_unescapable_override = None;
        self.transit_disabled = false;
        self.iters = Default::default();
        self.door_walk.clear();
        self.placeable_walk.clear();
        self.paths.clear();
        self.crowd.clear();

        let restored = self.restore_module(&module);
        let creatures = if restored { Vec::new() } else { self.spawn_git(&module) };

        // Player and party.
        if !self.is_valid(self.player) && self.stashed_pc.is_none() {
            let id = self.spawn_player();
            self.player = id;
            self.party.insert(0, id);
        }
        self.party.retain(|&id| self.objects.contains_key(&id));
        let (mut pos, mut bearing) = (Vec3::from(module.info.entry_position), module.info.entry_bearing);
        if let Some(tag) = waypoint_tag
            && let Some(t) = self.by_tag(tag, 0)
            && let Some(o) = self.get(t)
        {
            pos = o.position;
            bearing = o.bearing;
        }
        let party = self.party.clone();
        for (i, id) in party.iter().enumerate() {
            let offset = if i == 0 {
                Vec3::ZERO
            } else {
                let back = Quat::from_rotation_z(bearing) * Vec3::new(0.0, -1.5, 0.0);
                back + Quat::from_rotation_z(bearing) * Vec3::new(if i % 2 == 1 { -1.0 } else { 1.0 }, 0.0, 0.0)
            };
            let mut p = pos + offset;
            if let Some(h) = self.walk.ground(p.x, p.y, p.z + 1.0) {
                p.z = h.z;
            } else if i > 0 {
                p = pos;
            }
            if let Some(o) = self.get_mut(*id) {
                o.position = p;
                o.bearing = bearing;
            }
        }
        // Don't fire transition triggers the party starts inside. On a first
        // visit the other triggers do fire for a party that starts inside
        // (101per's Awaken trigger sits on the entry point and starts the
        // intro cutscene); a revisit marks them all as already entered.
        self.prime_triggers(restored);

        let name = module.name.clone();
        self.module = Some(module);
        // Doors spawned with the module start in their saved state: their routes are set before the grid is built.
        self.sync_door_passable();
        // Build the navigation grid now rather than on the first move.
        self.walk.prepare_nav();
        self.emit(WorldEvent::ModuleEntered { module: name.clone() });
        let mut all: Vec<ObjectId> = self.iter().map(|o| o.id).collect();
        all.sort_unstable();
        for id in all {
            self.emit(WorldEvent::ObjectSpawned(id));
        }
        if self.globals.declared_count() > 0 {
            self.globals.set_string("K_LAST_MODULE", name.to_ascii_uppercase());
        }

        let leader = self.party_leader();
        let enter = ScriptCtx { entering: leader, ..Default::default() };
        self.door_anims.instant = true;
        self.fire(self.module_id, "Mod_OnModLoad", ScriptCtx::default());
        for c in creatures {
            self.queue_event(c, "ScriptSpawn", ScriptCtx::default());
        }
        self.pump();
        if !restored {
            self.fire(self.module_id, "Mod_OnModStart", ScriptCtx::default());
        }
        self.start_minigame();
        self.fire(self.module_id, "Mod_OnClientEntr", enter.clone());
        self.fire(self.area_id, "OnEnter", enter);
        // Triggers the party starts inside fire now, from the module's entry
        // position: the client nudges the leader off blocking placeables
        // before the first tick, which would step it out of the 101per
        // Awaken trigger (the tank sits on the entry point) unseen.
        self.run_triggers();
        // Then the leader moves off a walkmesh island its entry point may sit on, to the
        // engine's safe location (WalkMesh::safe_spawn), as the game does on area load.
        let leader = self.party_leader();
        if let Some(p) = self.get(leader).map(|o| o.position)
            && let Some(safe) = self.walk.safe_spawn(p, self.leader_clearance())
            && let Some(o) = self.get_mut(leader)
        {
            o.position = safe;
        }
        self.door_anims.instant = false;
        Ok(())
    }

    /// First visit: module and area objects plus everything in the GIT.
    /// Returns the spawned creatures (their OnSpawn runs after placement).
    fn spawn_git(&mut self, module: &Module) -> Vec<ObjectId> {
        // Module and area objects.
        let module_id = self.alloc_id();
        let mut m = Object::new(module_id, ObjectKind::Module);
        m.tag = module.info.tag.clone();
        m.template = module.name.clone();
        m.name = self.resolve(&module.info.name);
        m.scripts = scripts_map(&module.info.scripts);
        self.objects.insert(module_id, m);
        self.module_id = module_id;

        let area_id = self.alloc_id();
        let mut a = Object::new(area_id, ObjectKind::Area);
        a.tag = module.area.tag.clone();
        a.template = module.area.resref.clone();
        a.name = self.resolve(&module.area.name);
        a.scripts = scripts_map(&module.area.scripts);
        self.objects.insert(area_id, a);
        self.area_id = area_id;

        self.build_walkmesh(module);

        let mut creatures = Vec::new();
        for c in &module.git.creatures {
            match self.spawn_creature(&c.template, Vec3::from(c.position), c.bearing) {
                Ok(id) => creatures.push(id),
                Err(e) => self.warnings.push(format!("creature {}: {e}", c.template)),
            }
        }
        for d in &module.git.doors {
            let r = Door::load(&self.res, &d.template).map(|bp| {
                let id = self.alloc_id();
                let mut o = Object::new(id, ObjectKind::Door);
                o.tag = if d.tag.is_empty() { bp.tag.clone() } else { d.tag.clone() };
                o.template = d.template.clone();
                o.name = self.resolve(&bp.name);
                o.position = Vec3::from(d.position);
                o.bearing = d.bearing;
                o.scripts = scripts_map(&bp.scripts);
                o.conversation = bp.conversation.clone();
                o.plot = bp.plot;
                o.trap = crate::traps::TrapState::from_fields(&bp.trap).map(Box::new);
                let model = bp.model(&self.res, &self.tables).ok();
                o.detail = Detail::Door(DoorState {
                    open: bp.open_state != 0,
                    locked: bp.locked,
                    locked_at_load: Some(bp.locked),
                    key_required: bp.key_required,
                    key_name: bp.key_name.clone(),
                    is_static: bp.is_static,
                    linked_to: if d.linked_to.is_empty() { bp.linked_to.clone() } else { d.linked_to.clone() },
                    linked_to_module: d.linked_to_module.clone().or(bp.linked_to_module.clone()).filter(|m| !m.is_empty()),
                    model: model.clone(),
                    hp: crate::bash::door_hp(&bp),
                    ..Default::default()
                });
                if let Some(model) = &model {
                    let xf = object_xf(o.position, o.bearing);
                    let [c0, c1, c2] = [0, 1, 2].map(|i| bwm(&self.res, &format!("{model}{i}"), ResourceType::Dwk));
                    if c0.is_some() || c1.is_some() || c2.is_some() {
                        let h = self.walk.add_door(c0.as_ref(), c1.as_ref(), c2.as_ref(), xf);
                        if bp.open_state != 0 {
                            self.walk.set_door(h, k2_walk::DoorState::Open1);
                        }
                        self.door_walk.insert(id, h);
                    }
                }
                self.objects.insert(id, o);
            });
            if let Err(e) = r {
                self.warnings.push(format!("door {}: {e}", d.template));
            }
        }
        for p in &module.git.placeables {
            let r = Placeable::load(&self.res, &p.template).map(|bp| {
                let id = self.alloc_id();
                let mut o = Object::new(id, ObjectKind::Placeable);
                o.tag = bp.tag.clone();
                o.template = p.template.clone();
                o.name = self.resolve(&bp.name);
                o.position = Vec3::from(p.position);
                o.bearing = p.bearing;
                o.scripts = scripts_map(&bp.scripts);
                o.conversation = bp.conversation.clone();
                o.plot = bp.plot;
                o.trap = crate::traps::TrapState::from_fields(&bp.trap).map(Box::new);
                let model = bp.model(&self.res, &self.tables).ok();
                if let Some(model) = &model
                    && let Some(b) = bwm(&self.res, model, ResourceType::Pwk)
                {
                    let mh = self.walk.add_object(&b, object_xf(o.position, o.bearing));
                    self.placeable_walk.insert(id, mh);
                }
                o.detail = Detail::Placeable(PlaceableState {
                    useable: bp.useable,
                    locked: bp.locked,
                    open: false,
                    has_inventory: bp.has_inventory,
                    is_static: bp.is_static,
                    model,
                    hp: crate::bash::placeable_hp(&bp),
                });
                self.objects.insert(id, o);
                for item in bp.inventory.clone() {
                    self.create_item(&item, id, 1);
                }
            });
            if let Err(e) = r {
                self.warnings.push(format!("placeable {}: {e}", p.template));
            }
        }
        for w in &module.git.waypoints {
            let id = self.alloc_id();
            let mut o = Object::new(id, ObjectKind::Waypoint);
            o.tag = w.tag.clone();
            o.template = w.template.clone();
            o.name = self.resolve(&w.name);
            o.position = Vec3::from(w.position);
            o.bearing = w.bearing;
            self.objects.insert(id, o);
        }
        for t in &module.git.triggers {
            let r = Trigger::load(&self.res, &t.template).map(|bp| {
                let id = self.alloc_id();
                let mut o = Object::new(id, ObjectKind::Trigger);
                o.tag = t.tag.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| bp.tag.clone());
                o.template = t.template.clone();
                o.name = self.resolve(&bp.name);
                o.position = Vec3::from(t.position);
                o.scripts = scripts_map(&bp.scripts);
                let linked_to_module =
                    t.linked_to_module.clone().or(bp.linked_to_module.clone()).filter(|m| !m.is_empty());
                let linked_to = t.linked_to.clone().filter(|s| !s.is_empty()).or(Some(bp.linked_to.clone()).filter(|s| !s.is_empty()));
                o.detail = Detail::Trigger(TriggerState {
                    polygon: t.world_polygon(),
                    is_transition: bp.is_transition() || linked_to_module.is_some(),
                    linked_to,
                    linked_to_module,
                    inside: Default::default(),
                });
                o.trap = crate::traps::TrapState::from_trigger(&bp).map(Box::new);
                self.objects.insert(id, o);
            });
            if let Err(e) = r {
                self.warnings.push(format!("trigger {}: {e}", t.template));
            }
        }
        for s in &module.git.sounds {
            let id = self.alloc_id();
            let mut o = Object::new(id, ObjectKind::Sound);
            o.template = s.template.clone();
            o.position = Vec3::from(s.position);
            if let Ok(bp) = SoundBlueprint::load(&self.res, &s.template) {
                o.tag = bp.tag.clone();
                o.detail = Detail::Sound(SoundState { active: bp.active, looping: bp.looping, sounds: bp.sounds.clone() });
            }
            self.objects.insert(id, o);
        }
        for s in &module.git.stores {
            let id = self.alloc_id();
            let mut o = Object::new(id, ObjectKind::Store);
            o.tag = s.tag.clone();
            // GIT store entries carry no Tag; scripts look stores up by the
            // UTM's.
            if o.tag.is_empty()
                && let Ok(g) = k2_data::read_gff(&self.res, &s.template, ResourceType::Utm)
            {
                o.tag = k2_data::GffExt::string_or_empty(&g.root, "Tag");
            }
            o.template = s.template.clone();
            o.position = Vec3::from(s.position);
            o.bearing = s.bearing;
            self.objects.insert(id, o);
        }
        for e in &module.git.encounters {
            let id = self.alloc_id();
            let mut o = Object::new(id, ObjectKind::Encounter);
            o.template = e.template.clone();
            o.position = Vec3::from(e.position);
            if let Err(err) = self.fill_encounter(&mut o, e) {
                self.warnings.push(format!("encounter {}: {err}", e.template));
            }
            self.objects.insert(id, o);
        }
        creatures
    }

    /// Adds placeable PWKs and door DWKs of the live objects to the walkmesh
    /// (restored modules and loaded saves; the GIT spawn adds its own).
    pub(crate) fn attach_object_walks(&mut self) {
        self.door_walk.clear();
        self.placeable_walk.clear();
        // A door saved mid transition loads settled.
        for o in self.objects.values_mut() {
            if let Some(d) = o.door_mut() {
                d.opening = 0.0;
            }
        }
        // (id, is door, model, open, transform)
        let objs: Vec<(ObjectId, bool, String, bool, Affine3A)> = self
            .iter()
            .filter_map(|o| match &o.detail {
                Detail::Door(d) => Some((o.id, true, d.model.clone()?, d.open, object_xf(o.position, o.bearing))),
                Detail::Placeable(p) => Some((o.id, false, p.model.clone()?, false, object_xf(o.position, o.bearing))),
                _ => None,
            })
            .collect();
        for (id, door, model, open, xf) in objs {
            if door {
                let [c0, c1, c2] = [0, 1, 2].map(|i| bwm(&self.res, &format!("{model}{i}"), ResourceType::Dwk));
                if c0.is_some() || c1.is_some() || c2.is_some() {
                    let h = self.walk.add_door(c0.as_ref(), c1.as_ref(), c2.as_ref(), xf);
                    self.door_walk.insert(id, h);
                    if open {
                        let side = self.get(id).and_then(|o| o.door()).map_or(1, |d| d.side);
                        let state = self.door_open_walk(id, side);
                        self.walk.set_door(h, state);
                    }
                }
            } else if let Some(b) = bwm(&self.res, &model, ResourceType::Pwk) {
                let mh = self.walk.add_object(&b, xf);
                self.placeable_walk.insert(id, mh);
            }
        }
        self.sync_door_passable();
    }

    pub(crate) fn build_walkmesh(&mut self, module: &Module) {
        let materials = self
            .res
            .get(&ResKey::new("surfacemat", ResourceType::TwoDa))
            .ok()
            .flatten()
            .and_then(|b| Materials::from_2da_bytes(&b).ok())
            .unwrap_or_default();
        let mut b = WalkMeshBuilder::new(materials);
        for room in &module.layout.rooms {
            if let Some(w) = bwm(&self.res, &room.model, ResourceType::Wok) {
                b.add_room(&w);
            }
        }
        self.walk = b.build();
    }

    pub(crate) fn resolve(&self, s: &k2_data::LocString) -> String {
        self.talk.as_ref().map(|t| t.resolve(s)).unwrap_or_default()
    }

    fn spawn_player(&mut self) -> ObjectId {
        // Character generation (or the default level 1 Guardian): chargen.rs.
        self.spawn_pc_from_spec()
    }

    pub(crate) fn fill_appearance(&self, c: &mut CreatureState, body_variation: char, texture_variation: u32) {
        if let Ok(m) = self.tables.creature_models(&self.res, c.appearance, body_variation, texture_variation) {
            c.body_model = Some(m.body);
            c.body_texture = m.body_texture;
            c.head_model = m.head;
            if m.walk_speed > 0.0 {
                c.walk_speed = m.walk_speed;
            }
            if m.run_speed > 0.0 {
                c.run_speed = m.run_speed;
            }
            if m.hit_radius > 0.0 {
                c.radius = m.hit_radius;
            }
        }
        if let Ok(t) = self.tables.get(&self.res, "appearance") {
            c.racial_type = t.int(c.appearance as usize, "racialtype").unwrap_or(6);
        }
    }

    /// Spawns a creature from a UTC blueprint.
    pub fn spawn_creature(&mut self, template: &str, position: Vec3, bearing: f32) -> crate::Result<ObjectId> {
        let bp = Creature::load(&self.res, template)?;
        let id = self.alloc_id();
        let mut o = Object::new(id, ObjectKind::Creature);
        o.tag = bp.tag.clone();
        o.template = template.to_ascii_lowercase();
        let first = self.resolve(&bp.first_name);
        let last = self.resolve(&bp.last_name);
        o.name = if last.is_empty() { first } else { format!("{first} {last}") };
        o.position = position;
        o.bearing = bearing;
        o.scripts = scripts_map(&bp.scripts);
        o.conversation = bp.conversation.clone().filter(|c| !c.is_empty());
        o.plot = bp.plot;
        let max_hp = if bp.max_hit_points > 0 { bp.max_hit_points } else { bp.hit_points.max(1) };
        let mut c = CreatureState {
            appearance: bp.appearance,
            gender: bp.gender as i32,
            hp: if bp.current_hit_points > 0 { bp.current_hit_points } else { max_hp },
            max_hp,
            force: bp.current_force,
            max_force: bp.force_points,
            min1hp: bp.min1hp,
            is_pc: bp.is_pc,
            good_evil: bp.good_evil,
            faction: bp.faction as i32,
            abilities: bp.abilities,
            classes: bp.classes.clone(),
            hologram: bp.hologram,
            not_reorienting: bp.not_reorienting,
            walk_speed: 1.75,
            run_speed: 5.4,
            radius: 0.5,
            perception_range: 20.0,
            ..Default::default()
        };
        // UTC PerceptionRange indexes ranges.2da rows 8..13; the sight range
        // is primaryrange. The player row (250) is capped at the long range.
        if let Ok(t) = self.tables.get(&self.res, "ranges") {
            let row = if (8..=13).contains(&bp.perception_range) { bp.perception_range as usize } else { 11 };
            if let Some(r) = t.float(row, "primaryrange").filter(|r| *r > 0.0) {
                c.perception_range = r.min(35.0);
            }
        }
        if let Ok(m) = bp.models(&self.res, &self.tables) {
            c.body_model = Some(m.body);
            c.body_texture = m.body_texture;
            c.head_model = m.head;
            if m.walk_speed > 0.0 {
                c.walk_speed = m.walk_speed;
            }
            if m.run_speed > 0.0 {
                c.run_speed = m.run_speed;
            }
            if m.hit_radius > 0.0 {
                c.radius = m.hit_radius;
            }
        }
        if let Ok(t) = self.tables.get(&self.res, "appearance") {
            c.racial_type = t.int(bp.appearance as usize, "racialtype").unwrap_or(6);
        }
        o.detail = Detail::Creature(Box::new(c));
        self.objects.insert(id, o);
        for (slot_mask, item) in &bp.equipment {
            let slot = slot_mask.trailing_zeros() as i32;
            if let Some(it) = self.create_item(item, id, 1) {
                // Equipped items are not in the inventory list.
                if let Some(owner) = self.get_mut(id) {
                    owner.inventory.retain(|&i| i != it);
                    if let Some(c) = owner.creature_mut() {
                        c.equipped.insert(slot, it);
                    }
                }
            }
        }
        self.repair_weapon_slots(id);
        for item in &bp.inventory {
            self.create_item(item, id, 1);
        }
        Ok(id)
    }

    /// Creates an item from a UTI blueprint, in `owner`'s inventory (or on
    /// the ground when `owner` is invalid). Returns None for a missing UTI.
    pub fn create_item(&mut self, template: &str, owner: ObjectId, stack: i32) -> Option<ObjectId> {
        let bp = Item::load(&self.res, template).ok()?;
        let id = self.alloc_id();
        let mut o = Object::new(id, ObjectKind::Item);
        o.tag = bp.tag.clone();
        o.template = template.to_ascii_lowercase();
        o.name = self.resolve(&bp.name);
        o.plot = bp.plot;
        let possessor = self.is_valid(owner).then_some(owner);
        if let Some(p) = possessor.and_then(|p| self.get(p)) {
            o.position = p.position;
        }
        o.detail = Detail::Item(ItemState {
            base_item: bp.base_item,
            stack_size: stack.max(bp.stack_size as i32).max(1),
            possessor,
            plot: bp.plot,
        });
        self.objects.insert(id, o);
        if let Some(p) = possessor.and_then(|p| self.get_mut(p)) {
            p.inventory.push(id);
        }
        Some(id)
    }

    /// Creates a placeable from a UTP blueprint (CreateObject).
    pub fn spawn_placeable(&mut self, template: &str, position: Vec3, bearing: f32) -> Option<ObjectId> {
        let bp = Placeable::load(&self.res, template).ok()?;
        let id = self.alloc_id();
        let mut o = Object::new(id, ObjectKind::Placeable);
        o.tag = bp.tag.clone();
        o.template = template.to_ascii_lowercase();
        o.name = self.resolve(&bp.name);
        o.position = position;
        o.bearing = bearing;
        o.scripts = scripts_map(&bp.scripts);
        o.conversation = bp.conversation.clone();
        o.plot = bp.plot;
        o.trap = crate::traps::TrapState::from_fields(&bp.trap).map(Box::new);
        let model = bp.model(&self.res, &self.tables).ok();
        o.detail = Detail::Placeable(PlaceableState {
            useable: bp.useable,
            locked: bp.locked,
            open: false,
            has_inventory: bp.has_inventory,
            is_static: bp.is_static,
            model,
            hp: crate::bash::placeable_hp(&bp),
        });
        self.objects.insert(id, o);
        for item in bp.inventory.clone() {
            self.create_item(&item, id, 1);
        }
        Some(id)
    }

    /// Creates a waypoint (CreateObject with OBJECT_TYPE_WAYPOINT).
    pub fn spawn_waypoint(&mut self, template: &str, position: Vec3, bearing: f32) -> ObjectId {
        let id = self.alloc_id();
        let mut o = Object::new(id, ObjectKind::Waypoint);
        o.template = template.to_ascii_lowercase();
        if let Ok(w) = k2_data::Waypoint::load(&self.res, template) {
            o.tag = w.tag.clone();
            o.name = self.resolve(&w.name);
        }
        o.position = position;
        o.bearing = bearing;
        self.objects.insert(id, o);
        id
    }

    /// Marks transition triggers (all triggers when `all`) the party starts
    /// inside as already entered. On a revisit (`all`) the encounters' occupants
    /// are primed too (prime_encounters).
    fn prime_triggers(&mut self, all: bool) {
        let party: Vec<(ObjectId, Vec3)> =
            self.party.iter().filter_map(|&id| self.get(id).map(|o| (id, o.position))).collect();
        for o in self.objects.values_mut() {
            if let Detail::Trigger(t) = &mut o.detail
                && (all || t.is_transition)
            {
                for &(id, p) in &party {
                    if k2_data::module::point_in_polygon(&t.polygon, p.x, p.y) {
                        t.inside.insert(id);
                    }
                }
            }
        }
        if all {
            self.prime_encounters();
        }
    }

    /// Destroys an object (and its inventory).
    pub fn destroy(&mut self, id: ObjectId) {
        if id == self.player || id == OBJECT_INVALID {
            return;
        }
        let Some(o) = self.objects.get_mut(&id) else { return };
        if o.destroyed {
            return;
        }
        o.destroyed = true;
        let inv = std::mem::take(&mut o.inventory);
        // A destroyed container cannot stay the open one (it would block saving).
        if self.inventory.open_container == Some(id) {
            self.inventory.open_container = None;
        }
        if let Some(h) = self.placeable_walk.remove(&id) {
            self.walk.set_enabled(h, false);
        }
        if let Some(h) = self.door_walk.remove(&id) {
            self.walk.set_door(h, k2_walk::DoorState::Open1);
        }
        let possessor = o.item().and_then(|i| i.possessor);
        for i in inv {
            if let Some(it) = self.objects.get_mut(&i) {
                it.destroyed = true;
            }
        }
        if let Some(p) = possessor.and_then(|p| self.objects.get_mut(&p)) {
            p.inventory.retain(|&i| i != id);
            if let Some(c) = p.creature_mut() {
                c.equipped.retain(|_, &mut i| i != id);
            }
        }
        self.party.retain(|&p| p != id);
        self.emit(WorldEvent::ObjectDestroyed(id));
    }
}
