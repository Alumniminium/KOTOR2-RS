//! Screen overlays: fade, skipped-movie card, floating barks and damage
//! numbers, and the debug line (area, FPS, leader position; F3 or
//! `--debug`). Party and target health are drawn by the GUI HUD
//! (`gui::hud`).

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use k2_script::ObjectId;
use k2_world::WorldEvent;

use crate::Args;
use crate::audio::Voice;
use crate::game::{AppState, Fade, FrameEvents, GameSet, MovieCards, ObjEntities, PendingBarks, WorldRes};

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<FrameTimeDiagnosticsPlugin>() {
            app.add_plugins(FrameTimeDiagnosticsPlugin::default());
        }
        app.init_resource::<ShowFps>()
            .init_resource::<LastTarget>()
            .add_systems(Startup, spawn_hud)
            .add_systems(Update, (world_fx, update_fade, update_card, update_barks, update_hud, forget_dead_target).chain().in_set(GameSet::Ui))
            .add_systems(OnExit(AppState::Game), hide_hud);
    }
}

#[derive(Resource, Default)]
struct ShowFps(bool);

#[derive(Component)]
struct FadeOverlay;
#[derive(Component)]
struct MovieCard;
#[derive(Component)]
struct HudLine;

/// A floating line over an object; damage numbers rise.
#[derive(Component)]
struct Bark {
    object: ObjectId,
    left: f32,
    life: f32,
    rise: bool,
}

/// The creature the player last attacked (target bar fallback).
#[derive(Resource, Default)]
pub struct LastTarget(pub Option<ObjectId>);

fn spawn_hud(mut commands: Commands, args: Res<Args>) {
    commands.insert_resource(ShowFps(args.debug));
    commands.spawn((
        Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        BackgroundColor(Color::NONE),
        GlobalZIndex(100),
        Pickable::IGNORE,
        FadeOverlay,
    ));
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::BLACK),
            Visibility::Hidden,
            GlobalZIndex(110),
            MovieCard,
        ))
        .with_child((Text::new(""), TextFont::from_font_size(22.0), TextColor(Color::srgb(0.7, 0.7, 0.7))));
    commands.spawn((
        Node { position_type: PositionType::Absolute, left: Val::Px(10.0), top: Val::Px(8.0), ..default() },
        Text::new(""),
        TextFont::from_font_size(16.0),
        TextColor(Color::srgb(0.85, 0.85, 0.85)),
        TextShadow::default(),
        GlobalZIndex(50),
        HudLine,
    ));
}

fn hide_hud(mut q: Query<&mut Text, With<HudLine>>) {
    for mut t in &mut q {
        t.0.clear();
    }
}

/// Fades, barks and one-shot sounds requested by scripts.
fn world_fx(
    mut commands: Commands,
    events: Res<FrameEvents>,
    mut local: ResMut<PendingBarks>,
    mut fade: ResMut<Fade>,
    mut last: ResMut<LastTarget>,
    world: Res<WorldRes>,
    mut voice: Voice,
) {
    let leader = world.0.party_leader();
    let local: Vec<WorldEvent> = local.0.drain(..).map(|(object, text)| WorldEvent::Bark { object, text }).collect();
    for e in events.0.iter().chain(&local) {
        match e {
            WorldEvent::FadeOut { wait, length, color } => fade.to(1.0, *wait, *length, *color),
            WorldEvent::FadeIn { wait, length, color } => fade.to(0.0, *wait, *length, *color),
            // AI scripts "shout" tokens like GEN_COMBAT_ACTIVE with silent
            // volumes (SpeakString's volume is not in the event); never shown.
            WorldEvent::Bark { object, text } | WorldEvent::Speak { object, text } if !text.trim().is_empty() && !is_shout_token(text) => {
                commands.spawn((
                    Node { position_type: PositionType::Absolute, ..default() },
                    Text::new(text.clone()),
                    TextFont::from_font_size(17.0),
                    TextColor(Color::srgb(0.95, 0.95, 0.8)),
                    TextShadow::default(),
                    Visibility::Hidden,
                    GlobalZIndex(30),
                    Bark { object: *object, left: 4.0, life: 4.0, rise: false },
                ));
            }
            WorldEvent::Damaged { object, amount, .. } if *amount > 0 => {
                commands.spawn((
                    Node { position_type: PositionType::Absolute, ..default() },
                    Text::new(amount.to_string()),
                    TextFont::from_font_size(20.0),
                    TextColor(if *object == leader { Color::srgb(1.0, 0.35, 0.3) } else { Color::srgb(1.0, 0.85, 0.3) }),
                    TextShadow::default(),
                    Visibility::Hidden,
                    GlobalZIndex(31),
                    Bark { object: *object, left: 1.4, life: 1.4, rise: true },
                ));
            }
            WorldEvent::AttackMissed { target, .. } => {
                commands.spawn((
                    Node { position_type: PositionType::Absolute, ..default() },
                    Text::new("miss"),
                    TextFont::from_font_size(15.0),
                    TextColor(Color::srgb(0.7, 0.7, 0.7)),
                    TextShadow::default(),
                    Visibility::Hidden,
                    GlobalZIndex(31),
                    Bark { object: *target, left: 1.0, life: 1.0, rise: true },
                ));
            }
            WorldEvent::AttackStarted { attacker, target, .. } if *attacker == leader => last.0 = Some(*target),
            WorldEvent::PlaySound { sound, .. } if !sound.is_empty() => {
                voice.play_sound(sound);
            }
            _ => {}
        }
    }
}

fn is_shout_token(text: &str) -> bool {
    text.contains('_') && !text.chars().any(|c| c.is_ascii_lowercase() || c == ' ')
}

fn update_fade(time: Res<Time<Real>>, mut fade: ResMut<Fade>, mut q: Query<&mut BackgroundColor, With<FadeOverlay>>) {
    let dt = time.delta_secs();
    if fade.wait > 0.0 {
        fade.wait -= dt;
    } else if fade.alpha != fade.target {
        let step = fade.rate * dt;
        fade.alpha = if fade.alpha < fade.target { (fade.alpha + step).min(fade.target) } else { (fade.alpha - step).max(fade.target) };
    }
    let [r, g, b] = fade.color;
    for mut bg in &mut q {
        bg.0 = Color::srgba(r, g, b, fade.alpha);
    }
}

fn update_card(cards: Res<MovieCards>, mut card: Query<(&mut Visibility, &Children), With<MovieCard>>, mut texts: Query<&mut Text>) {
    let Ok((mut vis, children)) = card.single_mut() else { return };
    match cards.current() {
        Some(name) => {
            *vis = Visibility::Visible;
            for c in children.iter() {
                if let Ok(mut t) = texts.get_mut(c) {
                    let s = format!("[movie {name}]");
                    if t.0 != s {
                        t.0 = s;
                    }
                }
            }
        }
        None => *vis = Visibility::Hidden,
    }
}

fn update_barks(
    mut commands: Commands,
    time: Res<Time>,
    ents: Res<ObjEntities>,
    objs: Query<&GlobalTransform>,
    cams: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut barks: Query<(Entity, &mut Bark, &mut Node, &mut Visibility, &ComputedNode)>,
) {
    let Ok((cam, cam_gt)) = cams.single() else { return };
    for (e, mut b, mut node, mut vis, computed) in &mut barks {
        b.left -= time.delta_secs();
        if b.left <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        let Some(screen) = ents
            .0
            .get(&b.object)
            .and_then(|o| objs.get(*o).ok())
            .and_then(|g| cam.world_to_viewport(cam_gt, g.translation() + Vec3::Y * 2.2).ok())
        else {
            *vis = Visibility::Hidden;
            continue;
        };
        let size = computed.size() * computed.inverse_scale_factor();
        let lift = if b.rise { (b.life - b.left) * 40.0 } else { 0.0 };
        node.left = Val::Px(screen.x - size.x * 0.5);
        node.top = Val::Px(screen.y - size.y - lift);
        *vis = Visibility::Visible;
    }
}

/// Drops the attack target once it is dead or gone.
fn forget_dead_target(world: Res<WorldRes>, mut last: ResMut<LastTarget>) {
    let w = &world.0;
    if last.0.is_some_and(|id| w.get(id).and_then(|o| o.creature()).is_none_or(|c| c.dead)) {
        last.0 = None;
    }
}

fn update_hud(
    keys: Res<ButtonInput<KeyCode>>,
    mut fps: ResMut<ShowFps>,
    diag: Res<DiagnosticsStore>,
    world: Res<WorldRes>,
    mut q: Query<&mut Text, With<HudLine>>,
) {
    if keys.just_pressed(KeyCode::F3) {
        fps.0 = !fps.0;
    }
    let w = &world.0;
    let area = w.get(w.area_id).map(|a| a.name.clone()).unwrap_or_default();
    let module = w.module.as_ref().map(|m| m.name.to_ascii_uppercase()).unwrap_or_default();
    let mut s = String::new();
    if fps.0 {
        s = format!("{area}  ({module})");
        let f = diag.get(&FrameTimeDiagnosticsPlugin::FPS).and_then(|d| d.smoothed()).unwrap_or(0.0);
        let leader = w.get(w.party_leader()).map(|o| format!("{} ({:.1}, {:.1}, {:.1})", o.tag, o.position.x, o.position.y, o.position.z));
        s.push_str(&format!("\n{f:.0} fps  {}", leader.unwrap_or_default()));
    }
    for mut t in &mut q {
        if t.0 != s {
            t.0 = s.clone();
        }
    }
}
