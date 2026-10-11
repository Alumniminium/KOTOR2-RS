//! Shadows: one dynamic shadow-casting directional light that the KMaterial
//! shader samples (material.rs), and a soft blob under every creature.
//!
//! The original darkens the floor under creatures and props with the
//! area's ShadowOpacity; none of the shipped area lights carry a usable
//! `shadow` flag, so the shadow light is synthesised. It is aimed from the
//! nearest model light node towards the party leader (clamped near vertical,
//! eased as the nearest light changes) and falls back to a fixed sun. Rooms
//! never cast (their lightmaps hold the baked shadows); creatures, doors and
//! placeables cast when the MDL mesh `shadow` flag is set. The light's colour
//! carries the shadow darkness for the shader. See docs/notes/rendering.md.

use bevy::asset::RenderAssetUsages;
use bevy::light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap, NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::area::AreaModule;
use crate::coords;
use crate::game::{AreaRoot, CreatureAnim, ObjId, WorldRes};
use crate::graphics::{GraphicsSettings, ShadowQuality};
use crate::play::{Player, RoomModel};

/// A light node of a model (radius in metres).
#[derive(Component, Clone, Copy, Debug)]
pub struct ModelLight {
    pub radius: f32,
}

/// The one shadow-casting directional light.
#[derive(Component)]
struct ShadowSun;

/// The soft disc under a creature.
#[derive(Component)]
struct Blob;

#[derive(Resource)]
struct BlobAssets {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

/// Light node entities of the current area's rooms.
#[derive(Resource, Default)]
struct RoomLights(Vec<Entity>);

/// Darkness of the blob when a real shadow covers the ground as well.
const BLOB_ALPHA_WITH_SHADOW: f32 = 0.4;
const BLOB_ALPHA: f32 = 0.6;
/// Lights farther than this (horizontally) do not aim the shadow.
const LIGHT_REACH: f32 = 14.0;
/// Widest angle of the shadow ray from vertical (radians).
const MAX_TILT: f32 = 0.6;
/// Shadow ray when no light applies (Bevy space, towards the ground).
const DEFAULT_RAY: Vec3 = Vec3::new(-0.3, -1.0, -0.2);
/// Darkness cap: the area's ShadowOpacity (0..255) scaled by this.
const OPACITY_SCALE: f32 = 0.9;

pub struct ShadowPlugin;

impl Plugin for ShadowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoomLights>()
            .add_systems(Startup, setup)
            .add_systems(Update, (env_override, scan_rooms, apply_quality, aim_sun, add_blobs, blob_visibility, frame_log).chain());
    }
}

fn blob_image() -> Image {
    const N: u32 = 64;
    let mut data = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = ((x as f32 + 0.5) / N as f32 * 2.0 - 1.0, (y as f32 + 0.5) / N as f32 * 2.0 - 1.0);
            let r = (dx * dx + dy * dy).sqrt();
            // Dense core, long soft edge.
            let a = (1.0 - r).clamp(0.0, 1.0);
            let a = a * a * (3.0 - 2.0 * a);
            data.extend_from_slice(&[0, 0, 0, (a * 255.0) as u8]);
        }
    }
    Image::new(Extent3d { width: N, height: N, depth_or_array_layers: 1 }, TextureDimension::D2, data, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD)
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>, mut images: ResMut<Assets<Image>>) {
    commands.spawn((
        Name::new("shadow sun"),
        ShadowSun,
        DirectionalLight { illuminance: 0.8, shadow_maps_enabled: false, ..default() },
        Transform::IDENTITY.looking_to(DEFAULT_RAY, Vec3::Z),
    ));
    let tex = images.add(blob_image());
    commands.insert_resource(BlobAssets {
        // A unit-half-size quad in the XY plane facing +Z (game up).
        mesh: meshes.add(Rectangle::new(2.0, 2.0)),
        material: materials.add(StandardMaterial {
            base_color: Color::srgba(1.0, 1.0, 1.0, BLOB_ALPHA),
            base_color_texture: Some(tex),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            cull_mode: None,
            fog_enabled: false,
            depth_bias: 4.0,
            ..default()
        }),
    });
}

/// K2_SHADOWS=off|low|high overrides the setting for test runs.
fn env_override(mut gfx: ResMut<GraphicsSettings>, mut done: Local<bool>) {
    if std::mem::replace(&mut *done, true) {
        return;
    }
    let Ok(v) = std::env::var("K2_SHADOWS") else { return };
    let q = match v.trim().to_ascii_lowercase().as_str() {
        "off" | "0" => ShadowQuality::Off,
        "low" | "1" => ShadowQuality::Low,
        "high" | "2" => ShadowQuality::High,
        other => {
            warn!("K2_SHADOWS={other}: expected off, low or high");
            return;
        }
    };
    gfx.shadows = q;
}

/// New room roots: rooms never cast (baked lightmaps), and their light nodes
/// become the candidates that aim the shadow.
fn scan_rooms(
    mut commands: Commands,
    mut lights: ResMut<RoomLights>,
    roots: Query<Entity, (Added<RoomModel>, Without<ObjId>)>,
    area: Query<Entity, Added<AreaRoot>>,
    children: Query<&Children>,
    meshes: Query<(), With<Mesh3d>>,
    model_lights: Query<&ModelLight>,
) {
    if !area.is_empty() {
        lights.0.clear();
    }
    for r in &roots {
        for d in children.iter_descendants(r) {
            if meshes.contains(d) {
                commands.entity(d).insert(NotShadowCaster);
            }
            if model_lights.get(d).is_ok_and(|l| l.radius > 0.0) {
                lights.0.push(d);
            }
        }
    }
}

/// Shadow map size, cascades and darkness for the quality setting and the
/// area's ShadowOpacity; Off also hides the blobs (blob_visibility).
fn apply_quality(
    gfx: Res<GraphicsSettings>,
    area: Option<Res<AreaModule>>,
    mut map: ResMut<DirectionalLightShadowMap>,
    mut sun: Query<&mut DirectionalLight, With<ShadowSun>>,
    mut commands: Commands,
    sun_entity: Query<Entity, With<ShadowSun>>,
    mut last: Local<Option<(ShadowQuality, u32)>>,
) {
    let opacity = area.as_ref().map_or(205, |a| a.0.area.shadow_opacity.min(255));
    let key = (gfx.shadows, opacity);
    if *last == Some(key) {
        return;
    }
    *last = Some(key);
    let (size, cascades, reach) = match gfx.shadows {
        ShadowQuality::Off => (1024, 1, 1.0),
        ShadowQuality::Low => (1024, 2, 22.0),
        ShadowQuality::High => (2048, 3, 40.0),
    };
    if map.size != size {
        map.size = size;
    }
    for mut l in &mut sun {
        l.shadow_maps_enabled = gfx.shadows.is_on();
        // The shader reads the darkness from the light colour.
        l.color = Color::WHITE;
        l.illuminance = opacity as f32 / 255.0 * OPACITY_SCALE;
        l.shadow_depth_bias = 0.04;
        l.shadow_normal_bias = 1.2;
    }
    for e in &sun_entity {
        commands.entity(e).insert(CascadeShadowConfigBuilder { num_cascades: cascades, minimum_distance: 0.1, maximum_distance: reach, first_cascade_far_bound: reach * 0.12, overlap_proportion: 0.2 }.build());
    }
    info!("shadows: {:?}, map {size}, {cascades} cascades to {reach} m, darkness {:.2}", gfx.shadows, opacity as f32 / 255.0 * OPACITY_SCALE);
}

/// Points the shadow light from the nearest area light at the party leader.
fn aim_sun(
    time: Res<Time>,
    lights: Res<RoomLights>,
    gts: Query<(&GlobalTransform, &ModelLight)>,
    player: Query<&Player>,
    mut sun: Query<&mut Transform, With<ShadowSun>>,
    mut ray: Local<Option<Vec3>>,
) {
    let Ok(mut t) = sun.single_mut() else { return };
    let want = player
        .single()
        .ok()
        .and_then(|p| {
            let at = coords::to_bevy(p.pos) + Vec3::Y * 0.9;
            let flat = |v: Vec3| Vec2::new(v.x, v.z);
            let nearest = lights
                .0
                .iter()
                .filter_map(|e| gts.get(*e).ok())
                .map(|(g, l)| (g.translation(), l))
                .filter(|(pos, _)| pos.y > at.y + 0.5)
                .min_by(|a, b| flat(a.0 - at).length().total_cmp(&flat(b.0 - at).length()))?;
            let (pos, _) = nearest;
            let h = flat(at - pos);
            if h.length() > LIGHT_REACH {
                return None;
            }
            let down = pos.y - at.y;
            let h = h.clamp_length_max(down * MAX_TILT.tan());
            Some(Vec3::new(h.x, -down, h.y).normalize())
        })
        .unwrap_or(DEFAULT_RAY.normalize());
    let cur = ray.get_or_insert(want);
    *cur = cur.lerp(want, 1.0 - (-4.0 * time.delta_secs()).exp()).normalize_or(want);
    t.rotation = Quat::from_rotation_arc(Vec3::NEG_Z, *cur);
}

/// A blob under every new creature, sized from its collision radius.
fn add_blobs(mut commands: Commands, assets: Option<Res<BlobAssets>>, world: Res<WorldRes>, new: Query<(Entity, &ObjId), Added<CreatureAnim>>) {
    let Some(assets) = assets else { return };
    for (e, id) in &new {
        let r = world.0.get(id.0).and_then(|o| o.creature()).map_or(0.5, |c| c.radius);
        let half = (r * 1.15).clamp(0.3, 2.5);
        commands.entity(e).with_child((
            Name::new("blob shadow"),
            Blob,
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(assets.material.clone()),
            Transform::from_translation(Vec3::new(0.0, 0.0, 0.03)).with_scale(Vec3::new(half, half, 1.0)),
            NotShadowCaster,
            NotShadowReceiver,
        ));
    }
}

/// Blobs follow the quality setting; with a real shadow on the ground they
/// fade back to a contact shadow.
fn blob_visibility(
    gfx: Res<GraphicsSettings>,
    assets: Option<Res<BlobAssets>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut blobs: Query<&mut Visibility, With<Blob>>,
    added: Query<(), Added<Blob>>,
) {
    if !gfx.is_changed() && added.is_empty() {
        return;
    }
    let Some(assets) = assets else { return };
    let vis = if gfx.shadows.is_on() { Visibility::Inherited } else { Visibility::Hidden };
    for mut v in &mut blobs {
        if *v != vis {
            *v = vis;
        }
    }
    if let Some(mut m) = materials.get_mut(&assets.material) {
        let a = if gfx.shadows == ShadowQuality::High { BLOB_ALPHA_WITH_SHADOW } else { BLOB_ALPHA };
        m.base_color = Color::srgba(1.0, 1.0, 1.0, a);
    }
}

/// K2_FRAMELOG=1 logs the mean frame time every 120 frames (shadow cost
/// measurements).
fn frame_log(time: Res<Time<Real>>, mut acc: Local<(u32, f32)>, mut enabled: Local<Option<bool>>) {
    if !*enabled.get_or_insert_with(|| std::env::var_os("K2_FRAMELOG").is_some()) {
        return;
    }
    acc.0 += 1;
    acc.1 += time.delta_secs();
    if acc.0 == 120 {
        info!("frametime: {:.2} ms mean over 120 frames ({:.0} fps)", acc.1 / 120.0 * 1000.0, 120.0 / acc.1);
        *acc = (0, 0.0);
    }
}
