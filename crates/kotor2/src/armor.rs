//! Head gear: the item model of the equipped head slot item (INVENTORY_SLOT_HEAD
//! 0, baseitems Mask, models i_mask_NNN) hangs off the head model's
//! `MaskHook` node, so it follows the head animation. Attached once the
//! body's `AnimBindings` exist, rebuilt on `EquipmentChanged` for the head
//! slot. Body armour is handled by the world (new body model and texture ->
//! `AppearanceChanged` -> respawn, see weapons.rs).

use bevy::prelude::*;
use k2_script::ObjectId;
use k2_world::WorldEvent;

use crate::anim::AnimBindings;
use crate::game::{ApplyEvents, FrameEvents, GameSet, ObjBody, ObjEntities, ObjId, WorldRes};
use crate::material::KMaterial;
use crate::model::{self, AssetCache, GameRes, Loader};

const SLOT_HEAD: i32 = 0;

/// Head gear model roots currently attached to this object's head.
#[derive(Component, Default)]
pub struct HeadGear(pub Vec<Entity>);

pub struct ArmorPlugin;

impl Plugin for ArmorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (log_body_changes, attach_headgear).in_set(GameSet::Sync).after(ApplyEvents));
    }
}

/// Logs the new body model and texture when worn armour changes them.
fn log_body_changes(events: Res<FrameEvents>, world: Res<WorldRes>) {
    let w = &world.0;
    for e in &events.0 {
        if let WorldEvent::AppearanceChanged { object } = e
            && let Some(c) = w.get(*object).and_then(|o| o.creature())
        {
            info!("{} body now {:?} texture {:?}", w.describe(*object), c.body_model, c.body_texture);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn attach_headgear(
    mut commands: Commands,
    events: Res<FrameEvents>,
    ents: Res<ObjEntities>,
    mut world: ResMut<WorldRes>,
    game: Res<GameRes>,
    mut cache: ResMut<AssetCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<KMaterial>>,
    roots: Query<(Entity, &ObjId, &ObjBody, Option<&HeadGear>)>,
    bindings: Query<&AnimBindings>,
) {
    let changed: Vec<ObjectId> = events
        .0
        .iter()
        .filter_map(|e| match e {
            WorldEvent::EquipmentChanged { creature, slot } if *slot == SLOT_HEAD => Some(*creature),
            _ => None,
        })
        .collect();
    let w = &mut world.0;
    let mut loader = Loader { res: &game.0, cache: &mut cache, images: &mut images, meshes: &mut meshes, materials: &mut materials };
    for (root, id, body, worn) in &roots {
        // AppearanceChanged unmaps (and despawns) the old root this frame.
        if ents.0.get(&id.0) != Some(&root) {
            continue;
        }
        if worn.is_some() && !changed.contains(&id.0) {
            continue;
        }
        if w.get(id.0).and_then(|o| o.creature()).is_none() {
            continue;
        }
        // The body's bindings appear a frame after spawn.
        let Some(b) = body.0.and_then(|e| bindings.get(e).ok()) else { continue };
        if let Some(h) = worn {
            for &e in &h.0 {
                commands.entity(e).despawn();
            }
        }
        let mut attached = Vec::new();
        if let Some((_, item)) = w.equipment(id.0).into_iter().find(|&(s, _)| s == SLOT_HEAD)
            && let Some(model_name) = w.item_def_of(item).and_then(|d| d.model.clone())
            && let Some(node) = b.entity("maskhook")
        {
            match loader.model(&model_name) {
                Some(t) => {
                    attached.push(model::spawn_template(&mut commands, &t, node, Transform::IDENTITY));
                    info!("{} wears {model_name}", w.describe(id.0));
                }
                None => warn!("head gear model {model_name} not loadable"),
            }
        }
        commands.entity(root).insert(HeadGear(attached));
    }
}
