//! Door timing: a door's walkmesh passes only once the opening animation
//! has finished, and blocks again as soon as closing starts. See
//! docs/notes/world.md ("Door transitions").

use std::collections::HashMap;

use glam::{Vec2, Vec3};
use k2_script::ObjectId;
use rakata_core::ResourceType;

use crate::World;

/// Opening time used when the model's clip has no length.
const FALLBACK_OPEN_SECS: f32 = 1.0;

/// Route cost (metres) of passing a closed door a walker opens: a little.
const DOOR_OPEN_COST: f32 = 2.0;
/// Route cost (metres) of passing a locked door: a lot, so a route goes round it when it can.
const DOOR_LOCKED_COST: f32 = 40.0;

/// Clip lengths of door models (opening1, opening2; None = no such clip) and
/// the load flag that makes opens during module entry instant.
#[derive(Debug, Default)]
pub(crate) struct DoorAnims {
    clips: HashMap<String, [Option<f32>; 2]>,
    /// Module entry scripts run with this set: doors they open are already
    /// open when the player first sees the area.
    pub(crate) instant: bool,
}

impl World {
    /// Seconds the opening clip of `side` (1 or 2) of a door model plays;
    /// 0 when the model has no opening clip. Side 2 falls back to side 1.
    fn door_open_secs(&mut self, model: &str, side: u8) -> f32 {
        let clips = match self.door_anims.clips.get(model) {
            Some(c) => *c,
            None => {
                let c = load_clips(self, model);
                self.door_anims.clips.insert(model.to_owned(), c);
                c
            }
        };
        let want = if side == 2 { clips[1].or(clips[0]) } else { clips[0] };
        want.map_or(0.0, |l| if l > 0.0 { l } else { FALLBACK_OPEN_SECS })
    }

    /// Which opening clip / DWK set (1 or 2) a door opens with when `by`
    /// opens it: 2 when the opener stands on the door's local +Y side (the
    /// leaf swings away from the opener, toward -Y, as the DWK sets that
    /// differ show), else 1. 1 when there is no usable opener.
    fn door_open_side(&self, door: ObjectId, by: ObjectId) -> u8 {
        let (Some(d), Some(o)) = (self.get(door), self.get(by)) else { return 1 };
        if by == door {
            return 1;
        }
        let rel = (o.position - d.position).truncate();
        let local_y = rel.dot(Vec2::new(-d.bearing.sin(), d.bearing.cos()));
        if local_y > 0.0 { 2 } else { 1 }
    }

    /// Walkmesh state of an open door of `side`.
    pub(crate) fn door_open_walk(&self, door: ObjectId, side: u8) -> k2_walk::DoorState {
        match self.door_walk.get(&door) {
            Some(h) if side == 2 && h.open2.is_some() => k2_walk::DoorState::Open2,
            _ => k2_walk::DoorState::Open1,
        }
    }

    /// Opens or closes a door: the open flag, events and scripts change at
    /// once; the walkmesh follows the animation (see `tick_doors`).
    pub fn set_door_open(&mut self, door: ObjectId, open: bool, by: ObjectId) {
        let Some(d) = self.get(door).and_then(|o| o.door()) else { return };
        if d.open == open {
            return;
        }
        let (model, prev_side) = (d.model.clone(), d.side);
        let side = if open { self.door_open_side(door, by) } else { prev_side };
        let secs = match model {
            Some(m) if open && !self.door_anims.instant && self.door_walk.contains_key(&door) => self.door_open_secs(&m, side),
            _ => 0.0,
        };
        if let Some(d) = self.get_mut(door).and_then(|o| o.door_mut()) {
            d.open = open;
            d.side = side;
            d.opening = secs;
        }
        let state = if !open || secs > 0.0 { k2_walk::DoorState::Closed } else { self.door_open_walk(door, side) };
        if let Some(h) = self.door_walk.get(&door).copied() {
            self.walk.set_door(h, state);
        }
        self.finish_door_change(door, open, by);
    }

    /// Sets how routes may pass each door's closed walkmesh: a static door or a transit door
    /// (its use changes the module) is a blocker; any other closed door is passable at a cost, a
    /// little when unlocked (a walker opens it on the way) and a lot when locked (the leader's walk
    /// stops at it; an NPC's route goes round it, see `find_path_avoiding_locked`).
    pub(crate) fn sync_door_passable(&mut self) {
        let doors: Vec<(ObjectId, k2_walk::MeshHandle)> = self.door_walk.iter().filter_map(|(id, h)| h.closed.map(|c| (*id, c))).collect();
        for (id, closed) in doors {
            let route = self.get(id).and_then(|o| o.door()).and_then(|d| {
                // A transit door is never walked through: its use changes the module.
                let transit = d.linked_to_module.as_deref().is_some_and(|m| !m.is_empty());
                (!d.is_static && !transit).then_some(k2_walk::DoorRoute { cost: if d.locked { DOOR_LOCKED_COST } else { DOOR_OPEN_COST }, locked: d.locked })
            });
            self.walk.set_door_route(closed, route);
        }
    }

    /// The closed door a walk from `a` to `b` runs into, if a walker can open it.
    pub fn door_across(&self, a: Vec3, b: Vec3) -> Option<ObjectId> {
        let mesh = self.walk.passable_crossing(a, b)?;
        self.door_walk.iter().find(|(_, h)| h.closed == Some(mesh)).map(|(id, _)| *id)
    }

    /// Advances door opening timers; a door that finishes opening becomes
    /// passable.
    pub(crate) fn tick_doors(&mut self, dt: f32) {
        let mut done = Vec::new();
        for o in self.objects.values_mut() {
            let id = o.id;
            if let Some(d) = o.door_mut()
                && d.opening > 0.0
            {
                d.opening -= dt;
                if d.opening <= 0.0 {
                    d.opening = 0.0;
                    done.push((id, d.side));
                }
            }
        }
        for (id, side) in done {
            let state = self.door_open_walk(id, side);
            if let Some(h) = self.door_walk.get(&id).copied() {
                self.walk.set_door(h, state);
            }
        }
    }
}

fn load_clips(world: &World, model: &str) -> [Option<f32>; 2] {
    let Ok(mdl) = k2_data::read_bytes(&world.res, model, ResourceType::Mdl) else { return [None; 2] };
    let mdx = k2_data::read_bytes(&world.res, model, ResourceType::Mdx).ok();
    let Ok(mdl) = k2_mdl::read_mdl_from_bytes(&mdl, mdx.as_deref()) else { return [None; 2] };
    let find = |name: &str| mdl.animations.iter().find(|a| a.name.eq_ignore_ascii_case(name)).map(|a| a.length);
    [find("opening1"), find("opening2")]
}
