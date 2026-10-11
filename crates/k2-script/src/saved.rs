//! Serde support for script values in save games.
//!
//! A [`ScriptState`](crate::ScriptState) holds its compiled script as an
//! `Arc<Ncs>`. Saves store only the script name; loading resolves names
//! through a per-thread table the host fills, see [`with_scripts`]. Names the
//! table lacks deserialize to an empty placeholder and are reported, so a host
//! can load them and deserialize again.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ncs::Ncs;

thread_local! {
    static RESOLVE: RefCell<Option<HashMap<String, Arc<Ncs>>>> = const { RefCell::new(None) };
    static MISSING: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Runs `f` (a deserialization) with `scripts` as the name table. Returns its
/// result and the script names that were not in the table.
pub fn with_scripts<R>(scripts: HashMap<String, Arc<Ncs>>, f: impl FnOnce() -> R) -> (R, Vec<String>) {
    RESOLVE.with(|r| *r.borrow_mut() = Some(scripts));
    MISSING.with(|m| m.borrow_mut().clear());
    let out = f();
    RESOLVE.with(|r| *r.borrow_mut() = None);
    let mut missing = MISSING.with(|m| std::mem::take(&mut *m.borrow_mut()));
    missing.sort();
    missing.dedup();
    (out, missing)
}

pub(crate) fn serialize<S: Serializer>(ncs: &Arc<Ncs>, s: S) -> Result<S::Ok, S::Error> {
    ncs.name.serialize(s)
}

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<Ncs>, D::Error> {
    let name = String::deserialize(d)?;
    let found = RESOLVE.with(|r| r.borrow().as_ref().and_then(|m| m.get(&name).cloned()));
    Ok(found.unwrap_or_else(|| {
        MISSING.with(|m| m.borrow_mut().push(name.clone()));
        Arc::new(Ncs::placeholder(name))
    }))
}
