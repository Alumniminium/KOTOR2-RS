//! Typed KotOR2 game data on top of k2-res: 2DA tables, TLK strings,
//! modules (IFO/ARE/GIT/LYT/VIS), blueprints and dialogs.
//!
//! Everything here is in game coordinates (Z-up, meters). No Bevy.

pub mod blueprint;
pub mod dlg;
pub mod gff;
pub mod module;
pub mod tables;
pub mod talk;
pub mod vis;

use k2_res::{ResKey, Resources};
use rakata_core::ResourceType;
use rakata_formats::gff::Gff;

pub use blueprint::{Creature, Door, Encounter, EncounterCreature, Item, Placeable, SoundBlueprint, Trigger, Waypoint};
pub use dlg::Dialog;
pub use gff::{GffExt, LocString};
pub use module::Module;
pub use tables::Tables;
pub use talk::Talk;

#[derive(Debug, thiserror::Error)]
pub enum DataError {
    #[error(transparent)]
    Res(#[from] k2_res::ResError),
    #[error("{0} not found")]
    Missing(String),
    #[error("{what}: {msg}")]
    Parse { what: String, msg: String },
}

pub type Result<T> = std::result::Result<T, DataError>;

pub(crate) fn parse_err(what: impl std::fmt::Display, msg: impl std::fmt::Display) -> DataError {
    DataError::Parse { what: what.to_string(), msg: msg.to_string() }
}

/// Raw bytes of a resource, or `Missing`.
pub fn read_bytes(res: &Resources, name: &str, ty: ResourceType) -> Result<Vec<u8>> {
    let key = ResKey::new(name, ty);
    res.get(&key)?.ok_or_else(|| DataError::Missing(key.to_string()))
}

/// Parse a GFF resource.
pub fn read_gff(res: &Resources, name: &str, ty: ResourceType) -> Result<Gff> {
    let key = ResKey::new(name, ty);
    let bytes = res.get(&key)?.ok_or_else(|| DataError::Missing(key.to_string()))?;
    rakata_formats::read_gff_from_bytes(&bytes).map_err(|e| parse_err(&key, e))
}

/// Parse a GFF resource if present.
pub fn try_read_gff(res: &Resources, name: &str, ty: ResourceType) -> Result<Option<Gff>> {
    match read_gff(res, name, ty) {
        Ok(g) => Ok(Some(g)),
        Err(DataError::Missing(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Debug dump of a GFF resource (k2probe `gff`).
pub fn dump_gff(res: &Resources, name: &str, ty: ResourceType, max_list: usize) -> Result<String> {
    let g = read_gff(res, name, ty)?;
    Ok(format!("type {:?}\n{}", String::from_utf8_lossy(&g.file_type), gff::dump(&g.root, 0, max_list)))
}
