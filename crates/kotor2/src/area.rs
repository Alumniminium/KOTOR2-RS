//! Shared area resources: the loaded module's typed data, the 2DA/TLK
//! cache, and the `Interactable` marker. Objects themselves are spawned
//! from k2-world (see `game.rs`).

use std::sync::Arc;

use bevy::prelude::*;
use k2_data::{Module, Tables};

/// The loaded module's typed data.
#[derive(Resource, Clone)]
pub struct AreaModule(pub Arc<Module>);

/// Shared 2DA cache (names and strings come from the world's talk table).
#[derive(Resource)]
pub struct GameData {
    pub tables: Tables,
}

/// Something the player can point at: display name, interaction radius and
/// the height of its label above the feet (game space).
#[derive(Component, Clone, Debug)]
pub struct Interactable {
    pub name: String,
    pub radius: f32,
    pub height: f32,
    #[allow(dead_code)] // read by the world layer
    pub useable: bool,
}

