//! K2 holograms: creatures whose UTC has `Hologram` set (the recorded
//! speakers of security/medical console logs, e.g. 101per MedOff) are drawn
//! as a blue scanlined projection, and meshes flagged `hide_in_holograms`
//! (teeth, tongue) are not drawn. The look is alpha-tested, not blended, so
//! it writes depth and nothing behind the front surface shows through.
//!
//! An object root gets `HologramLook`; every mesh below it (body, head,
//! weapons, attached later too) swaps its material for a hologram variant.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::material::{Blend, FLAG_ALPHA_MASK, FLAG_HOLO, FLAG_LIGHTMAP, KMaterial};

pub struct HoloPlugin;

impl Plugin for HoloPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HoloCache>().add_systems(PostUpdate, (apply_holo, flicker));
    }
}

/// Marks an object root to be drawn as a hologram.
#[derive(Component)]
pub struct HologramLook;

/// Mesh entity whose node is flagged hide_in_holograms.
#[derive(Component)]
pub struct HideInHolo;

/// Mesh already converted.
#[derive(Component)]
struct HoloDone;

/// Original material -> hologram variant, shared by every hologram.
#[derive(Resource, Default)]
struct HoloCache(HashMap<AssetId<KMaterial>, Handle<KMaterial>>);

fn apply_holo(
    mut commands: Commands,
    roots: Query<Entity, With<HologramLook>>,
    children: Query<&Children>,
    mut meshes: Query<(&mut MeshMaterial3d<KMaterial>, Has<HideInHolo>), Without<HoloDone>>,
    mut materials: ResMut<Assets<KMaterial>>,
    mut cache: ResMut<HoloCache>,
) {
    for root in &roots {
        for e in children.iter_descendants(root) {
            let Ok((mut mat, hide)) = meshes.get_mut(e) else { continue };
            commands.entity(e).insert(HoloDone);
            if hide {
                commands.entity(e).insert(Visibility::Hidden);
                continue;
            }
            let id = mat.0.id();
            let holo = match cache.0.get(&id) {
                Some(h) => h.clone(),
                None => {
                    let Some(src) = materials.get(id).cloned() else { continue };
                    let mut m = src;
                    // Alpha-tested opaque: writes depth, keeps texture
                    // cutouts (hair), and the shader's scanline discard
                    // supplies the transparency.
                    m.params.flags = (m.params.flags & !FLAG_LIGHTMAP) | FLAG_HOLO | FLAG_ALPHA_MASK;
                    m.params.alpha_cut = m.params.alpha_cut.max(0.5);
                    m.params._pad0 = 1.0;
                    m.blend = Blend::Opaque;
                    let h = materials.add(m);
                    cache.0.insert(id, h.clone());
                    h
                }
            };
            mat.0 = holo;
        }
    }
}

/// Projector flicker: a slow shimmer with occasional dips, shared by all.
fn flicker(time: Res<Time>, cache: Res<HoloCache>, mut materials: ResMut<Assets<KMaterial>>) {
    if cache.0.is_empty() {
        return;
    }
    let t = time.elapsed_secs();
    let dip = if (t * 7.3).sin() * (t * 3.1).sin() > 0.93 { 0.55 } else { 1.0 };
    let k = (0.88 + 0.12 * (t * 23.0).sin().abs()) * dip;
    for h in cache.0.values() {
        if let Some(mut m) = materials.get_mut(h) {
            m.params._pad0 = k;
        }
    }
}
