//! NWScript (NCS) virtual machine for KotOR2.
//!
//! - [`Ncs`]: parsed bytecode.
//! - [`Routines`]: engine routine table parsed at runtime from the game's
//!   own nwscript.nss (877 routines in K2).
//! - [`Vm`]: executes scripts against a [`ScriptHost`] that implements the
//!   engine routines.
//!
//! See docs/notes/ncs-vm.md.

pub mod builtins;
pub mod ncs;
pub mod nss;
pub mod saved;
pub mod value;
pub mod vm;

use std::sync::Arc;

use k2_res::{ResKey, Resources};
use rakata_core::ResourceType;

pub use builtins::Builtins;
pub use ncs::{Instr, Ncs, NcsError, Op};
pub use nss::{DefaultValue, Param, Routine, Routines, VarType};
pub use value::{EngineValue, ObjectId, ScriptState, Value, OBJECT_INVALID};
pub use vm::{Call, Outcome, ScriptHost, Vm, VmError, VmErrorKind};

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error(transparent)]
    Res(#[from] k2_res::ResError),
    #[error("{0} not found")]
    Missing(String),
    #[error("{0}: {1}")]
    Ncs(String, NcsError),
    #[error(transparent)]
    Nss(#[from] nss::NssError),
}

/// Parses the game's nwscript.nss into the routine table.
pub fn load_routines(res: &Resources) -> Result<Routines, LoadError> {
    let key = ResKey::new("nwscript", ResourceType::Nss);
    let bytes = res.get(&key)?.ok_or_else(|| LoadError::Missing(key.to_string()))?;
    Ok(Routines::parse(&bytes)?)
}

/// Loads and parses a compiled script by resref.
pub fn load_ncs(res: &Resources, name: &str) -> Result<Arc<Ncs>, LoadError> {
    let key = ResKey::new(name, ResourceType::Ncs);
    let bytes = res.get(&key)?.ok_or_else(|| LoadError::Missing(key.to_string()))?;
    Ncs::parse(&key.name, &bytes).map(Arc::new).map_err(|e| LoadError::Ncs(key.to_string(), e))
}

/// A host with no game world: world-independent routines run through
/// [`Builtins`], every other routine returns its type's default. Records the
/// calls it saw and the deferred actions it was handed.
#[derive(Default)]
pub struct StubHost {
    pub builtins: Builtins,
    pub log: bool,
    pub calls: Vec<String>,
    pub deferred: Vec<Arc<ScriptState>>,
}

impl ScriptHost for StubHost {
    fn call(&mut self, call: Call<'_>) -> Value {
        for a in call.args {
            if let Value::Action(s) = a {
                self.deferred.push(s.clone());
            }
        }
        if self.log {
            let args: Vec<String> = call
                .args
                .iter()
                .map(|a| match a {
                    Value::Action(s) => format!("<action @{:#x}>", s.offset),
                    Value::String(s) => format!("{s:?}"),
                    Value::Int(v) => v.to_string(),
                    Value::Float(v) => format!("{v:?}"),
                    Value::Object(o) => format!("obj:{o:#x}"),
                    Value::Vector(v) => format!("[{}, {}, {}]", v[0], v[1], v[2]),
                    Value::Engine(e) => format!("eng{}:{}", e.kind, e.handle),
                    Value::Void => "void".into(),
                })
                .collect();
            let line = format!("{}({})", call.routine.name, args.join(", "));
            println!("  {line}");
            self.calls.push(line);
        } else {
            self.calls.push(call.routine.name.clone());
        }
        self.builtins.call(&call).unwrap_or_else(|| Value::default_for(call.routine.ret))
    }
}
