//! Host-side data behind script engine structures (effect, event, location,
//! talent). The VM only carries a handle.

use glam::Vec3;
use k2_script::{EngineValue, ObjectId, Value, OBJECT_INVALID};
use serde::{Deserialize, Serialize};

/// Engine-structure kinds in K2's nwscript.nss declaration order.
pub const ENGINE_EFFECT: u8 = 0;
pub const ENGINE_EVENT: u8 = 1;
pub const ENGINE_LOCATION: u8 = 2;
pub const ENGINE_TALENT: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Location {
    pub position: Vec3,
    /// NWScript facing in degrees (0 = +X, counter-clockwise).
    pub facing: f32,
    pub area: ObjectId,
}

impl Default for Location {
    fn default() -> Self {
        Self { position: Vec3::ZERO, facing: 0.0, area: OBJECT_INVALID }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    /// EFFECT_TYPE_* value (0 when unknown).
    pub ty: i32,
    /// Constructor routine name, e.g. EffectDamage.
    pub name: String,
    pub args: Vec<Value>,
    pub creator: ObjectId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    UserDefined(i32),
    /// Any other EventX constructor, by routine name.
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Talent {
    pub ty: i32,
    pub id: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EngineData {
    Effect(Effect),
    Event(Event),
    Location(Location),
    Talent(Talent),
}

impl EngineData {
    fn kind(&self) -> u8 {
        match self {
            EngineData::Effect(_) => ENGINE_EFFECT,
            EngineData::Event(_) => ENGINE_EVENT,
            EngineData::Location(_) => ENGINE_LOCATION,
            EngineData::Talent(_) => ENGINE_TALENT,
        }
    }
}

/// Handle table. Handle 0 is the empty value RSADD reserves; handles are
/// index + 1. TODO: reclaim handles no longer referenced by any stack,
/// local or action (they are small, so this only matters for long sessions).
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineStore {
    items: Vec<EngineData>,
}

impl EngineStore {
    pub fn add(&mut self, d: EngineData) -> Value {
        let kind = d.kind();
        self.items.push(d);
        Value::Engine(EngineValue { kind, handle: self.items.len() as u32 })
    }

    pub fn get(&self, v: &Value) -> Option<&EngineData> {
        match v {
            Value::Engine(e) if e.handle > 0 => self.items.get(e.handle as usize - 1),
            _ => None,
        }
    }

    pub fn by_handle(&self, handle: u32) -> Option<&EngineData> {
        if handle == 0 {
            return None;
        }
        self.items.get(handle as usize - 1)
    }

    pub fn location(&self, v: &Value) -> Option<Location> {
        match self.get(v)? {
            EngineData::Location(l) => Some(*l),
            _ => None,
        }
    }

    pub fn effect(&self, v: &Value) -> Option<&Effect> {
        match self.get(v)? {
            EngineData::Effect(e) => Some(e),
            _ => None,
        }
    }

    pub fn event(&self, v: &Value) -> Option<&Event> {
        match self.get(v)? {
            EngineData::Event(e) => Some(e),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
