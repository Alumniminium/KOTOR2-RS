//! Model viewer: `kotor2 --viewer MODEL [--head HEAD] [--tex TEX]
//! [--anim NAME] [--atime SECONDS] [--yaw DEG]`.
//!
//! Shows one creature on a grey plane. Left mouse drag orbits, wheel zooms,
//! Space/Backspace step through the animations of the supermodel chain.
//! `--atime` freezes the animation at that time (for screenshots).
//! `--dist M` and `--focus H` set the orbit distance and target height.
//! `--lip RESREF` plays that LIP track on the model's face (from t = 0).
//! `--holo 1` draws the model as a K2 hologram.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;

use super::{AnimPlayer, CreatureSpec, spawn_creature};
use crate::coords::{self, WorldRoot};
use crate::material::KMaterial;
use crate::model::{AssetCache, GameRes, Loader};

#[derive(Resource, Clone, Debug, Default)]
pub struct ViewerArgs {
    pub model: String,
    pub head: Option<String>,
    pub tex: Option<String>,
    pub anim: Option<String>,
    pub atime: Option<f32>,
    pub yaw: f32,
    pub dist: f32,
    pub focus: f32,
}

pub struct ViewerPlugin;

impl Plugin for ViewerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(super::lipsync::LipSyncPlugin).add_systems(Startup, setup).add_systems(Update, (orbit, step_anims, log_nodes));
    }
}

#[derive(Component)]
struct Viewed;

#[derive(Component)]
struct Orbit {
    yaw: f32,
    pitch: f32,
    dist: f32,
    target: Vec3,
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    args: Res<ViewerArgs>,
    game: Res<GameRes>,
    mut cache: ResMut<AssetCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<KMaterial>>,
    mut std_mats: ResMut<Assets<StandardMaterial>>,
) {
    let world = commands.spawn((Name::new("world"), WorldRoot, coords::world_root_transform(), Visibility::default())).id();
    let mut loader = Loader { res: &game.0, cache: &mut cache, images: &mut images, meshes: &mut meshes, materials: &mut materials };
    let spec = CreatureSpec { body: args.model.clone(), head: args.head.clone(), texture: args.tex.clone(), own_idle: false };
    match spawn_creature(&mut commands, &mut loader, &spec, world, Transform::IDENTITY) {
        Some(e) => {
            commands.entity(e).insert(Viewed);
            if get_flag("--holo").is_some_and(|v| v != "0") {
                commands.entity(e).insert(crate::holo::HologramLook);
            }
            if let Some(lip) = get_flag("--lip") {
                let mut index = super::lipsync::LipIndex::default();
                match index.get(&game.0, &lip) {
                    Some(d) => {
                        info!("viewer: lip {lip}: {} keys, {:.1}s", d.keys.len(), d.length);
                        commands.entity(e).insert(super::lipsync::LipSync::new(d));
                    }
                    None => warn!("viewer: lip {lip} not found"),
                }
            }
        }
        None => error!("viewer: model {} not found", args.model),
    }
    if let Some(set) = super::AnimSet::resolve(&mut loader, &args.model) {
        info!("animations: {}", set.names().join(" "));
    }

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, 8.0))),
        MeshMaterial3d(std_mats.add(StandardMaterial { base_color: Color::srgb(0.35, 0.35, 0.37), perceptual_roughness: 1.0, ..default() })),
    ));
    commands.spawn((DirectionalLight { illuminance: 6000.0, ..default() }, Transform::from_xyz(3.0, 6.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y)));
    let orbit = Orbit { yaw: args.yaw.to_radians(), pitch: 0.15, dist: args.dist, target: Vec3::new(0.0, args.focus, 0.0) };
    let tf = orbit_transform(&orbit);
    commands.spawn((Camera3d::default(), tf, orbit));
}

/// Yaw 0 looks at the model's front (game +Y faces the camera).
fn orbit_transform(o: &Orbit) -> Transform {
    let game_dir = Vec3::new(o.yaw.sin(), o.yaw.cos(), 0.0);
    let offset = coords::to_bevy(game_dir) * o.pitch.cos() + Vec3::Y * o.pitch.sin();
    Transform::from_translation(o.target + offset * o.dist).looking_at(o.target, Vec3::Y)
}

fn orbit(
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    mut q: Query<(&mut Orbit, &mut Transform)>,
) {
    for (mut o, mut tf) in &mut q {
        if buttons.pressed(MouseButton::Left) {
            o.yaw -= motion.delta.x * 0.01;
            o.pitch = (o.pitch + motion.delta.y * 0.01).clamp(-1.2, 1.4);
        }
        o.dist = (o.dist * (1.0 - scroll.delta.y * 0.1)).clamp(0.5, 30.0);
        *tf = orbit_transform(&o);
    }
}

fn step_anims(
    keys: Res<ButtonInput<KeyCode>>,
    args: Res<ViewerArgs>,
    mut started: Local<bool>,
    mut index: Local<usize>,
    mut q: Query<&mut AnimPlayer, With<Viewed>>,
) {
    for mut p in &mut q {
        let names = p.set.names();
        if names.is_empty() {
            continue;
        }
        if !*started {
            *started = true;
            let want = args.anim.clone().unwrap_or_else(|| "pause1".into());
            *index = names.iter().position(|n| *n == want.to_ascii_lowercase()).unwrap_or(0);
            let n = names[*index].clone();
            // A frozen time means a still: no advance between seek and sample.
            p.play(&n, true, if args.atime.is_some() { 0.0 } else { 1.0 });
            info!("playing {n}");
        }
        let step = if keys.just_pressed(KeyCode::Space) {
            1
        } else if keys.just_pressed(KeyCode::Backspace) {
            names.len() - 1
        } else {
            0
        };
        if step != 0 {
            *index = (*index + step) % names.len();
            let n = names[*index].clone();
            p.fade_to(&n, true, 1.0, 0.25);
            info!("playing {n}");
        }
        if let Some(t) = args.atime {
            p.seek(t);
        }
    }
}

/// Viewer flags from the command line; None unless `--viewer` is present.
/// main's parser skips these flags.
pub fn from_env() -> Option<ViewerArgs> {
    let argv: Vec<String> = std::env::args().collect();
    let get = |flag: &str| argv.iter().position(|a| a == flag).and_then(|i| argv.get(i + 1)).cloned();
    Some(ViewerArgs {
        model: get("--viewer")?.to_ascii_lowercase(),
        head: get("--head"),
        tex: get("--tex"),
        anim: get("--anim"),
        atime: get("--atime").and_then(|v| v.parse().ok()),
        yaw: get("--yaw").and_then(|v| v.parse().ok()).unwrap_or(0.0),
        dist: get("--dist").and_then(|v| v.parse().ok()).unwrap_or(3.2),
        focus: get("--focus").and_then(|v| v.parse().ok()).unwrap_or(1.0),
    })
}

/// Flags owned by the viewer, each taking one value.
pub const FLAGS: [&str; 10] = ["--viewer", "--head", "--tex", "--anim", "--atime", "--yaw", "--dist", "--focus", "--lip", "--holo"];

fn get_flag(flag: &str) -> Option<String> {
    let argv: Vec<String> = std::env::args().collect();
    argv.iter().position(|a| a == flag).and_then(|i| argv.get(i + 1)).cloned()
}

/// `K2_NODEDBG=1`: logs world positions (game space, model at the origin
/// facing +Y) of key skeleton nodes once a second, to measure poses.
fn log_nodes(time: Res<Time>, mut next: Local<f32>, roots: Query<&super::AnimBindings>, gts: Query<&GlobalTransform>) {
    if std::env::var_os("K2_NODEDBG").is_none() || time.elapsed_secs() < *next {
        return;
    }
    *next = time.elapsed_secs() + 1.0;
    for b in &roots {
        for n in ["rootdummy", "pelvis_g", "head_g", "headhook", "lfoot_g", "rfoot_g", "lhand_g"] {
            if let Some(e) = b.entity(n)
                && let Ok(g) = gts.get(e)
            {
                info!("node {n} at {:?}", coords::to_game(g.translation()));
            }
        }
    }
}
