//! Minigames on the Bevy side: the swoop/turret view (k2-world minigame.rs
//! does the simulation) and the pazaak screens (pazaak.rs).
//!
//! While the module has a minigame: the normal player control is blocked,
//! the party leader and the HUD are hidden, every follower and obstacle is
//! drawn from its models, bullets fly as their bullet models, and the camera
//! sits on the player's camera model `camerahook` node (so camera
//! animations such as shakes play). Turret aim turns the player's rotating
//! models (and the camera when CameraRotate is set).
//!
//! Controls: swoop A/D or Left/Right steer, S/Down/Space or right click
//! jump (gears shift by themselves; W/Up or left click shift early);
//! turret mouse aims, left mouse or Space fires.

pub mod pazaak;

use std::collections::HashMap;
use std::sync::Arc;

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::camera::visibility::VisibilitySystems;
use bevy::transform::TransformSystems;
use k2_script::ObjectId;
use k2_world::minigame::{MgEvent, MgInput, MgObject};

use crate::anim::clip::Clip;
use crate::anim::{AlphaOverlay, AnimBindings, AnimPlayer, AnimSystems, CreatureSpec, spawn_creature};
use crate::audio::Voice;
use crate::game::{AreaRoot, GameSet, InConversation, ObjEntities, WorldRes};
use crate::gui::hud::GameHud;
use crate::material::KMaterial;
use crate::model::{AssetCache, GameRes, Loader};

/// Value-less flags owned here (main skips them).
pub const SWITCHES: [&str; 1] = ["--mg-auto"];

pub struct MiniGamePlugin;

impl Plugin for MiniGamePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MgView>()
            .add_systems(Update, mg_input.in_set(GameSet::Input))
            .add_systems(Update, (mg_build, mg_sync, mg_shots).chain().in_set(GameSet::Ui))
            // After transforms propagate (the hook's global pose is current)
            // but before frusta and culling use the camera.
            .add_systems(PostUpdate, mg_camera.after(TransformSystems::Propagate).before(VisibilitySystems::UpdateFrusta))
            // Overlay clips pose their nodes after the main clip; the hidden
            // leader is re-hidden after the follow camera's visibility pass.
            .add_systems(PostUpdate, (mg_overlays.after(AnimSystems), mg_hide_leader).before(TransformSystems::Propagate))
            .add_plugins((pazaak::PazaakPlugin, crate::emitter::EmitterPlugin));
    }
}

/// Spawned minigame entities.
#[derive(Resource, Default)]
pub struct MgView {
    /// Area root the view was built under (rebuild on area change).
    built_for: Option<Entity>,
    /// Object -> (root, aim node, model bodies).
    objects: HashMap<ObjectId, MgEntity>,
    bullets: HashMap<u32, Entity>,
    /// The camerahook node of the player's camera model.
    camera_hook: Option<Entity>,
    hidden_leader: Option<Entity>,
    active: bool,
    /// Overlay clips (SWMG_PlayAnimation with bOverlay): HUD digits, dials.
    overlays: Vec<Overlay>,
}

struct Overlay {
    body: Entity,
    clip: Arc<Clip>,
    time: f32,
    looping: bool,
}

struct MgEntity {
    root: Entity,
    aim: Entity,
    bodies: Vec<Entity>,
    /// Looping engine sound (the follower's ARE Sounds/Engine).
    engine: Option<Entity>,
}

/// Falloff of follower sounds (minigame units are metres: swoops cover
/// 100-200 per second, turret targets sit 100-300 away).
const SOUND_RANGE: (f32, f32) = (15.0, 300.0);

/// Marker on minigame entities (despawned with the view).
#[derive(Component)]
struct MgPart;

fn mg_input(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    conv: Res<InConversation>,
    mut world: ResMut<WorldRes>,
    time: Res<Time>,
    mut skipped: Local<bool>,
) {
    let w = &mut world.0;
    let Some(mg) = w.minigame.as_ref() else { return };
    let turret = mg.is_turret();
    // The on-foot controller stays off for the whole minigame.
    w.input_blocked_for = w.input_blocked_for.max(0.25);
    if conv.0 {
        return;
    }
    let pressed = |k: &[KeyCode]| k.iter().any(|k| keys.pressed(*k));
    let just = |k: &[KeyCode]| k.iter().any(|k| keys.just_pressed(*k));
    let mut input = MgInput::default();
    // The autopilot shifts for itself; a player shifts with Up / W.
    w.auto_gears = std::env::args().any(|a| a == "--mg-auto");
    if turret {
        // Mouse X turns, mouse Y raises (screen up = aim up).
        input.aim = Vec2::new(-motion.delta.x, -motion.delta.y) * 0.15;
        // Arrow keys aim too.
        let k = 90.0 * time.delta_secs();
        if pressed(&[KeyCode::ArrowLeft, KeyCode::KeyA]) {
            input.aim.x += k;
        }
        if pressed(&[KeyCode::ArrowRight, KeyCode::KeyD]) {
            input.aim.x -= k;
        }
        if pressed(&[KeyCode::ArrowUp, KeyCode::KeyW]) {
            input.aim.y += k;
        }
        if pressed(&[KeyCode::ArrowDown, KeyCode::KeyS]) {
            input.aim.y -= k;
        }
        input.fire = buttons.pressed(MouseButton::Left) || pressed(&[KeyCode::Space]);
    } else {
        if pressed(&[KeyCode::KeyA, KeyCode::ArrowLeft]) {
            input.lateral -= 1.0;
        }
        if pressed(&[KeyCode::KeyD, KeyCode::ArrowRight]) {
            input.lateral += 1.0;
        }
        input.accelerate = just(&[KeyCode::KeyW, KeyCode::ArrowUp]) || buttons.just_pressed(MouseButton::Left);
        input.brake = just(&[KeyCode::KeyS, KeyCode::ArrowDown, KeyCode::Space]) || buttons.just_pressed(MouseButton::Right);
    }
    // `--mg-auto`: a crude autopilot for screenshots (turret fires; the swoop
    // needs no help, it accelerates by itself).
    if turret
        && std::env::args().any(|a| a == "--mg-auto")
        && let Some((yaw, pitch)) = auto_aim(mg)
    {
        input.aim = Vec2::new(yaw.clamp(-10.0, 10.0), pitch.clamp(-10.0, 10.0));
        input.fire = yaw.abs() < 4.0 && pitch.abs() < 4.0;
    }
    // The swoop autopilot also cruises: crashes cannot slow it below 120 (it
    // does not steer, so it would otherwise never get past the first crates).
    // K2_MG_SKIP=N jumps N units down the track once the race starts (to
    // look at a late set piece without driving there).
    if !turret && std::env::args().any(|a| a == "--mg-auto") {
        let skip = std::env::var("K2_MG_SKIP").ok().and_then(|v| v.parse::<f32>().ok());
        if let Some(mg) = w.minigame.as_mut().filter(|m| m.speed > 0.0) {
            if let Some(n) = skip.filter(|_| !*skipped) {
                mg.distance = n;
                *skipped = true;
            }
            mg.min_speed = mg.min_speed.max(120.0);
            mg.max_speed = mg.max_speed.max(125.0);
            mg.speed = mg.speed.max(120.0);
        }
    }
    w.minigame_input(input);
}

/// Degrees to turn (mouse-x axis, mouse-y axis) to face the nearest enemy.
fn auto_aim(mg: &k2_world::minigame::MiniGame) -> Option<(f32, f32)> {
    let p = mg.player.position;
    let e = mg.enemies.iter().filter(|e| e.is_live() && !e.trigger).min_by(|a, b| a.position.distance(p).total_cmp(&b.position.distance(p)))?;
    let local = mg.player.rotation.inverse() * (e.position - p).normalize_or_zero();
    let pitch = local.z.clamp(-1.0, 1.0).asin().to_degrees();
    let yaw = (-local.x).atan2(local.y).to_degrees();
    let dyaw = (yaw - mg.offset.z + 540.0).rem_euclid(360.0) - 180.0;
    let dpitch = pitch - mg.offset.x;
    let pick = |axis: u32| match axis {
        1 => dpitch,
        3 => dyaw,
        _ => 0.0,
    };
    Some((pick(mg.mouse_axis_x), pick(mg.mouse_axis_y)))
}

#[allow(clippy::too_many_arguments)]
fn mg_build(
    mut commands: Commands,
    world: Res<WorldRes>,
    game: Res<GameRes>,
    mut view: ResMut<MgView>,
    mut cache: ResMut<AssetCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<KMaterial>>,
    area: Query<Entity, With<AreaRoot>>,
    parts: Query<Entity, (With<MgPart>, Without<ChildOf>)>,
    ents: Res<ObjEntities>,
    mut vis: Query<&mut Visibility>,
    hud: Query<Entity, With<GameHud>>,
    mut voice: Voice,
) {
    let w = &world.0;
    let area_root = area.single().ok();
    let want = w.minigame.is_some() && area_root.is_some();
    if !want {
        if view.active {
            for e in &parts {
                commands.entity(e).despawn();
            }
            for (_, o) in view.objects.drain() {
                commands.entity(o.root).try_despawn();
            }
            for (_, b) in view.bullets.drain() {
                commands.entity(b).try_despawn();
            }
            if let Some(l) = view.hidden_leader.take()
                && let Ok(mut v) = vis.get_mut(l)
            {
                *v = Visibility::Inherited;
            }
            for h in &hud {
                if let Ok(mut v) = vis.get_mut(h) {
                    *v = Visibility::Inherited;
                }
            }
            view.active = false;
            view.built_for = None;
            view.camera_hook = None;
            view.overlays.clear();
        }
        return;
    }
    // Keep the HUD and the on-foot leader hidden.
    for h in &hud {
        if let Ok(mut v) = vis.get_mut(h) {
            *v = Visibility::Hidden;
        }
    }
    view.hidden_leader = ents.0.get(&w.party_leader()).copied();
    if view.active && view.built_for == area_root {
        return;
    }
    let area_root = area_root.unwrap();
    let Some(mg) = w.minigame.as_ref() else { return };
    let mut loader = Loader { res: &game.0, cache: &mut cache, images: &mut images, meshes: &mut meshes, materials: &mut materials };
    view.objects.clear();
    view.bullets.clear();
    view.camera_hook = None;
    let objects: Vec<&MgObject> = std::iter::once(&mg.player).chain(&mg.enemies).chain(&mg.obstacles).collect();
    for o in objects {
        let root = commands
            .spawn((Name::new(format!("mg {}", o.name)), MgPart, Transform::from_translation(o.position).with_rotation(o.rotation), Visibility::default(), ChildOf(area_root)))
            .id();
        let aim = commands.spawn((Name::new("mg aim"), MgPart, Transform::IDENTITY, Visibility::default(), ChildOf(root))).id();
        let mut bodies = Vec::new();
        let mut models: Vec<(String, bool)> = o.models.iter().map(|m| (m.name.clone(), m.rotating)).collect();
        if o.id == mg.player.id && !mg.camera_model.is_empty() && !models.iter().any(|(n, _)| *n == mg.camera_model) {
            models.push((mg.camera_model.clone(), mg.camera_rotate));
        }
        for (name, rotating) in models {
            let spec = CreatureSpec { body: name.clone(), head: None, texture: None, own_idle: true };
            let parent = if rotating { aim } else { root };
            match spawn_creature(&mut commands, &mut loader, &spec, parent, Transform::IDENTITY) {
                Some(b) => {
                    commands.entity(b).insert((MgPart, crate::emitter::FxLive));
                    bodies.push(b);
                    if o.id == mg.player.id && name == mg.camera_model {
                        commands.entity(b).insert(CameraModel);
                    }
                }
                None => debug!("minigame: model {name} not loadable"),
            }
        }
        // Followers hum their engine from where they are (the player's own
        // engine is flat: the scripts play it through sound objects).
        let engine = if o.id != mg.player.id && !o.engine_sound.is_empty() {
            let e = voice.play_loop_on(&o.engine_sound, root, SOUND_RANGE);
            if e.is_some() {
                info!("minigame: engine loop {} on {} ({:#x}, spatial)", o.engine_sound, o.name, o.id);
            }
            e
        } else {
            None
        };
        view.objects.insert(o.id, MgEntity { root, aim, bodies, engine });
    }
    info!(
        "minigame view: {} objects ({} enemies, {} obstacles), camera {}",
        view.objects.len(),
        mg.enemies.len(),
        mg.obstacles.len(),
        mg.camera_model
    );
    view.built_for = Some(area_root);
    view.active = true;
}

/// The player's camera model body.
#[derive(Component)]
struct CameraModel;

#[allow(clippy::too_many_arguments)]
fn mg_sync(
    mut commands: Commands,
    mut world: ResMut<WorldRes>,
    game: Res<GameRes>,
    mut view: ResMut<MgView>,
    mut tf: Query<&mut Transform, With<MgPart>>,
    mut players: Query<&mut AnimPlayer>,
    bindings: Query<&AnimBindings>,
    cam_model: Query<Entity, With<CameraModel>>,
    mut vis: Query<&mut Visibility>,
    mut cache: ResMut<AssetCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<KMaterial>>,
    area: Query<Entity, With<AreaRoot>>,
    mut voice: Voice,
) {
    if !view.active {
        return;
    }
    let events = world.0.drain_minigame_events();
    let w = &world.0;
    let Some(mg) = w.minigame.as_ref() else { return };
    // Camera hook, once the camera model's nodes exist.
    if view.camera_hook.is_none() {
        for b in &cam_model {
            if let Ok(bind) = bindings.get(b) {
                view.camera_hook = bind.entity("camerahook");
            }
        }
    }
    let area_root = area.single().ok();
    let aim_rot = if mg.is_turret() { Quat::from_rotation_z(mg.offset.z.to_radians()) * Quat::from_rotation_x(mg.offset.x.to_radians()) } else { Quat::IDENTITY };
    let mut silent = Vec::new();
    for o in std::iter::once(&mg.player).chain(&mg.enemies) {
        let Some(e) = view.objects.get(&o.id) else { continue };
        if !o.is_live() && e.engine.is_some() {
            silent.push(o.id);
        }
        if let Ok(mut t) = tf.get_mut(e.root) {
            t.translation = o.position;
            t.rotation = o.rotation;
        }
        if o.id == mg.player.id
            && let Ok(mut t) = tf.get_mut(e.aim)
        {
            t.rotation = aim_rot;
        }
        if o.removed
            && let Ok(mut v) = vis.get_mut(e.root)
        {
            *v = Visibility::Hidden;
        }
    }
    for id in silent {
        if let Some(en) = view.objects.get_mut(&id).and_then(|e| e.engine.take()) {
            commands.entity(en).try_despawn();
        }
    }
    for ev in events {
        match ev {
            MgEvent::Animation { object, anim, looping, overlay, .. } => {
                if anim == "die" {
                    info!("minigame: {object:#x} destroyed");
                } else if anim == "hit" {
                    info!("minigame: {object:#x} plays its hit animation ({} bodies)", view.objects.get(&object).map_or(0, |e| e.bodies.len()));
                }
                debug!("minigame: animation {anim} on {object:#x} (looping {looping}, overlay {overlay})");
                let bodies = view.objects.get(&object).map(|e| e.bodies.clone()).unwrap_or_default();
                for b in bodies {
                    let Ok(mut p) = players.get_mut(b) else { continue };
                    if overlay {
                        // Layer on top: replace overlays driving the same nodes.
                        if let Some(clip) = p.set.clip(&anim) {
                            // A running damage flicker (the swoop's shield) is not
                            // cut short by the gear shift that follows a crash.
                            view.overlays.retain(|o| o.body != b || is_running_damage(o) || !o.clip.tracks.keys().any(|k| clip.tracks.contains_key(k)));
                            view.overlays.push(Overlay { body: b, clip, time: 0.0, looping });
                        }
                    } else {
                        p.play(&anim, looping, 1.0);
                    }
                }
            }
            MgEvent::RemoveAnimation { object, anim } => {
                let bodies = view.objects.get(&object).map(|e| e.bodies.clone()).unwrap_or_default();
                view.overlays.retain(|o| !bodies.contains(&o.body) || o.clip.name != anim);
            }
            MgEvent::SpeedBlur { .. } => {}
            MgEvent::Sound { sound, position } => {
                // The player's own sounds are flat; everything else comes
                // from where it happened.
                let flat = position.distance(mg.player.position) < 3.0;
                let played = match area_root {
                    Some(root) if !flat => voice.play_sound_at(&sound, root, position, SOUND_RANGE),
                    _ => voice.play_sound(&sound),
                };
                if played.is_some() {
                    info!("minigame: sound {sound} at ({:.0}, {:.0}, {:.0}) ({})", position.x, position.y, position.z, if flat { "flat" } else { "spatial" });
                }
            }
            MgEvent::Removed { object } => {
                if let Some(e) = view.objects.get(&object)
                    && let Ok(mut v) = vis.get_mut(e.root)
                {
                    *v = Visibility::Hidden;
                }
            }
        }
    }
    // Bullets.
    let live: Vec<(u32, Vec3, Vec3, String)> = mg.bullets.iter().map(|b| (b.id, b.position, b.velocity, b.model.clone())).collect();
    let Some(area_root) = area_root else { return };
    let mut loader = Loader { res: &game.0, cache: &mut cache, images: &mut images, meshes: &mut meshes, materials: &mut materials };
    for (id, pos, vel, model) in &live {
        let rot = Quat::from_rotation_arc(Vec3::Y, vel.normalize_or(Vec3::Y));
        match view.bullets.get(id) {
            Some(e) => {
                if let Ok(mut t) = tf.get_mut(*e) {
                    t.translation = *pos;
                    t.rotation = rot;
                }
            }
            None => {
                let root = commands.spawn((Name::new("mg bullet"), MgPart, Transform::from_translation(*pos).with_rotation(rot), Visibility::default(), ChildOf(area_root))).id();
                let spec = CreatureSpec { body: model.clone(), head: None, texture: None, own_idle: true };
                if let Some(b) = spawn_creature(&mut commands, &mut loader, &spec, root, Transform::IDENTITY) {
                    commands.entity(b).insert(MgPart);
                }
                view.bullets.insert(*id, root);
            }
        }
    }
    let gone: Vec<u32> = view.bullets.keys().filter(|id| !live.iter().any(|l| l.0 == **id)).copied().collect();
    for id in gone {
        if let Some(e) = view.bullets.remove(&id) {
            commands.entity(e).try_despawn();
        }
    }
}

/// Puts the game camera on the camera model's `camerahook` (looking down
/// its -Z, up +Y, which is also Bevy's camera convention).
fn mg_camera(view: Res<MgView>, conv: Res<InConversation>, hooks: Query<&GlobalTransform, Without<Camera3d>>, mut cams: Query<(&mut Transform, &mut GlobalTransform), With<Camera3d>>) {
    if !view.active || conv.0 {
        return;
    }
    let Some(hook) = view.camera_hook.and_then(|h| hooks.get(h).ok()) else { return };
    for (mut t, mut g) in &mut cams {
        *g = *hook;
        *t = hook.compute_transform();
    }
}

/// Poses overlay clips on top of whatever the main clip did this frame.
fn is_running_damage(o: &Overlay) -> bool {
    o.clip.name == "damage" && o.time < o.clip.length
}

fn mg_overlays(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut view: ResMut<MgView>,
    bindings: Query<&AnimBindings>,
    mut tf: Query<&mut Transform>,
    with_alpha: Query<Entity, With<AlphaOverlay>>,
    with_fx: Query<Entity, With<crate::emitter::FxClips>>,
) {
    let dt = time.delta_secs();
    // Alpha tracks of the overlays (the swoop's shield) per body.
    let mut alphas: HashMap<Entity, HashMap<String, f32>> = HashMap::new();
    for o in &mut view.overlays {
        o.time += dt;
    }
    // A finished damage flicker is dropped; a running one is applied last so
    // it wins over the gear clips that share its nodes.
    view.overlays.retain(|o| o.clip.name != "damage" || o.time < o.clip.length);
    let mut order: Vec<usize> = (0..view.overlays.len()).collect();
    order.sort_by_key(|&i| is_running_damage(&view.overlays[i]));
    for i in order {
        let o = &view.overlays[i];
        let len = o.clip.length.max(1e-3);
        let t = if o.looping { o.time % len } else { o.time.min(len) };
        let Ok(bind) = bindings.get(o.body) else { continue };
        for (key, e, rest_t, rest_r) in bind.rest_poses() {
            let Some(track) = o.clip.tracks.get(key) else { continue };
            if let Some(a) = track.sample_alpha(t) {
                alphas.entry(o.body).or_default().insert(key.to_owned(), a);
            }
            let Ok(mut x) = tf.get_mut(e) else { continue };
            if let Some(d) = track.sample_pos(t) {
                x.translation = rest_t + d * bind.anim_scale;
            }
            x.rotation = track.sample_rot(t).unwrap_or(rest_r);
            if let Some(sc) = track.sample_scale(t) {
                x.scale = Vec3::splat(sc);
            }
        }
    }
    // Overlay clips drive emitters too (the swoop's damage sparks).
    let mut fx: HashMap<Entity, Vec<(Arc<Clip>, f32)>> = HashMap::new();
    for o in &view.overlays {
        let len = o.clip.length.max(1e-3);
        let t = if o.looping { o.time % len } else { o.time.min(len) };
        fx.entry(o.body).or_default().push((o.clip.clone(), t));
    }
    for e in &with_fx {
        if !fx.contains_key(&e) {
            commands.entity(e).remove::<crate::emitter::FxClips>();
        }
    }
    for (e, v) in fx {
        commands.entity(e).insert(crate::emitter::FxClips(v));
    }
    for e in &with_alpha {
        if !alphas.contains_key(&e) {
            commands.entity(e).remove::<AlphaOverlay>();
        }
    }
    for (e, map) in alphas {
        commands.entity(e).insert(AlphaOverlay(map));
    }
}

/// Test hook (with K2_TEST_WINDOW=1): `K2_MG_SHOT=PATH,SECS[,SECS...]` takes
/// a screenshot when the minigame clock reaches each SECS (`{}` in PATH
/// becomes SECS; fresh paths) and exits after the last. With `K2_MG_CRASH`
/// (world side) this frames the swoop's crash effects.
fn mg_shots(
    mut commands: Commands,
    world: Res<WorldRes>,
    mut exit: MessageWriter<AppExit>,
    mut taken: Local<Vec<std::path::PathBuf>>,
) {
    let Ok(spec) = std::env::var("K2_MG_SHOT") else { return };
    let Some(mg) = world.0.minigame.as_ref() else { return };
    let mut parts = spec.split(',');
    let path = parts.next().unwrap_or_default().to_owned();
    let shots: Vec<(f32, std::path::PathBuf)> =
        parts.filter_map(|p| Some((p.parse().ok()?, std::path::PathBuf::from(path.replace("{}", p))))).collect();
    for (secs, p) in &shots {
        if mg.elapsed >= *secs && !taken.contains(p) {
            info!("mg shot at {:.2}s: {}", mg.elapsed, p.display());
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p.clone()));
            taken.push(p.clone());
        }
    }
    if !shots.is_empty() && taken.len() == shots.len() && taken.iter().all(|p| p.exists()) {
        exit.write(AppExit::Success);
    }
}

/// Keeps the on-foot leader hidden while the minigame runs (the follow
/// camera re-shows it every frame).
fn mg_hide_leader(view: Res<MgView>, mut vis: Query<&mut Visibility>) {
    if view.active
        && let Some(l) = view.hidden_leader
        && let Ok(mut v) = vis.get_mut(l)
        && *v != Visibility::Hidden
    {
        *v = Visibility::Hidden;
    }
}
