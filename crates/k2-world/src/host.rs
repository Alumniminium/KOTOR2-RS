//! Engine routines: [`ScriptHost`] for [`World`].
//!
//! Routines are dispatched by name. Anything not handled here falls through
//! to k2-script's [`k2_script::Builtins`] (math, strings, dice), then to the
//! type's default value, counted in `World::unimplemented`.

use std::sync::Arc;

use glam::Vec3;
use k2_script::nss::{ConstValue, VarType};
use k2_script::{Call, ObjectId, ScriptHost, ScriptState, Value, OBJECT_INVALID};

use crate::engine::{Effect, EngineData, Event, Location, Talent, ENGINE_EFFECT, ENGINE_EVENT, ENGINE_TALENT};
use crate::object::{bearing_towards, facing_to_bearing, Action, Detail, FollowTarget, ObjectKind, Target};
use crate::{AvailableNpc, Pending, ScriptCtx, TimerKind, World, WorldEvent};

fn int(a: &[Value], n: usize) -> i32 {
    a.get(n).and_then(Value::as_int).unwrap_or(0)
}

fn float(a: &[Value], n: usize) -> f32 {
    a.get(n).and_then(Value::as_float).unwrap_or(0.0)
}

fn string(a: &[Value], n: usize) -> &str {
    a.get(n).and_then(Value::as_str).unwrap_or("")
}

fn obj(a: &[Value], n: usize) -> ObjectId {
    a.get(n).and_then(Value::as_object).unwrap_or(OBJECT_INVALID)
}

fn vector(a: &[Value], n: usize) -> Vec3 {
    a.get(n).and_then(Value::as_vector).map(Vec3::from).unwrap_or(Vec3::ZERO)
}

fn action(a: &[Value], n: usize) -> Option<Arc<ScriptState>> {
    match a.get(n) {
        Some(Value::Action(s)) => Some(s.clone()),
        _ => None,
    }
}

fn i(v: i32) -> Value {
    Value::Int(v)
}

fn b(v: bool) -> Value {
    Value::Int(v as i32)
}

fn f(v: f32) -> Value {
    Value::Float(v)
}

fn s(v: impl AsRef<str>) -> Value {
    Value::String(Arc::from(v.as_ref()))
}

fn o(v: ObjectId) -> Value {
    Value::Object(v)
}

fn o_opt(v: Option<ObjectId>) -> Value {
    Value::Object(v.unwrap_or(OBJECT_INVALID))
}

fn vec(v: Vec3) -> Value {
    Value::Vector(v.to_array())
}

/// "EffectMovementSpeedDecrease" -> "MOVEMENT_SPEED_DECREASE".
fn camel_to_upper_snake(name: &str) -> String {
    let mut out = String::new();
    for (idx, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() && idx > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_uppercase());
    }
    out
}

impl ScriptHost for World {
    fn call(&mut self, call: Call<'_>) -> Value {
        let log = self.call_log;
        let (name, caller) = (call.routine.name.clone(), call.caller);
        let v = self.call_inner(call);
        if log && std::env::var_os("K2_CALLS_RET").is_some() {
            println!("        -> {name} = {v:?} (self={})", self.describe(caller));
        }
        v
    }
}

impl World {
    fn call_inner(&mut self, call: Call<'_>) -> Value {
        let name = call.routine.name.as_str();
        if self.call_log {
            let args: Vec<String> = call.args.iter().map(|a| match a {
                Value::String(x) => format!("{x:?}"),
                Value::Object(x) => self.describe(*x),
                Value::Action(_) => "<action>".into(),
                other => format!("{other:?}"),
            }).collect();
            println!("      {}({}) self={}", name, args.join(", "), self.describe(call.caller));
        }
        if let Some(v) = self.minigame_call(name, call.caller, call.args) {
            return v;
        }
        if let Some(v) = self.pazaak_call(name, call.caller, call.args) {
            return v;
        }
        if let Some(v) = self.power_call(name, call.caller, call.args) {
            return v;
        }
        if let Some(v) = self.combat_call(name, call.caller, call.args) {
            return v;
        }
        if let Some(v) = self.craft_call(name, call.caller, call.args) {
            return v;
        }
        if let Some(v) = self.item_call(name, call.caller, call.args) {
            return v;
        }
        if let Some(v) = self.sheet_call(name, call.caller, call.args) {
            return v;
        }
        if let Some(v) = self.traps_call(name, call.caller, call.args) {
            return v;
        }
        if let Some(v) = self.stealth_call(name, call.caller, call.args) {
            return v;
        }
        if let Some(v) = self.influence_call(name, call.caller, call.args) {
            return v;
        }
        if let Some(v) = self.encounter_call(name, call.caller, call.args) {
            return v;
        }
        if let Some(v) = self.dispatch(name, call.caller, call.args, call.routine.ret) {
            return v;
        }
        if let Some(v) = self.builtins.call(&call) {
            return v;
        }
        *self.unimplemented.entry(name.to_owned()).or_default() += 1;
        Value::default_for(call.routine.ret)
    }
}

impl World {
    /// Integer constant from nwscript.nss.
    pub fn constant(&self, name: &str) -> Option<i32> {
        match self.routines.constants.get(name)? {
            ConstValue::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub(crate) fn location_of(&self, id: ObjectId) -> Location {
        self.get(id)
            .map(|o| Location { position: o.position, facing: o.facing_deg(), area: self.area_id })
            .unwrap_or_default()
    }

    fn loc_arg(&self, a: &[Value], n: usize) -> Location {
        a.get(n).and_then(|v| self.engine.location(v)).unwrap_or_default()
    }

    fn me_or(&self, a: &[Value], n: usize, me: ObjectId) -> ObjectId {
        let v = obj(a, n);
        if v == OBJECT_INVALID && a.len() <= n { me } else { v }
    }

    fn creature_state(&self, id: ObjectId) -> Option<&crate::object::CreatureState> {
        self.get(id).and_then(|o| o.creature())
    }

    fn pos(&self, id: ObjectId) -> Option<Vec3> {
        self.get(id).map(|o| o.position)
    }

    fn user_defined_label(&self, id: ObjectId) -> &'static str {
        match self.get(id).map(|o| o.kind) {
            Some(ObjectKind::Creature) | Some(ObjectKind::Trigger) => "ScriptUserDefine",
            Some(ObjectKind::Module) => "Mod_OnUsrDefined",
            _ => "OnUserDefined",
        }
    }

    /// Nearest live objects to `from` (excluding `from`), filtered.
    fn nearest(&self, from: ObjectId, filter: impl Fn(&crate::Object) -> bool) -> Vec<ObjectId> {
        let Some(p) = self.pos(from) else { return Vec::new() };
        let mut v: Vec<(f32, ObjectId)> = self
            .iter()
            .filter(|o| o.id != from && !matches!(o.kind, ObjectKind::Module | ObjectKind::Area))
            .filter(|o| o.item().is_none_or(|i| i.possessor.is_none()))
            .filter(|o| filter(o))
            .map(|o| (o.position.distance_squared(p), o.id))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        v.into_iter().map(|(_, id)| id).collect()
    }

    fn is_hostile(&self, a: ObjectId, b: ObjectId) -> bool {
        let fa = self.faction(a);
        let fb = self.faction(b);
        let hostile = |f: i32| f == 1 || f == 3 || f == 6 || f == 11;
        let friendly = |f: i32| f == 2 || f == 4 || f == 23;
        (hostile(fa) && friendly(fb)) || (friendly(fa) && hostile(fb)) || (fa == 1 && fb == 3) || (fa == 3 && fb == 1)
    }

    fn faction(&self, id: ObjectId) -> i32 {
        if self.party.contains(&id) {
            return 2;
        }
        self.creature_state(id).map_or(5, |c| c.faction)
    }

    fn is_dead(&self, id: ObjectId) -> bool {
        self.creature_state(id).is_some_and(|c| c.dead)
    }

    fn type_filter(o: &crate::Object, filter: i32) -> bool {
        let bit = o.kind.type_bit();
        bit != 0 && filter & bit != 0
    }

    fn new_effect(&mut self, name: &str, args: &[Value], creator: ObjectId) -> Value {
        let base = name.strip_prefix("Effect").unwrap_or(name);
        let ty = self.constant(&format!("EFFECT_TYPE_{}", camel_to_upper_snake(base))).unwrap_or(0);
        self.engine.add(EngineData::Effect(Effect { ty, name: name.to_owned(), args: args.to_vec(), creator }))
    }

    fn apply_effect(&mut self, eff: &Value, target: ObjectId, duration_type: i32, duration: f32) {
        let Some(e) = self.engine.effect(eff).cloned() else { return };
        match e.name.as_str() {
            "EffectLinkEffects" => {
                for a in &e.args {
                    self.apply_effect(a, target, duration_type, duration);
                }
            }
            "EffectDamage" => self.damage(target, int(&e.args, 0), e.creator),
            "EffectHeal" => self.heal(target, int(&e.args, 0)),
            "EffectHealForcePoints" => {
                if let Some(c) = self.get_mut(target).and_then(|o| o.creature_mut()) {
                    c.force = (c.force + int(&e.args, 0)).min(c.max_force.max(c.force));
                }
            }
            "EffectDeath" => self.harm(target, i32::MAX / 2, e.creator, true),
            "EffectResurrection" => self.resurrect(target, int(&e.args, 0)),
            "EffectVisualEffect" => self.emit(WorldEvent::VisualEffect { object: target, effect: int(&e.args, 0) }),
            _ => {
                if duration_type != 0 {
                    let handle = match eff {
                        Value::Engine(v) => v.handle,
                        _ => 0,
                    };
                    let remaining = (duration_type == 1).then_some(duration);
                    if let Some(o) = self.get_mut(target) {
                        o.effects.push(crate::object::AppliedEffect { handle, duration_type, remaining });
                    }
                }
            }
        }
    }

    /// Drops the applied effects of `target` whose engine value matches.
    fn remove_effects(&mut self, target: ObjectId, matches: impl Fn(&Effect) -> bool) {
        let gone: Vec<u32> = self.get(target).map_or_else(Vec::new, |o| {
            o.effects
                .iter()
                .filter(|ae| matches!(self.engine.by_handle(ae.handle), Some(EngineData::Effect(e)) if matches(e)))
                .map(|ae| ae.handle)
                .collect()
        });
        if let Some(o) = self.get_mut(target) {
            o.effects.retain(|ae| !gone.contains(&ae.handle));
        }
        self.forget_effect_handles(&gone);
    }

    /// Drops the spell id and absorb pool kept for removed effect handles.
    /// A handle still applied to another object keeps its entries.
    pub(crate) fn forget_effect_handles(&mut self, handles: &[u32]) {
        for &h in handles {
            if !self.objects.values().any(|o| o.effects.iter().any(|ae| ae.handle == h)) {
                self.powers.effect_spell.remove(&h);
                self.powers.absorb_left.remove(&h);
            }
        }
    }

    fn damage(&mut self, target: ObjectId, amount: i32, by: ObjectId) {
        self.apply_damage(target, amount, by);
    }

    /// EffectHeal: adds `amount` HP up to max. Healing a knocked-out creature
    /// above 0 HP wakes it (party.rs get_up). The dead are not healed: a
    /// heal does not clear `dead`, so it would leave a dead creature with HP.
    fn heal(&mut self, target: ObjectId, amount: i32) {
        if self.creature_state(target).is_some_and(|c| c.dead) {
            return;
        }
        if let Some(c) = self.get_mut(target).and_then(|o| o.creature_mut()) {
            c.hp = (c.hp + amount).min(c.max_hp);
        }
        if let Some(hp) = self.creature_state(target).filter(|c| c.down && c.hp > 0).map(|c| c.hp) {
            self.get_up(target, hp);
        }
    }

    /// EffectHeal from outside the scripts (test hook).
    pub fn heal_creature(&mut self, target: ObjectId, amount: i32) {
        self.heal(target, amount);
        self.pump();
    }

    fn push_front_clear(&mut self, id: ObjectId) {
        if let Some(o) = self.get_mut(id) {
            o.actions.clear();
            o.action_epoch += 1;
        }
    }

    fn give_item(&mut self, item: ObjectId, to: ObjectId) {
        let Some(old) = self.get(item).and_then(|o| o.item()).map(|i| i.possessor) else { return };
        if let Some(p) = old.and_then(|p| self.get_mut(p)) {
            p.inventory.retain(|&x| x != item);
            if let Some(c) = p.creature_mut() {
                c.equipped.retain(|_, &mut x| x != item);
            }
        }
        let to = self.is_valid(to).then_some(to);
        if let Some(it) = self.get_mut(item).and_then(|o| o.item_mut()) {
            it.possessor = to;
        }
        if let Some(t) = to.and_then(|t| self.get_mut(t)) {
            t.inventory.push(item);
        }
    }

    /// All items carried by `id`: inventory plus equipped.
    fn carried(&self, id: ObjectId) -> Vec<ObjectId> {
        let Some(o) = self.get(id) else { return Vec::new() };
        let mut v = o.inventory.clone();
        if let Some(c) = o.creature() {
            let mut eq: Vec<_> = c.equipped.iter().collect();
            eq.sort();
            v.extend(eq.into_iter().map(|(_, &i)| i));
        }
        v
    }

    fn party_gold(&self) -> i32 {
        self.creature_state(self.player).map_or(0, |c| c.gold)
    }

    fn add_gold(&mut self, delta: i32) {
        let p = self.player;
        if let Some(c) = self.get_mut(p).and_then(|o| o.creature_mut()) {
            c.gold = (c.gold + delta).max(0);
        }
    }

    fn npc_object(&self, npc: i32) -> Option<ObjectId> {
        if npc == -1 {
            return Some(self.player);
        }
        self.available_npcs.get(&npc).and_then(|n| n.object).filter(|&id| self.is_valid(id))
    }

    pub(crate) fn spawn_npc(&mut self, npc: i32, loc: Location) -> ObjectId {
        if let Some(id) = self.npc_object(npc) {
            self.place(id, loc.position);
            if let Some(o) = self.get_mut(id) {
                o.bearing = facing_to_bearing(loc.facing);
            }
            return id;
        }
        let Some(tpl) = self.available_npcs.get(&npc).map(|n| n.template.clone()).filter(|t| !t.is_empty()) else {
            return OBJECT_INVALID;
        };
        match self.spawn_creature(&tpl, loc.position, facing_to_bearing(loc.facing)) {
            Ok(id) => {
                self.place(id, loc.position);
                if let Some(n) = self.available_npcs.get_mut(&npc) {
                    n.object = Some(id);
                }
                self.emit(WorldEvent::ObjectSpawned(id));
                self.queue_event(id, "ScriptSpawn", ScriptCtx::default());
                id
            }
            Err(e) => {
                self.warnings.push(format!("SpawnAvailableNPC {npc} {tpl}: {e}"));
                OBJECT_INVALID
            }
        }
    }

    /// K2 SwitchPlayerCharacter: control NPC `npc` alone (spawning it at
    /// the leader's spot if needed); the original PC leaves the area until
    /// switched back with NPC_PLAYER (-1).
    fn switch_player_character(&mut self, npc: i32) -> bool {
        if npc == -1 {
            let Some(pc) = self.stashed_pc.take() else { return self.party.first() == Some(&self.player) };
            let at = self.get(self.party_leader()).map(|o| (o.position, o.bearing));
            let id = pc.id;
            self.objects.insert(id, pc);
            if let (Some((p, b)), Some(o)) = (at, self.get_mut(id)) {
                o.position = p;
                o.bearing = b;
            }
            self.party.retain(|&p| p != id);
            self.party.insert(0, id);
            self.drop_follows(id);
            self.emit(WorldEvent::ObjectSpawned(id));
            return true;
        }
        let leader = self.party_leader();
        let loc = self.location_of(leader);
        let id = match self.npc_object(npc) {
            Some(id) => id,
            None => self.spawn_npc(npc, loc),
        };
        if !self.is_valid(id) {
            return false;
        }
        if self.stashed_pc.is_none()
            && leader == self.player
            && id != self.player
            && let Some(pc) = self.objects.remove(&self.player)
        {
            self.stashed_pc = Some(pc);
            self.emit(WorldEvent::ObjectDestroyed(self.player));
        }
        self.party.retain(|&p| p != id && p != self.player);
        self.party.insert(0, id);
        self.drop_follows(id);
        true
    }

    fn create_object(&mut self, ty: i32, template: &str, loc: Location) -> ObjectId {
        let bearing = facing_to_bearing(loc.facing);
        let id = match ty {
            1 => match self.spawn_creature(template, loc.position, bearing) {
                Ok(id) => {
                    self.place(id, loc.position);
                    self.queue_event(id, "ScriptSpawn", ScriptCtx::default());
                    Some(id)
                }
                Err(e) => {
                    self.warnings.push(format!("CreateObject creature {template}: {e}"));
                    None
                }
            },
            2 => {
                let id = self.create_item(template, OBJECT_INVALID, 1);
                if let Some(o) = id.and_then(|id| self.get_mut(id)) {
                    o.position = loc.position;
                }
                id
            }
            64 => self.spawn_placeable(template, loc.position, bearing),
            32 => Some(self.spawn_waypoint(template, loc.position, bearing)),
            _ => {
                let id = self.alloc_id();
                let kind = match ty {
                    128 => ObjectKind::Store,
                    512 => ObjectKind::Sound,
                    4 => ObjectKind::Trigger,
                    8 => ObjectKind::Door,
                    _ => ObjectKind::Waypoint,
                };
                let mut obj = crate::Object::new(id, kind);
                obj.template = template.to_ascii_lowercase();
                obj.tag = template.to_owned();
                obj.position = loc.position;
                obj.bearing = bearing;
                self.objects.insert(id, obj);
                Some(id)
            }
        };
        if let Some(id) = id {
            self.emit(WorldEvent::ObjectSpawned(id));
        }
        id.unwrap_or(OBJECT_INVALID)
    }

    fn objects_in_shape(&self, size: f32, loc: Location, filter: i32, shape: i32) -> Vec<ObjectId> {
        let mut v: Vec<(f32, ObjectId)> = self
            .iter()
            .filter(|o| Self::type_filter(o, filter))
            .filter(|o| o.item().is_none_or(|i| i.possessor.is_none()))
            .filter_map(|o| {
                let d = o.position - loc.position;
                let inside = if shape == 2 { d.x.abs() <= size && d.y.abs() <= size } else { d.length() <= size };
                inside.then_some((d.length_squared(), o.id))
            })
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.into_iter().map(|(_, id)| id).collect()
    }

    fn next_in(cursor: &mut (Vec<ObjectId>, usize)) -> ObjectId {
        let r = cursor.0.get(cursor.1).copied().unwrap_or(OBJECT_INVALID);
        cursor.1 += 1;
        r
    }

    /// The big routine table. Returns None for routines with no
    /// implementation here.
    fn dispatch(&mut self, name: &str, me: ObjectId, a: &[Value], ret: VarType) -> Option<Value> {
        let v = match name {
            // ---------------------------------------------------------
            // Commands and actions
            // ---------------------------------------------------------
            "AssignCommand" => {
                if let Some(st) = action(a, 1) {
                    let target = obj(a, 0);
                    if self.is_valid(target) {
                        self.pending.push_back(Pending::Assign(target, st));
                    }
                }
                Value::Void
            }
            "DelayCommand" => {
                if let Some(st) = action(a, 1) {
                    let mut st = (*st).clone();
                    st.caller = me;
                    self.add_timer(float(a, 0), TimerKind::Resume(Arc::new(st)));
                }
                Value::Void
            }
            "ActionDoCommand" => {
                if let Some(st) = action(a, 0) {
                    self.push_action(me, Action::DoCommand(st));
                }
                Value::Void
            }
            "ClearAllActions" => {
                self.push_front_clear(me);
                Value::Void
            }
            "SetCommandable" => {
                let t = self.me_or(a, 1, me);
                if let Some(x) = self.get_mut(t) {
                    x.commandable = int(a, 0) != 0;
                }
                Value::Void
            }
            "GetCommandable" => {
                let t = self.me_or(a, 0, me);
                b(self.get(t).is_some_and(|x| x.commandable))
            }
            "ExecuteScript" => {
                let ctx = ScriptCtx { script_var: int(a, 2), ..self.ctx().clone() };
                let target = obj(a, 1);
                if self.is_valid(target) {
                    self.run_script(string(a, 0), target, ctx);
                }
                Value::Void
            }
            "GetRunScriptVar" => i(self.ctx().script_var),
            "ActionMoveToLocation" => {
                let l = self.loc_arg(a, 0);
                self.push_action(me, Action::MoveTo { target: Target::Point(l.position), run: int(a, 1) != 0, range: 0.5, timeout: None, elapsed: 0.0, stuck: 0.0 });
                Value::Void
            }
            "ActionForceMoveToLocation" => {
                let l = self.loc_arg(a, 0);
                let t = float(a, 2);
                self.push_action(me, Action::MoveTo { target: Target::Point(l.position), run: int(a, 1) != 0, range: 0.5, timeout: Some(if t > 0.0 { t } else { 30.0 }), elapsed: 0.0, stuck: 0.0 });
                Value::Void
            }
            "ActionMoveToObject" => {
                let r = if a.len() > 2 { float(a, 2) } else { 1.0 };
                self.push_action(me, Action::MoveTo { target: Target::Object(obj(a, 0)), run: int(a, 1) != 0, range: r.max(0.5), timeout: None, elapsed: 0.0, stuck: 0.0 });
                Value::Void
            }
            "ActionForceMoveToObject" => {
                let r = if a.len() > 2 { float(a, 2) } else { 1.0 };
                let t = float(a, 3);
                self.push_action(me, Action::MoveTo { target: Target::Object(obj(a, 0)), run: int(a, 1) != 0, range: r.max(0.5), timeout: Some(if t > 0.0 { t } else { 30.0 }), elapsed: 0.0, stuck: 0.0 });
                Value::Void
            }
            "ActionMoveAwayFromObject" => {
                let r = if a.len() > 2 { float(a, 2) } else { 40.0 };
                self.push_action(me, Action::MoveAway { from: obj(a, 0), run: int(a, 1) != 0, range: r, elapsed: 0.0 });
                Value::Void
            }
            "ActionMoveAwayFromLocation" => {
                Value::Void
            }
            "ActionJumpToObject" => {
                self.push_action(me, Action::JumpTo { target: Target::Object(obj(a, 0)), bearing: None });
                Value::Void
            }
            "ActionJumpToLocation" => {
                let l = self.loc_arg(a, 0);
                self.push_action(me, Action::JumpTo { target: Target::Point(l.position), bearing: Some(facing_to_bearing(l.facing)) });
                Value::Void
            }
            "JumpToLocation" => {
                let l = self.loc_arg(a, 0);
                self.place(me, l.position);
                if let Some(x) = self.get_mut(me) {
                    x.bearing = facing_to_bearing(l.facing);
                }
                Value::Void
            }
            "JumpToObject" => {
                let t = obj(a, 0);
                if let Some((p, bb)) = self.get(t).map(|x| (x.position, x.bearing)) {
                    self.place(me, p);
                    if let Some(x) = self.get_mut(me) {
                        x.bearing = bb;
                    }
                }
                Value::Void
            }
            "ActionWait" => {
                self.push_action(me, Action::Wait { remaining: float(a, 0) });
                Value::Void
            }
            "ActionPlayAnimation" => {
                let speed = if a.len() > 1 { float(a, 1) } else { 1.0 };
                self.push_action(me, Action::PlayAnimation { anim: int(a, 0), speed, duration: float(a, 2), started: false, remaining: 0.0 });
                Value::Void
            }
            "PlayAnimation" => {
                let anim = int(a, 0);
                let speed = if a.len() > 1 { float(a, 1) } else { 1.0 };
                self.emit(WorldEvent::PlayAnimation { object: me, animation: anim, speed, duration: float(a, 2), looping: crate::tick::is_looping_anim(anim) });
                Value::Void
            }
            "ActionOpenDoor" => {
                self.push_action(me, Action::OpenDoor(obj(a, 0)));
                Value::Void
            }
            "ActionCloseDoor" => {
                self.push_action(me, Action::CloseDoor(obj(a, 0)));
                Value::Void
            }
            "ActionLockObject" => {
                self.push_action(me, Action::Lock(obj(a, 0), true));
                Value::Void
            }
            "ActionUnlockObject" => {
                self.push_action(me, Action::Lock(obj(a, 0), false));
                Value::Void
            }
            "ActionStartConversation" => {
                // nBarkX / nBarkY (args 12, 13); -1 keeps the layout's position.
                let arg = |n: usize| a.get(n).and_then(Value::as_int).unwrap_or(-1);
                let bark = Some((arg(12), arg(13))).filter(|&(x, y)| x >= 0 && y >= 0);
                self.push_action(me, Action::StartConversation { with: obj(a, 0), dialog: string(a, 1).to_owned(), bark });
                Value::Void
            }
            "ActionPauseConversation" => {
                self.push_action(me, Action::PauseConversation);
                Value::Void
            }
            "ActionResumeConversation" => {
                self.push_action(me, Action::ResumeConversation);
                Value::Void
            }
            "ActionSpeakString" => {
                self.push_action(me, Action::Speak { text: string(a, 0).to_owned(), volume: int(a, 1) });
                Value::Void
            }
            "ActionSpeakStringByStrRef" | "ActionBarkString" => {
                let text = self.talk.as_ref().and_then(|t| t.get(int(a, 0) as u32)).unwrap_or("").to_owned();
                self.push_action(me, Action::Speak { text, volume: 0 });
                Value::Void
            }
            "SpeakString" => {
                let text = string(a, 0).to_owned();
                self.emit(WorldEvent::Speak { object: me, text: text.clone() });
                self.hear_speech(me, &text, int(a, 1));
                Value::Void
            }
            // oTokenTarget (the second argument) is ignored; see cutscenes.md.
            "SpeakOneLinerConversation" => {
                let dialog = match string(a, 0) {
                    "" => self.get(me).and_then(|o| o.conversation.clone()).unwrap_or_default(),
                    d => d.to_owned(),
                };
                self.speak_one_liner_conversation(me, &dialog);
                Value::Void
            }
            "BarkString" => {
                let text = self.talk.as_ref().and_then(|t| t.get(int(a, 1) as u32)).unwrap_or("").to_owned();
                self.emit(WorldEvent::Bark { object: obj(a, 0), text });
                Value::Void
            }
            "ActionAttack" => {
                self.push_action(me, Action::Attack(obj(a, 0)));
                Value::Void
            }
            "ActionTakeItem" => {
                self.give_item(obj(a, 0), me);
                Value::Void
            }
            "ActionGiveItem" | "GiveItem" => {
                self.give_item(obj(a, 0), obj(a, 1));
                Value::Void
            }
            "ActionSwitchWeapons" => {
                self.push_action(me, Action::SwitchWeapons);
                Value::Void
            }
            "ActionFollowLeader" => {
                self.queue_follow(me, FollowTarget::Leader, crate::nav::FOLLOW_STOP);
                Value::Void
            }
            "ActionFollowOwner" => {
                // The VM fills nwscript's default fRange = 2.5 for a call that omits it.
                self.queue_follow(me, FollowTarget::Owner, float(a, 0));
                Value::Void
            }
            "ActionForceFollowObject" => {
                // (oFollow, fFollowDistance); nwscript default distance 0.0.
                self.queue_follow(me, FollowTarget::Object(obj(a, 0)), float(a, 1).max(0.0));
                Value::Void
            }
            // ActionUseSkill: craft (security), stealth and traps (demolitions on a
            // trap) answer first, so only the other skills reach this no-op.
            "ActionUseSkill" | "ActionEquipMostEffectiveArmor" | "ActionPickUpItem" | "ActionPutDownItem"
            | "ActionRandomWalk" | "ActionInteractObject" | "ActionSurrenderToEnemies" => {
                self.push_action(me, Action::Noop("unsimulated".into()));
                Value::Void
            }
            "GetCurrentAction" => {
                let t = self.me_or(a, 0, me);
                let n = self.get(t).and_then(|x| x.actions.front()).map(|x| x.constant_name()).unwrap_or("ACTION_INVALID");
                i(self.constant(n).unwrap_or(65535))
            }
            "GetUserActionsPending" | "GetCombatActionsPending" => b(false),

            // ---------------------------------------------------------
            // Objects and lookup
            // ---------------------------------------------------------
            "GetIsObjectValid" => b(self.is_valid(obj(a, 0))),
            "GetObjectByTag" => o_opt(self.by_tag(string(a, 0), int(a, 1).max(0) as usize)),
            "GetWaypointByTag" => {
                let tag = string(a, 0);
                o_opt(self.iter().find(|x| x.kind == ObjectKind::Waypoint && x.tag.eq_ignore_ascii_case(tag)).map(|x| x.id))
            }
            "GetNearestObjectByTag" => {
                let tag = string(a, 0).to_owned();
                let from = self.me_or(a, 1, me);
                let nth = int(a, 2).max(1) as usize;
                o_opt(self.nearest(from, |x| x.tag.eq_ignore_ascii_case(&tag)).get(nth - 1).copied())
            }
            "GetNearestObject" => {
                let filter = if a.is_empty() { 32767 } else { int(a, 0) };
                let from = self.me_or(a, 1, me);
                let nth = int(a, 2).max(1) as usize;
                o_opt(self.nearest(from, |x| Self::type_filter(x, filter)).get(nth - 1).copied())
            }
            "GetNearestObjectToLocation" => {
                let filter = int(a, 0);
                let l = self.loc_arg(a, 1);
                let nth = int(a, 2).max(1) as usize;
                o_opt(self.objects_in_shape(1e6, l, filter, 4).get(nth - 1).copied())
            }
            "GetNearestCreature" => {
                let from = self.me_or(a, 2, me);
                let nth = int(a, 3).max(1) as usize;
                let criteria = [(int(a, 0), int(a, 1)), (if a.len() > 4 { int(a, 4) } else { -1 }, int(a, 5)), (if a.len() > 6 { int(a, 6) } else { -1 }, int(a, 7))];
                let party = self.party.clone();
                let leader = self.party_leader();
                let mut list = Vec::new();
                for id in self.nearest(from, |x| x.kind == ObjectKind::Creature) {
                    let ok = criteria.iter().all(|&(ty, val)| match ty {
                        1 => (id == leader) == (val == 0),
                        4 => self.is_dead(id) != (val != 0),
                        3 => {
                            let hostile = self.is_hostile(id, from);
                            match val {
                                0 => !hostile && !party.contains(&id) || hostile,
                                1 => hostile,
                                2 => party.contains(&id) || !hostile,
                                _ => true,
                            }
                        }
                        7 => self.distance(id, from).is_some_and(|d| d < 20.0),
                        _ => true,
                    });
                    if ok {
                        list.push(id);
                    }
                }
                o_opt(list.get(nth - 1).copied())
            }
            "GetTag" => s(self.get(obj(a, 0)).map(|x| x.tag.clone()).unwrap_or_default()),
            "GetName" => s(self.get(obj(a, 0)).map(|x| x.name.clone()).unwrap_or_default()),
            "GetObjectType" => i(self.get(obj(a, 0)).map_or(32767, |x| x.kind.type_bit())),
            "GetArea" => o(if self.is_valid(obj(a, 0)) { self.area_id } else { OBJECT_INVALID }),
            "GetModule" => o(self.module_id),
            "GetModuleName" => s(self.module.as_ref().map(|m| m.name.to_ascii_uppercase()).unwrap_or_default()),
            "GetPosition" => vec(self.pos(obj(a, 0)).unwrap_or(Vec3::ZERO)),
            "GetFacing" => f(self.get(obj(a, 0)).map_or(0.0, |x| x.facing_deg())),
            "SetFacing" => {
                if let Some(x) = self.get_mut(me) {
                    x.bearing = facing_to_bearing(float(a, 0));
                }
                Value::Void
            }
            "SetFacingPoint" => {
                let target = vector(a, 0);
                if let Some(x) = self.get_mut(me)
                    && let Some(bb) = bearing_towards(x.position, target)
                {
                    x.bearing = bb;
                }
                Value::Void
            }
            "GetDistanceBetween" => f(self.distance(obj(a, 0), obj(a, 1)).unwrap_or(0.0)),
            "GetDistanceBetween2D" => f(match (self.pos(obj(a, 0)), self.pos(obj(a, 1))) {
                (Some(p), Some(q)) => p.truncate().distance(q.truncate()),
                _ => 0.0,
            }),
            "GetDistanceToObject" => f(self.distance(me, obj(a, 0)).unwrap_or(-1.0)),
            "GetDistanceToObject2D" => f(match (self.pos(me), self.pos(obj(a, 0))) {
                (Some(p), Some(q)) => p.truncate().distance(q.truncate()),
                _ => -1.0,
            }),
            "GetDistanceBetweenLocations" => f(self.loc_arg(a, 0).position.distance(self.loc_arg(a, 1).position)),
            "GetDistanceBetweenLocations2D" => f(self.loc_arg(a, 0).position.truncate().distance(self.loc_arg(a, 1).position.truncate())),
            "GetObjectPersonalSpace" => f(self.creature_state(obj(a, 0)).map_or(0.5, |c| c.radius)),
            "ObjectToString" => s(format!("{:x}", obj(a, 0))),
            "GetStringByStrRef" => s(self.talk.as_ref().and_then(|t| t.get(int(a, 0) as u32)).unwrap_or("")),
            "GetFirstObjectInArea" => {
                let filter = if a.len() > 1 { int(a, 1) } else { 1 };
                let list: Vec<ObjectId> = self
                    .iter()
                    .filter(|x| Self::type_filter(x, filter) && x.item().is_none_or(|i| i.possessor.is_none()))
                    .map(|x| x.id)
                    .collect();
                self.iters.area = (list, 0);
                o(Self::next_in(&mut self.iters.area))
            }
            "GetNextObjectInArea" => o(Self::next_in(&mut self.iters.area)),
            "GetFirstObjectInShape" => {
                let filter = if a.len() > 4 { int(a, 4) } else { 1 };
                let l = self.loc_arg(a, 2);
                self.iters.shape = (self.objects_in_shape(float(a, 1), l, filter, int(a, 0)), 0);
                o(Self::next_in(&mut self.iters.shape))
            }
            "GetNextObjectInShape" => o(Self::next_in(&mut self.iters.shape)),
            "GetFirstInPersistentObject" => {
                let t = self.me_or(a, 0, me);
                let filter = if a.len() > 1 { int(a, 1) } else { 1 };
                let poly = self.get(t).and_then(|x| x.trigger()).map(|t| t.polygon.clone()).unwrap_or_default();
                let list: Vec<ObjectId> = self
                    .iter()
                    .filter(|x| Self::type_filter(x, filter) && k2_data::module::point_in_polygon(&poly, x.position.x, x.position.y))
                    .map(|x| x.id)
                    .collect();
                self.iters.persistent = (list, 0);
                o(Self::next_in(&mut self.iters.persistent))
            }
            "GetNextInPersistentObject" => o(Self::next_in(&mut self.iters.persistent)),
            "GetRandomDestination" => {
                let p = self.pos(obj(a, 0)).unwrap_or(Vec3::ZERO);
                let r = int(a, 1).max(1) as f32;
                let mut out = p;
                for _ in 0..8 {
                    let ang = self.builtins.random(360) as f32 * std::f32::consts::PI / 180.0;
                    let d = self.builtins.random((r * 100.0) as i32) as f32 / 100.0;
                    let q = p + Vec3::new(ang.cos() * d, ang.sin() * d, 0.0);
                    if let Some(h) = self.walk.ground(q.x, q.y, q.z + 1.0) {
                        out = Vec3::new(q.x, q.y, h.z);
                        break;
                    }
                }
                vec(out)
            }
            "HasLineOfSight" => b(self.walk.line_of_sight(vector(a, 0) + Vec3::Z, vector(a, 1) + Vec3::Z)),
            "GetObjectSeen" | "GetObjectHeard" => {
                let src = self.me_or(a, 1, me);
                b(self.can_see(src, obj(a, 0)))
            }

            // ---------------------------------------------------------
            // Locations
            // ---------------------------------------------------------
            "GetLocation" => {
                let l = self.location_of(obj(a, 0));
                self.engine.add(EngineData::Location(l))
            }
            "Location" => self.engine.add(EngineData::Location(Location { position: vector(a, 0), facing: float(a, 1), area: self.area_id })),
            "GetPositionFromLocation" => vec(self.loc_arg(a, 0).position),
            "GetFacingFromLocation" => f(self.loc_arg(a, 0).facing),
            "GetGlobalLocation" => {
                let l = self.globals.location(string(a, 0)).unwrap_or_default();
                self.engine.add(EngineData::Location(l))
            }
            "SetGlobalLocation" => {
                let l = self.loc_arg(a, 1);
                self.globals.set_location(string(a, 0), l);
                Value::Void
            }

            // ---------------------------------------------------------
            // Variables
            // ---------------------------------------------------------
            "GetGlobalNumber" => i(self.globals.number(string(a, 0))),
            "SetGlobalNumber" => {
                self.globals.set_number(string(a, 0), int(a, 1));
                Value::Void
            }
            "IncrementGlobalNumber" | "DecrementGlobalNumber" => {
                let n = string(a, 0).to_owned();
                let d = if name == "IncrementGlobalNumber" { int(a, 1) } else { -int(a, 1) };
                let v = self.globals.number(&n) + d;
                self.globals.set_number(&n, v);
                Value::Void
            }
            "GetGlobalBoolean" => b(self.globals.boolean(string(a, 0))),
            "SetGlobalBoolean" => {
                self.globals.set_boolean(string(a, 0), int(a, 1) != 0);
                Value::Void
            }
            "GetGlobalString" => s(self.globals.string(string(a, 0))),
            "SetGlobalString" => {
                self.globals.set_string(string(a, 0), string(a, 1).to_owned());
                Value::Void
            }
            "GetLocalBoolean" => b(self.get(obj(a, 0)).is_some_and(|x| x.local_bools.get(&int(a, 1)).copied().unwrap_or(false))),
            "SetLocalBoolean" => {
                let (idx, val) = (int(a, 1), int(a, 2) != 0);
                if let Some(x) = self.get_mut(obj(a, 0)) {
                    x.local_bools.insert(idx, val);
                }
                Value::Void
            }
            "GetLocalNumber" => i(self.get(obj(a, 0)).and_then(|x| x.local_numbers.get(&int(a, 1)).copied()).unwrap_or(0)),
            "SetLocalNumber" => {
                let (idx, val) = (int(a, 1), int(a, 2));
                if let Some(x) = self.get_mut(obj(a, 0)) {
                    x.local_numbers.insert(idx, val);
                }
                Value::Void
            }
            "SetCustomToken" => {
                self.tokens.insert(int(a, 0), string(a, 1).to_owned());
                Value::Void
            }

            // ---------------------------------------------------------
            // Script context
            // ---------------------------------------------------------
            "GetEnteringObject" => o(self.ctx().entering),
            "GetExitingObject" => o(self.ctx().exiting),
            "GetClickingObject" => o(self.ctx().clicking),
            "GetLastUsedBy" => o(self.ctx().last_used_by),
            "GetLastOpenedBy" => o(self.ctx().last_opened_by),
            "GetLastClosedBy" => o(self.ctx().last_closed_by),
            "GetLastSpeaker" => o(self.ctx().last_speaker),
            "GetPCSpeaker" => {
                let c = self.conversation.as_ref().map(|c| c.pc);
                o(c.unwrap_or(if self.ctx().last_speaker != OBJECT_INVALID { self.ctx().last_speaker } else { self.party_leader() }))
            }
            "GetListenPatternNumber" => i(self.ctx().listen_pattern),
            "GetUserDefinedEventNumber" => i(self.ctx().user_event),
            "GetLastPerceived" => o(self.ctx().last_perceived),
            "GetLastPerceptionSeen" => b(self.ctx().perception_seen),
            "GetLastPerceptionVanished" => b(self.ctx().last_perceived != OBJECT_INVALID && !self.ctx().perception_seen),
            "GetLastPerceptionHeard" | "GetLastPerceptionInaudible" => b(false),
            "GetScriptParameter" => i(self.ctx().params.get((int(a, 0) - 1).clamp(0, 4) as usize).copied().unwrap_or(0)),
            "GetScriptStringParameter" => s(self.ctx().string_param.clone()),
            "GetLoadFromSaveGame" | "GetIsXBox" => b(false),

            // ---------------------------------------------------------
            // Events
            // ---------------------------------------------------------
            "SignalEvent" => {
                let target = obj(a, 0);
                if let Some(ev) = a.get(1).and_then(|v| self.engine.event(v)).cloned() {
                    match ev {
                        Event::UserDefined(n) => {
                            let label = self.user_defined_label(target);
                            self.queue_event(target, label, ScriptCtx { user_event: n, ..Default::default() });
                        }
                        Event::Other(_) => {}
                    }
                }
                Value::Void
            }
            "EventUserDefined" => self.engine.add(EngineData::Event(Event::UserDefined(int(a, 0)))),

            // ---------------------------------------------------------
            // Creation and destruction
            // ---------------------------------------------------------
            "CreateObject" => {
                let l = self.loc_arg(a, 2);
                o(self.create_object(int(a, 0), string(a, 1), l))
            }
            "CreateItemOnObject" => {
                let target = self.me_or(a, 1, me);
                let stack = if a.len() > 2 { int(a, 2) } else { 1 };
                o_opt(self.create_item(string(a, 0), target, stack))
            }
            "DestroyObject" => {
                let t = obj(a, 0);
                let delay = float(a, 1);
                if delay > 0.0 {
                    self.add_timer(delay, TimerKind::Destroy(t));
                } else {
                    self.destroy(t);
                }
                Value::Void
            }
            "SetPlotFlag" => {
                if let Some(x) = self.get_mut(obj(a, 0)) {
                    x.plot = int(a, 1) != 0;
                }
                Value::Void
            }
            "GetPlotFlag" => {
                let t = self.me_or(a, 0, me);
                b(self.get(t).is_some_and(|x| x.plot))
            }
            "SetLockOrientationInDialog" => {
                let lock = int(a, 1) != 0;
                if let Some(x) = self.get_mut(obj(a, 0)) {
                    x.local_bools.insert(crate::dialog::LOCK_ORIENTATION_LOCAL, lock);
                }
                Value::Void
            }
            "RemoveHeartbeat" => {
                // Drops the object's OnHeartbeat script (nwscript 866).
                if let Some(o) = self.get_mut(obj(a, 0)) {
                    o.scripts.remove("OnHeartbeat");
                }
                Value::Void
            }
            "ClearAllEffects" => {
                let gone: Vec<u32> = self.get(me).map_or_else(Vec::new, |o| o.effects.iter().map(|ae| ae.handle).collect());
                if let Some(o) = self.get_mut(me) {
                    o.effects.clear();
                }
                self.forget_effect_handles(&gone);
                Value::Void
            }
            "SetAreaUnescapable" => {
                self.area_unescapable_override = Some(int(a, 0) != 0);
                Value::Void
            }
            "SetDisableTransit" => {
                self.transit_disabled = int(a, 0) != 0;
                Value::Void
            }
            "SetIsDestroyable" | "SetOrientOnClick" | "SetLightsaberPowered" | "CreatureFlourishWeapon"
            | "SetFakeCombatState" | "SetForceAlwaysUpdate" | "SetCreatureAILevel" | "CancelCombat"
            | "SetLockHeadFollowInDialog" | "SetDialogPlaceableCamera" | "SetKeepStealthInDialog"
            | "ResetDialogState" | "SetListenPattern" | "SetForfeitConditions" | "SurrenderToEnemies"
            | "AdjustCreatureAttributes" | "GiveXPToCreature" | "GivePlotXP" | "GrantSpell" | "MusicBackgroundPlay"
            | "MusicBackgroundStop" | "MusicBackgroundChangeDay" | "MusicBackgroundChangeNight" | "AmbientSoundPlay"
            | "AmbientSoundStop" | "SetTime" | "SetPlayerRestrictMode" | "EnableVideoEffect" | "DisableVideoEffect"
            | "SetHealTarget" | "SetNPCAIStyle" | "SetPartyAIStyle" | "ShowTutorialWindow" | "SetInputClass"
            | "SetMinigameVars" | "AddBonusForcePoints" | "SetButtonMashCheck" | "DisplayFeedBackText"
            | "SetGoodEvilValue" | "SetMaxHitPoints" | "AssignPUP" | "SetAvailableNPCId" | "SetCameraMode"
            | "AdjustReputation" | "SetStealthMode" | "EnableRendering" | "SetSoloMode"
            | "RevealMap" | "DisableMap" | "SetMapPinEnabled" | "SetBonusForcePoints" | "SetCurrentStealthXP" => {
                match name {
                    "SetListenPattern" => {
                        let (t, pat, n) = (obj(a, 0), string(a, 1).to_owned(), int(a, 2));
                        if let Some(c) = self.get_mut(t).and_then(|x| x.creature_mut()) {
                            // One pattern per string: setting it again replaces its number.
                            c.listen_patterns.retain(|(p, _)| !p.eq_ignore_ascii_case(&pat));
                            c.listen_patterns.push((pat, n));
                        }
                    }
                    "SetGoodEvilValue" => {
                        let (t, v) = (obj(a, 0), int(a, 1));
                        if let Some(c) = self.get_mut(t).and_then(|x| x.creature_mut()) {
                            c.good_evil = v.clamp(0, 100);
                        }
                    }
                    "SetMaxHitPoints" => {
                        let (t, v) = (obj(a, 0), int(a, 1));
                        if let Some(c) = self.get_mut(t).and_then(|x| x.creature_mut()) {
                            c.max_hp = v.max(1);
                            c.hp = c.hp.min(c.max_hp);
                        }
                    }
                    "SetSoloMode" => self.solo_mode = int(a, 0) != 0,
                    _ => {}
                }
                Value::default_for(ret)
            }
            "GetIsListening" => b(self.get(obj(a, 0)).and_then(|x| x.creature()).is_some_and(|c| c.listening)),
            "SetListening" => {
                let (t, v) = (obj(a, 0), int(a, 1) != 0);
                if let Some(c) = self.get_mut(t).and_then(|x| x.creature_mut()) {
                    c.listening = v;
                }
                Value::Void
            }

            // ---------------------------------------------------------
            // Doors, placeables, locks
            // ---------------------------------------------------------
            "GetIsOpen" => b(self.get(obj(a, 0)).is_some_and(|x| match &x.detail {
                Detail::Door(d) => d.open,
                Detail::Placeable(p) => p.open,
                _ => false,
            })),
            "GetLocked" => b(self.get(obj(a, 0)).is_some_and(|x| match &x.detail {
                Detail::Door(d) => d.locked,
                Detail::Placeable(p) => p.locked,
                _ => false,
            })),
            "SetLocked" => {
                self.set_locked(obj(a, 0), int(a, 1) != 0);
                Value::Void
            }
            "GetLockKeyRequired" => b(self.get(obj(a, 0)).and_then(|x| x.door()).is_some_and(|d| d.key_required)),
            "GetLockKeyTag" => s(self.get(obj(a, 0)).and_then(|x| x.door()).map(|d| d.key_name.clone()).unwrap_or_default()),
            "PlayRoomAnimation" => {
                self.emit(WorldEvent::DebugText(format!("room {} anim {}", string(a, 0), int(a, 1))));
                Value::Void
            }

            // ---------------------------------------------------------
            // Party and the PC
            // ---------------------------------------------------------
            "GetFirstPC" => {
                self.iters.pc = 1;
                o(self.party_leader())
            }
            "GetNextPC" => {
                let idx = self.iters.pc;
                self.iters.pc += 1;
                o_opt(self.party.get(idx).copied())
            }
            "GetIsPC" => b(obj(a, 0) == self.party_leader()),
            "GetPartyLeader" => o(self.party_leader()),
            "GetIsPartyLeader" => {
                let t = self.me_or(a, 0, me);
                b(t == self.party_leader())
            }
            "GetPartyMemberByIndex" => o_opt(self.party.get(int(a, 0).max(0) as usize).copied()),
            "GetPartyMemberCount" => i(self.party.len() as i32),
            "IsObjectPartyMember" => b(self.party.contains(&obj(a, 0))),
            "IsNPCPartyMember" => b(self.npc_object(int(a, 0)).is_some_and(|id| self.party.contains(&id))),
            "IsAvailableCreature" => b(self.available_npcs.contains_key(&int(a, 0))),
            "AddAvailableNPCByTemplate" => {
                self.available_npcs.insert(int(a, 0), AvailableNpc { template: string(a, 1).to_ascii_lowercase(), object: None, selectable: true, stored: None });
                b(true)
            }
            "AddAvailableNPCByObject" => {
                let id = obj(a, 1);
                let template = self.get(id).map(|x| x.template.clone()).unwrap_or_default();
                self.available_npcs.insert(int(a, 0), AvailableNpc { template, object: Some(id), selectable: true, stored: None });
                b(true)
            }
            "RemoveAvailableNPC" => {
                let n = int(a, 0);
                if let Some(id) = self.npc_object(n) {
                    self.party.retain(|&p| p != id);
                }
                b(self.available_npcs.remove(&n).is_some())
            }
            "AddPartyMember" => {
                let (n, id) = (int(a, 0), obj(a, 1));
                if self.is_valid(id) {
                    let template = self.get(id).map(|x| x.template.clone()).unwrap_or_default();
                    let e = self.available_npcs.entry(n).or_insert(AvailableNpc { template, object: None, selectable: true, stored: None });
                    e.object = Some(id);
                    if !self.party.contains(&id) {
                        self.party.push(id);
                    }
                    b(true)
                } else {
                    b(false)
                }
            }
            "RemovePartyMember" | "RemoveNPCFromPartyToBase" => {
                let n = int(a, 0);
                if let Some(id) = self.npc_object(n)
                    && id != self.player
                {
                    self.party.retain(|&p| p != id);
                }
                b(true)
            }
            "SpawnAvailableNPC" => {
                let l = self.loc_arg(a, 1);
                o(self.spawn_npc(int(a, 0), l))
            }
            "SetPartyLeader" => {
                let n = int(a, 0);
                if let Some(id) = self.npc_object(n).filter(|id| self.party.contains(id)) {
                    self.party.retain(|&p| p != id);
                    self.party.insert(0, id);
                    self.drop_follows(id);
                    b(true)
                } else {
                    b(false)
                }
            }
            "SwitchPlayerCharacter" => b(self.switch_player_character(int(a, 0))),
            "GetNPCSelectability" => b(self.available_npcs.get(&int(a, 0)).is_some_and(|n| n.selectable)),
            "SetNPCSelectability" => {
                if let Some(n) = self.available_npcs.get_mut(&int(a, 0)) {
                    n.selectable = int(a, 1) != 0;
                }
                Value::Void
            }
            "GetSoloMode" => b(self.solo_mode),
            "GetInfluence" => i(self.influence.get(&int(a, 0)).copied().unwrap_or(50)),
            "ModifyInfluence" => {
                let n = int(a, 0);
                let v = self.influence.get(&n).copied().unwrap_or(50) + int(a, 1);
                self.influence.insert(n, v.clamp(0, 100));
                Value::Void
            }
            "GetIsPuppet" => b(false),
            "GetPUPOwner" => {
                // nwscript 841: the owner must exist and be in the party.
                let t = self.me_or(a, 0, me);
                o(self.get(t).and_then(|x| x.owner).filter(|&w| self.is_valid(w) && self.is_party_member(w)).unwrap_or(OBJECT_INVALID))
            }
            "ShowPartySelectionGUI" => {
                // The screen answers through World::apply_party_selection
                // (party.rs), which then runs the exit script.
                let forced = |n: usize| a.get(n).and_then(Value::as_int).unwrap_or(-1);
                self.party_select = Some(crate::party::PartySelectRequest {
                    exit_script: string(a, 0).to_owned(),
                    forced: [forced(1), forced(2)],
                    allow_cancel: a.get(3).and_then(Value::as_int).unwrap_or(0) != 0,
                });
                self.emit(WorldEvent::ShowGui("partyselection".into()));
                Value::Void
            }

            // ---------------------------------------------------------
            // Creature stats
            // ---------------------------------------------------------
            "GetCurrentHitPoints" => {
                let t = self.me_or(a, 0, me);
                i(self.creature_state(t).map(|c| c.hp).or_else(|| self.object_hp(t).map(|h| h.current)).unwrap_or(0))
            }
            "GetMaxHitPoints" => {
                let t = self.me_or(a, 0, me);
                i(self.creature_state(t).map(|c| c.max_hp).or_else(|| self.object_hp(t).map(|h| h.max)).unwrap_or(0))
            }
            "GetCurrentForcePoints" => {
                let t = self.me_or(a, 0, me);
                i(self.creature_state(t).map_or(0, |c| c.force))
            }
            "GetMaxForcePoints" => {
                let t = self.me_or(a, 0, me);
                i(self.creature_state(t).map_or(0, |c| c.max_force))
            }
            "GetIsDead" => b(self.is_dead(obj(a, 0))),
            "SetMinOneHP" => {
                let (t, v) = (obj(a, 0), int(a, 1) != 0);
                if let Some(c) = self.get_mut(t).and_then(|x| x.creature_mut()) {
                    c.min1hp = v;
                }
                Value::Void
            }
            "GetMinOneHP" => b(self.creature_state(obj(a, 0)).is_some_and(|c| c.min1hp)),
            "GetGender" => i(self.creature_state(obj(a, 0)).map_or(0, |c| c.gender)),
            "GetRacialType" => i(self.creature_state(obj(a, 0)).map_or(6, |c| c.racial_type)),
            "GetAppearanceType" => i(self.creature_state(obj(a, 0)).map_or(0, |c| c.appearance as i32)),
            "GetHitDice" => i(self.creature_state(obj(a, 0)).map_or(0, |c| c.classes.iter().map(|x| x.1).sum::<i32>().max(1))),
            "GetLevelByClass" => {
                let t = self.me_or(a, 1, me);
                let cls = int(a, 0);
                i(self.creature_state(t).map_or(0, |c| c.classes.iter().filter(|x| x.0 == cls).map(|x| x.1).sum()))
            }
            "GetClassByPosition" => {
                let t = self.me_or(a, 1, me);
                let p = int(a, 0).max(1) as usize - 1;
                i(self.creature_state(t).and_then(|c| c.classes.get(p)).map_or(255, |x| x.0))
            }
            "GetLevelByPosition" => {
                let t = self.me_or(a, 1, me);
                let p = int(a, 0).max(1) as usize - 1;
                i(self.creature_state(t).and_then(|c| c.classes.get(p)).map_or(0, |x| x.1))
            }
            "GetAbilityScore" => {
                let ab = int(a, 1).clamp(0, 5) as usize;
                i(self.creature_state(obj(a, 0)).map_or(10, |c| c.abilities[ab] as i32))
            }
            "GetAbilityModifier" => {
                let t = self.me_or(a, 1, me);
                let ab = int(a, 0).clamp(0, 5) as usize;
                i(self.creature_state(t).map_or(0, |c| (c.abilities[ab] as i32 - 10).div_euclid(2)))
            }
            "GetGoodEvilValue" => i(self.creature_state(obj(a, 0)).map_or(50, |c| c.good_evil)),
            "GetAlignmentGoodEvil" => {
                let v = self.creature_state(obj(a, 0)).map_or(50, |c| c.good_evil);
                i(if v >= 70 { 2 } else if v <= 30 { 3 } else { 1 })
            }
            "AdjustAlignment" => {
                let (t, al, shift) = (obj(a, 0), int(a, 1), int(a, 2));
                let delta = match al {
                    2 => shift,
                    3 => -shift,
                    _ => 0,
                };
                if let Some(c) = self.get_mut(t).and_then(|x| x.creature_mut()) {
                    c.good_evil = (c.good_evil + delta).clamp(0, 100);
                }
                Value::Void
            }
            "GetStandardFaction" => i(self.faction(obj(a, 0))),
            "ChangeToStandardFaction" => {
                let (t, fac) = (obj(a, 0), int(a, 1));
                if let Some(c) = self.get_mut(t).and_then(|x| x.creature_mut()) {
                    c.faction = fac;
                }
                Value::Void
            }
            "ChangeFaction" => {
                let fac = self.faction(obj(a, 1));
                if let Some(c) = self.get_mut(obj(a, 0)).and_then(|x| x.creature_mut()) {
                    c.faction = fac;
                }
                Value::Void
            }
            "GetIsEnemy" => {
                let src = self.me_or(a, 1, me);
                b(self.is_hostile(obj(a, 0), src))
            }
            "GetIsFriend" => {
                let src = self.me_or(a, 1, me);
                let t = obj(a, 0);
                b(!self.is_hostile(t, src) && self.faction(t) != 5 && self.faction(src) != 5)
            }
            "GetIsNeutral" => {
                let src = self.me_or(a, 1, me);
                let t = obj(a, 0);
                b(!self.is_hostile(t, src) && (self.faction(t) == 5 || self.faction(src) == 5))
            }
            "GetIsInCombat" | "GetIsDebilitated" | "IsStealthed" | "GetHasSpell" | "GetHasFeat"
            | "GetHasSkill" | "GetHasSpellEffect" | "GetSpellAcquired" | "GetFeatAcquired" | "GetIsInConversation" => {
                if name == "GetIsInConversation" {
                    let t = self.me_or(a, 0, me);
                    b(self.conversation.as_ref().is_some_and(|c| c.owner == t || c.pc == t))
                } else {
                    b(false)
                }
            }
            "GetSkillRank" => i(0),
            "GetGold" => i(self.party_gold()),
            "GiveGoldToCreature" => {
                self.add_gold(int(a, 1));
                Value::Void
            }
            "TakeGoldFromCreature" => {
                self.add_gold(-int(a, 0));
                Value::Void
            }

            // ---------------------------------------------------------
            // Items
            // ---------------------------------------------------------
            "GetItemPossessedBy" => {
                let tag = string(a, 1).to_owned();
                let owner = obj(a, 0);
                let items = self.carried(owner);
                o_opt(items.into_iter().find(|&it| self.get(it).is_some_and(|x| x.tag.eq_ignore_ascii_case(&tag))))
            }
            "GetItemInSlot" => {
                let t = self.me_or(a, 1, me);
                o_opt(self.creature_state(t).and_then(|c| c.equipped.get(&int(a, 0)).copied()))
            }
            "GetFirstItemInInventory" => {
                let t = self.me_or(a, 0, me);
                let list = self.get(t).map(|x| x.inventory.clone()).unwrap_or_default();
                let mut cur = (list, 0);
                let r = Self::next_in(&mut cur);
                self.iters.inventory.insert(t, cur);
                o(r)
            }
            "GetNextItemInInventory" => {
                let t = self.me_or(a, 0, me);
                o(self.iters.inventory.get_mut(&t).map_or(OBJECT_INVALID, Self::next_in))
            }
            "GetItemStackSize" | "GetNumStackedItems" => i(self.get(obj(a, 0)).and_then(|x| x.item()).map_or(0, |it| it.stack_size)),
            "SetItemStackSize" => {
                let (t, n) = (obj(a, 0), int(a, 1));
                if let Some(it) = self.get_mut(t).and_then(|x| x.item_mut()) {
                    it.stack_size = n.max(1);
                }
                Value::Void
            }
            "GetBaseItemType" => i(self.get(obj(a, 0)).and_then(|x| x.item()).map_or(-1, |it| it.base_item as i32)),
            "GetItemPossessor" => o_opt(self.get(obj(a, 0)).and_then(|x| x.item()).and_then(|it| it.possessor)),
            "OpenStore" => {
                self.emit(WorldEvent::OpenStore { store: obj(a, 0) });
                Value::Void
            }

            // ---------------------------------------------------------
            // Effects and talents
            // ---------------------------------------------------------
            "ApplyEffectToObject" => {
                let target = obj(a, 2);
                if let Some(e) = a.get(1).cloned() {
                    self.apply_effect(&e, target, int(a, 0), float(a, 3));
                }
                Value::Void
            }
            "ApplyEffectAtLocation" => {
                if let Some(e) = a.get(1).and_then(|v| self.engine.effect(v)).cloned()
                    && e.name == "EffectVisualEffect"
                {
                    self.emit(WorldEvent::VisualEffect { object: OBJECT_INVALID, effect: int(&e.args, 0) });
                }
                Value::Void
            }
            "GetFirstEffect" | "GetNextEffect" => {
                let t = obj(a, 0);
                let idx = if name == "GetFirstEffect" { 0 } else { self.iters.effects.get(&t).copied().unwrap_or(0) };
                let h = self.get(t).and_then(|x| x.effects.get(idx)).map(|e| e.handle);
                self.iters.effects.insert(t, idx + 1);
                match h {
                    Some(h) if h > 0 => Value::Engine(k2_script::EngineValue { kind: ENGINE_EFFECT, handle: h }),
                    _ => Value::Engine(k2_script::EngineValue { kind: ENGINE_EFFECT, handle: 0 }),
                }
            }
            "GetIsEffectValid" => b(a.first().and_then(|v| self.engine.effect(v)).is_some()),
            "GetEffectType" => i(a.first().and_then(|v| self.engine.effect(v)).map_or(0, |e| e.ty)),
            "GetEffectCreator" => o(a.first().and_then(|v| self.engine.effect(v)).map_or(OBJECT_INVALID, |e| e.creator)),
            "RemoveEffect" => {
                let t = obj(a, 0);
                if let Some(Value::Engine(e)) = a.get(1) {
                    let h = e.handle;
                    if let Some(x) = self.get_mut(t) {
                        x.effects.retain(|q| q.handle != h);
                    }
                    self.forget_effect_handles(&[h]);
                }
                Value::Void
            }
            "GetIsTalentValid" => b(matches!(a.first().and_then(|v| self.engine.get(v)), Some(EngineData::Talent(_)))),

            // ---------------------------------------------------------
            // Dialog
            // ---------------------------------------------------------
            "BeginConversation" => {
                let dlg = string(a, 0);
                let dlg = if dlg.is_empty() { self.get(me).and_then(|x| x.conversation.clone()).unwrap_or_default() } else { dlg.to_owned() };
                let with = obj(a, 1);
                let pc = if self.is_valid(with) {
                    with
                } else if self.is_valid(self.ctx().last_speaker) {
                    self.ctx().last_speaker
                } else {
                    self.party_leader()
                };
                b(self.start_conversation(me, pc, &dlg))
            }
            "GetIsConversationActive" => b(self.conversation.is_some()),
            "GetLastConversation" => s(self.conversation.as_ref().map(|c| c.dialog.resref.clone()).unwrap_or_default()),

            // ---------------------------------------------------------
            // Presentation
            // ---------------------------------------------------------
            "SetGlobalFadeOut" | "SetGlobalFadeIn" => {
                let (wait, length) = (float(a, 0), float(a, 1));
                let color = [float(a, 2), float(a, 3), float(a, 4)];
                self.emit(if name == "SetGlobalFadeOut" {
                    WorldEvent::FadeOut { wait, length, color }
                } else {
                    WorldEvent::FadeIn { wait, length, color }
                });
                Value::Void
            }
            "SetFadeUntilScript" => {
                self.emit(WorldEvent::FadeOut { wait: 0.0, length: 0.0, color: [0.0; 3] });
                Value::Void
            }
            "NoClicksFor" => {
                self.input_blocked_for = self.input_blocked_for.max(float(a, 0));
                Value::Void
            }
            "PlayMovie" | "QueueMovie" => {
                self.emit(WorldEvent::Movie(string(a, 0).to_ascii_lowercase()));
                Value::Void
            }
            "PlayMovieQueue" => Value::Void,
            "AurPostString" => {
                self.emit(WorldEvent::DebugText(string(a, 0).to_owned()));
                Value::Void
            }
            "PlaySound" => {
                self.emit(WorldEvent::PlaySound { object: me, sound: string(a, 0).to_ascii_lowercase() });
                Value::Void
            }
            "SoundObjectPlay" | "SoundObjectStop" | "SoundObjectFadeAndStop" => {
                let t = obj(a, 0);
                let playing = name == "SoundObjectPlay";
                if let Some(Detail::Sound(sd)) = self.get_mut(t).map(|x| &mut x.detail) {
                    sd.active = playing;
                }
                let fade = if name == "SoundObjectFadeAndStop" { float(a, 1).max(0.0) } else { 0.0 };
                self.emit(WorldEvent::SoundObject { object: t, playing, fade });
                Value::Void
            }
            "SoundObjectSetVolume" => {
                self.emit(WorldEvent::SoundObjectVolume { object: obj(a, 0), volume: int(a, 1) });
                Value::Void
            }
            "SoundObjectSetFixedVariance" => {
                self.emit(WorldEvent::SoundObjectVariance { object: obj(a, 0), variance: float(a, 1) });
                Value::Void
            }
            "SoundObjectSetPosition" => Value::Void,
            "AddJournalQuestEntry" => {
                let (plot, st, allow) = (string(a, 0).to_ascii_lowercase(), int(a, 1), int(a, 2) != 0);
                let cur = self.journal.get(&plot).copied().unwrap_or(0);
                if allow || st > cur {
                    self.journal.insert(plot.clone(), st);
                    self.emit(WorldEvent::Journal { plot, state: st });
                }
                Value::Void
            }
            "RemoveJournalQuestEntry" => {
                self.journal.remove(&string(a, 0).to_ascii_lowercase());
                Value::Void
            }
            "GetJournalEntry" => i(self.journal.get(&string(a, 0).to_ascii_lowercase()).copied().unwrap_or(0)),
            "StartNewModule" => {
                let module = string(a, 0).to_owned();
                let wp = string(a, 1);
                let movies: Vec<String> = (2..8).map(|n| string(a, n).to_owned()).filter(|m| !m.is_empty()).collect();
                self.request_transition(module, Some(wp.to_owned()).filter(|w| !w.is_empty()), movies);
                Value::Void
            }
            "GetAreaUnescapable" => b(self.area_unescapable()),
            "GetPlayerRestrictMode" => b(false),
            "PlayPazaak" => {
                self.last_pazaak = 1;
                let end = string(a, 1).to_owned();
                if !end.is_empty() {
                    self.run_script(&end, me, ScriptCtx::default());
                }
                Value::Void
            }
            "GetLastPazaakResult" => i(self.last_pazaak),
            "GetTimeHour" | "GetTimeMinute" | "GetTimeSecond" | "GetTimeMillisecond" => {
                let (start, mph) = self.module.as_ref().map_or((13.0, 2.0), |m| (f64::from(m.info.start_hour), f64::from(m.info.min_per_hour.max(1))));
                // Aurora clock: one game hour lasts `mph` real minutes, and
                // minutes/seconds/milliseconds are real time within that hour
                // (GetTimeMinute runs 0..mph-1). Swoop lap timing relies on it.
                let real = self.time;
                i(match name {
                    "GetTimeHour" => (start as i32 + (real / (mph * 60.0)) as i32) % 24,
                    "GetTimeMinute" => ((real / 60.0) as i64 % mph as i64) as i32,
                    "GetTimeSecond" => (real as i64 % 60) as i32,
                    _ => (real.fract() * 1000.0) as i32,
                })
            }
            "GetReputation" => {
                let (src, t) = (obj(a, 0), obj(a, 1));
                i(if self.is_hostile(src, t) {
                    0
                } else if self.faction(src) == 5 || self.faction(t) == 5 {
                    50
                } else {
                    100
                })
            }
            // Combat/AI state: nothing is simulated yet, so these report
            // "no target / no talent / default style" explicitly.
            "GetAttemptedAttackTarget" | "GetAttemptedSpellTarget" | "GetAttackTarget" | "GetAttemptedMovementTarget"
            | "GetLastDamager" | "GetLastHostileTarget" | "GetLastHostileActor" | "GetHealTarget" | "GetLastAttacker"
            | "GetLastKiller" | "GetSpellTarget" | "GetLastSpellCaster" => o(OBJECT_INVALID),
            "GetNPCAIStyle" | "GetPartyAIStyle" | "GetIdFromTalent" | "GetTypeFromTalent" | "GetCategoryFromTalent"
            | "GetCreatureHasTalent" | "IsMeditating" | "GetLastAttackResult" | "GetLastForcePowerUsed"
            | "GetWasForcePowerSuccessful" | "GetLastCombatFeatUsed" | "GetSpellId" => i(0),
            "GetLastAttackAction" => i(self.constant("ACTION_INVALID").unwrap_or(65535)),
            "GetCreatureTalentRandom" | "GetCreatureTalentBest" => Value::Engine(k2_script::EngineValue { kind: ENGINE_TALENT, handle: 0 }),
            "DisableHealthRegen" => {
                // Global switch for the party's innate vitality regeneration
                // (regen.rs): TRUE turns it off until FALSE is passed.
                self.powers.innate_hp_regen_off = int(a, 0) != 0;
                Value::Void
            }
            "StopRumblePattern" | "ResetCreatureAILevel" | "MusicBattlePlay"
            | "MusicBattleStop" | "GrantFeat" | "AddMultiClass" | "AdjustCreatureSkills" | "ModifyWillSavingThrowBase"
            | "ModifyReflexSavingThrowBase" | "ModifyFortitudeSavingThrowBase"
            | "SetReturnStrref" | "PauseGame" | "RebuildPartyTable" | "UnlockAllSongs" | "CancelPostDialogCharacterSwitch"
            | "SetCurrentForm" | "DuplicateHeadAppearance" | "SpawnMine" | "DetonateMine" | "YavinHackDoorClose" => Value::Void,
            "PlayRumblePattern" | "GetIsPoisoned" | "GetIsTrapped" | "GetSkillRankBase" | "GetSpellBaseForcePointCost"
            | "GetLastForfeitViolation" | "GetTotalDamageDealt" | "IsFormActive" | "GetSpellForcePointCost"
            | "GetIsLinkImmune" | "GetIsImmune" | "GetLastSpellHarmful" | "GetItemHasItemProperty" | "GetOwnerDemolitionsSkill"
            | "GetSubRace" | "GetCreatureMovmentType" | "ResistForce" | "SetTrapDetectedBy" => i(0),
            "GetEffectSpellId" | "GetTrapBaseType" => i(-1),
            "GetGameDifficulty" => i(1),
            "GetSpellSaveDC" => i(10),
            "GetCreatureSize" => i(3),
            "GetIsDoorActionPossible" => b(self.get(obj(a, 0)).is_some_and(|x| x.kind == ObjectKind::Door)),
            "DoDoorAction" => {
                // DOOR_ACTION_OPEN = 0; the others (unlock, bash, ...) also end open.
                let d = obj(a, 0);
                self.set_locked(d, false);
                self.set_door_open(d, true, me);
                Value::Void
            }
            "GetBlockingDoor" | "GetBlockingCreature" | "GetFoundEnemyCreature" | "GetInventoryDisturbItem"
            | "GetLastWeaponUsed" | "GetSpellTargetObject" | "SpawnAvailablePUP" => o(OBJECT_INVALID),
            "GetSpellTargetLocation" => {
                let l = self.location_of(me);
                self.engine.add(EngineData::Location(l))
            }
            "GetIsPlayerMadeCharacter" => b(obj(a, 0) == self.player),
            "AddAvailablePUPByObject" | "AddAvailablePUPByTemplate" => b(true),
            "GetModuleFileName" => s(self.module.as_ref().map(|m| m.name.to_ascii_uppercase()).unwrap_or_default()),
            "GetXP" => i(self.creature_state(obj(a, 0)).map_or(0, |c| c.xp)),
            "SetXP" => {
                let (t, v) = (obj(a, 0), int(a, 1));
                if let Some(c) = self.get_mut(t).and_then(|x| x.creature_mut()) {
                    c.xp = v.max(0);
                }
                Value::Void
            }
            "IsRunning" => b(matches!(self.motion.get(&obj(a, 0)), Some(crate::Motion::Run))),
            "ReflexSave" | "WillSave" | "FortitudeSave" => {
                let roll = self.builtins.random(20) + 1;
                b(roll + 2 >= int(a, 1))
            }
            "GetReflexAdjustedDamage" => i(int(a, 0)),
            "SetEffectIcon" => a.first().cloned().unwrap_or(Value::default_for(ret)),
            "RemoveEffectByID" => {
                let id = int(a, 1);
                self.remove_effects(obj(a, 0), |e| e.ty == id);
                Value::Void
            }
            "RemoveEffectByExactMatch" => {
                // Same effect type and the same Integer(0) and Integer(1).
                if let Some(reference) = a.get(1).and_then(|v| self.engine.effect(v)).cloned() {
                    self.remove_effects(obj(a, 0), |e| {
                        e.ty == reference.ty && (0..2).all(|n| int(&e.args, n) == int(&reference.args, n))
                    });
                }
                Value::Void
            }
            "ChangeObjectAppearance" => {
                let (t, ap) = (obj(a, 0), int(a, 1));
                if let Some(c) = self.get_mut(t).and_then(|x| x.creature_mut()) {
                    c.appearance = ap.max(0) as u32;
                }
                self.emit(WorldEvent::ObjectSpawned(t));
                Value::Void
            }
            "CutsceneAttack" => {
                let (t, anim, dmg) = (obj(a, 0), int(a, 1), int(a, 3));
                self.emit(WorldEvent::PlayAnimation { object: me, animation: anim, speed: 1.0, duration: 0.0, looping: false });
                if dmg > 0 {
                    self.damage(t, dmg, me);
                }
                Value::Void
            }
            "PlayOverlayAnimation" => {
                self.emit(WorldEvent::PlayAnimation { object: obj(a, 0), animation: int(a, 1), speed: 1.0, duration: 0.0, looping: false });
                Value::Void
            }
            "FaceObjectAwayFromObject" => {
                let (t, from) = (obj(a, 0), obj(a, 1));
                if let (Some(p), Some(q)) = (self.pos(t), self.pos(from))
                    && let Some(bb) = bearing_towards(q, p)
                    && let Some(x) = self.get_mut(t)
                {
                    x.bearing = bb;
                }
                Value::Void
            }
            "GetFactionEqual" => {
                let second = self.me_or(a, 1, me);
                b(self.faction(obj(a, 0)) == self.faction(second))
            }
            "ForceHeartbeat" => {
                let t = obj(a, 0);
                self.queue_event(t, "ScriptHeartbeat", ScriptCtx::default());
                Value::Void
            }
            "SetPlanetAvailable" | "SetPlanetSelectable" => {
                let e = self.planets.entry(int(a, 0)).or_insert((false, false));
                if name == "SetPlanetAvailable" {
                    e.0 = int(a, 1) != 0;
                } else {
                    e.1 = int(a, 1) != 0;
                }
                Value::Void
            }
            "GetPlanetAvailable" => b(self.planets.get(&int(a, 0)).is_some_and(|p| p.0)),
            "GetPlanetSelectable" => b(self.planets.get(&int(a, 0)).is_some_and(|p| p.1)),
            "GetSelectedPlanet" => i(self.selected_planet),
            "ShowGalaxyMap" => {
                self.selected_planet = int(a, 0);
                self.emit(WorldEvent::ShowGui("galaxymap".into()));
                Value::Void
            }
            "DisplayMessageBox" => {
                let text = self.talk.as_ref().and_then(|t| t.get(int(a, 0) as u32)).unwrap_or("").to_owned();
                self.emit(WorldEvent::DebugText(text));
                Value::Void
            }
            "DisplayDatapad" | "ShowChemicalUpgradeScreen" | "ShowSwoopUpgradeScreen" => {
                self.emit(WorldEvent::ShowGui(name.to_ascii_lowercase()));
                Value::Void
            }
            "EndGame" => {
                self.emit(WorldEvent::ShowGui("endgame".into()));
                Value::Void
            }
            "ShowUpgradeScreen" => {
                self.emit(WorldEvent::ShowGui("upgrade".into()));
                Value::Void
            }
            _ => {
                // Generic constructors so effects, events and talents are real
                // values even when not simulated.
                if name.starts_with("Effect") && ret == VarType::Engine(ENGINE_EFFECT) {
                    self.new_effect(name, a, me)
                } else if name.starts_with("Event") && ret == VarType::Engine(ENGINE_EVENT) {
                    self.engine.add(EngineData::Event(Event::Other(name.to_owned())))
                } else if name.starts_with("Talent") && ret == VarType::Engine(ENGINE_TALENT) {
                    self.engine.add(EngineData::Talent(Talent { ty: 0, id: int(a, 0) }))
                } else {
                    return None;
                }
            }
        };
        Some(v)
    }
}
