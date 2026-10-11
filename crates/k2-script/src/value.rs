//! Script values.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ncs::Ncs;
use crate::nss::VarType;

pub type ObjectId = u32;

/// The engine's invalid object id. Scripts spell it 1, 0xFFFFFFFF or
/// 0x7F000000 in CONSTO; all of them map here.
pub const OBJECT_INVALID: ObjectId = 0x7F00_0000;

/// An engine structure value (effect, event, location, talent). The host
/// owns the data; the VM only carries the handle. Handle 0 is the empty value
/// RSADD reserves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EngineValue {
    pub kind: u8,
    pub handle: u32,
}

/// A value on the stack or crossing the host boundary.
///
/// On the stack every value is one 4-byte slot; vectors are three Float
/// slots (x deepest, z on top). `Vector` and `Action` only appear in
/// host-call arguments and return values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Void,
    Int(i32),
    Float(f32),
    String(Arc<str>),
    Object(ObjectId),
    Engine(EngineValue),
    Vector([f32; 3]),
    Action(Arc<ScriptState>),
}

impl Value {
    pub fn default_for(ty: VarType) -> Value {
        match ty {
            VarType::Void | VarType::Action => Value::Void,
            VarType::Int => Value::Int(0),
            VarType::Float => Value::Float(0.0),
            VarType::String => Value::String(Arc::from("")),
            VarType::Object => Value::Object(OBJECT_INVALID),
            VarType::Vector => Value::Vector([0.0; 3]),
            VarType::Engine(kind) => Value::Engine(EngineValue { kind, handle: 0 }),
        }
    }

    /// Whether this value can sit in a slot of the given type.
    pub fn fits(&self, ty: VarType) -> bool {
        matches!(
            (self, ty),
            (Value::Void, VarType::Void)
                | (Value::Int(_), VarType::Int)
                | (Value::Float(_), VarType::Float)
                | (Value::String(_), VarType::String)
                | (Value::Object(_), VarType::Object)
                | (Value::Vector(_), VarType::Vector)
                | (Value::Action(_), VarType::Action)
        ) || matches!((self, ty), (Value::Engine(e), VarType::Engine(k)) if e.kind == k)
    }

    pub fn as_int(&self) -> Option<i32> {
        match self {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f32> {
        match self {
            Value::Float(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<ObjectId> {
        match self {
            Value::Object(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_vector(&self) -> Option<[f32; 3]> {
        match self {
            Value::Vector(v) => Some(*v),
            _ => None,
        }
    }
}

/// A deferred code block captured by STORE_STATE: the argument of
/// DelayCommand, AssignCommand, ActionDoCommand and friends.
///
/// Resume with [`crate::Vm::resume`]: the globals are pushed, BP is set above
/// them, the locals are pushed, and execution starts at `offset` until the
/// block's RETN.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptState {
    /// Saved by name; see [`crate::saved`].
    #[serde(with = "crate::saved")]
    pub script: Arc<Ncs>,
    pub offset: u32,
    /// Saved globals, deepest first.
    pub globals: Vec<Value>,
    /// Saved locals, deepest first.
    pub locals: Vec<Value>,
    /// Object that was OBJECT_SELF when the state was stored.
    pub caller: ObjectId,
}
