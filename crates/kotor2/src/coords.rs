//! The one place game space (Z-up) meets Bevy space (Y-up).
//!
//! Everything loaded from game data is spawned under the `WorldRoot` entity,
//! whose transform is the basis change, so node and instance transforms stay
//! in game coordinates. Code outside the world root (camera, UI) converts
//! with the helpers below. bevy = (x, z, -y).

use bevy::prelude::*;

/// Parent of all game-space entities.
#[derive(Component)]
pub struct WorldRoot;

pub fn world_root_transform() -> Transform {
    Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
}

pub fn to_bevy(p: Vec3) -> Vec3 {
    Vec3::new(p.x, p.z, -p.y)
}

#[allow(dead_code)] // used once gameplay lands
pub fn to_game(p: Vec3) -> Vec3 {
    Vec3::new(p.x, -p.z, p.y)
}

/// Game yaw (radians, 0 = +X, counter-clockwise about +Z) to a Bevy yaw for a
/// camera whose forward is -Z.
pub fn game_yaw_to_bevy(yaw: f32) -> f32 {
    yaw - std::f32::consts::FRAC_PI_2
}
