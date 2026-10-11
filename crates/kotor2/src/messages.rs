//! Messages between the game layer and presentation subsystems.

use bevy::prelude::*;

/// An area finished loading (rooms spawned, `AreaModule` replaced). Audio
/// rebuilds music, ambience and sound objects on this.
#[derive(Message, Clone, Debug)]
pub struct AreaLoaded {
    pub module: String,
}
