//! Target selection.
//!
//! The target is the interactable under the mouse cursor (ray against a
//! sphere around each object), else the nearest one in front of the player
//! within a few meters. Its name and health are drawn by the GUI HUD
//! (`gui::hud`).

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::area::Interactable;
use crate::coords;
use crate::play::Player;

/// Max distance for the nearest-in-front fallback (m).
const NEAR_RANGE: f32 = 4.0;
/// Max distance for cursor picking (m).
const PICK_RANGE: f32 = 40.0;

/// The current target, if any.
#[derive(Resource, Default)]
pub struct Target(pub Option<Entity>);

pub struct HoverPlugin;

impl Plugin for HoverPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Target>().add_systems(PostUpdate, pick_target.after(TransformSystems::Propagate));
    }
}

use bevy::transform::TransformSystems;

/// Center of an object's pick sphere in Bevy space.
fn pick_center(gt: &GlobalTransform, it: &Interactable) -> Vec3 {
    gt.translation() + Vec3::Y * (it.height * 0.5)
}

fn pick_target(
    windows: Query<&Window, With<PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    player: Query<&Player>,
    objects: Query<(Entity, &GlobalTransform, &Interactable)>,
    ui: Query<&Interaction>,
    mut target: ResMut<Target>,
) {
    // Keep the target while the cursor is on a HUD button (the quick action
    // bar acts on it).
    if ui.iter().any(|i| *i != Interaction::None) {
        return;
    }
    let Ok((cam, cam_gt)) = cams.single() else { return };
    let cursor_ray = windows.single().ok().and_then(|w| w.cursor_position()).and_then(|c| cam.viewport_to_world(cam_gt, c).ok());

    let mut best: Option<(f32, Entity)> = None;
    if let Some(ray) = cursor_ray {
        for (e, gt, it) in &objects {
            let c = pick_center(gt, it);
            let r = it.radius.max(0.5).max(it.height * 0.5);
            let oc = c - ray.origin;
            let t = oc.dot(*ray.direction);
            if !(0.0..PICK_RANGE).contains(&t) {
                continue;
            }
            let miss = (oc - *ray.direction * t).length();
            if miss <= r && best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, e));
            }
        }
    }
    if best.is_none()
        && let Ok(p) = player.single()
    {
        let pos = coords::to_bevy(p.pos);
        let fwd = coords::to_bevy(Vec3::new(p.facing.cos(), p.facing.sin(), 0.0));
        for (e, gt, _) in &objects {
            let d = gt.translation() - pos;
            let flat = Vec3::new(d.x, 0.0, d.z);
            let dist = flat.length();
            if dist > NEAR_RANGE || dist < 1e-3 {
                continue;
            }
            let facing = flat.dot(fwd) / dist;
            if facing < 0.3 {
                continue;
            }
            let score = dist * (2.0 - facing);
            if best.is_none_or(|(bs, _)| score < bs) {
                best = Some((score, e));
            }
        }
    }
    target.0 = best.map(|(_, e)| e);
}
