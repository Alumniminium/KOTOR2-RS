//! Barks: one-liner conversations shown in the bark bubble (barkbubble_p.gui).
//!
//! k2-world turns a conversation whose starting line has no reply links into
//! `WorldEvent::BarkLine` and runs its scripts without a conversation. Here
//! the line goes into the bubble layout, is voiced when it has audio, and
//! stays up for the voice-over or a reading time. No camera, no pause: the
//! world keeps ticking. A new bark replaces the bubble, and it is hidden while
//! a conversation or a movie holds the screen.

use bevy::prelude::*;
use k2_world::WorldEvent;

use crate::audio::Voice;
use crate::dialog_ui::read_time;
use crate::game::{AppState, FrameEvents, GameSet, InConversation, MovieCards, Paused};
use crate::gui::{GuiScreen, Placement, open_screen};
use crate::model::GameRes;

/// Layout name; the `_p` (PC) file is picked by the layout resolver.
const BUBBLE: &str = "barkbubble";
/// The bubble's text label.
const LINE: &str = "LBL_BARKTEXT";
/// Above the HUD (40), below the movie card and fades.
const Z: i32 = 45;
/// Seconds added after a voice-over, as for dialog lines.
const VO_PAD: f32 = 0.35;

pub struct BarkPlugin;

impl Plugin for BarkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Bubble>()
            .add_systems(Update, (show_lines, tick_bubble).chain().in_set(GameSet::Ui))
            .add_systems(OnExit(AppState::Game), hide_bubble);
    }
}

/// The bubble on screen and the seconds it has left.
#[derive(Resource, Default)]
pub(crate) struct Bubble {
    screen: Option<Entity>,
    left: f32,
    /// The bark's measured length when it went up (its voice-over plus the pad, or its text time).
    total: f32,
}

impl Bubble {
    /// Seconds left in the bark on screen (a one-liner conversation too), None when no bubble is up.
    pub(crate) fn speaking_left(&self) -> Option<f32> {
        (self.screen.is_some() && self.left > 0.0).then_some(self.left)
    }

    /// The bark's measured length, while it is on screen.
    pub(crate) fn speaking_total(&self) -> Option<f32> {
        self.speaking_left().map(|_| self.total)
    }
}

#[derive(Component)]
struct BubbleScreen;

fn show_lines(
    mut commands: Commands,
    events: Res<FrameEvents>,
    game: Res<GameRes>,
    in_conversation: Res<InConversation>,
    mut bubble: ResMut<Bubble>,
    mut voice: Voice,
) {
    // A conversation owns the voice and the screen: a bark that lands during
    // one is dropped, and tick_bubble takes down any bubble still up.
    if in_conversation.0 {
        return;
    }
    // Only the newest bark of the frame plays. Each play_voice stops the line
    // before it, but that stop only sees voices spawned in earlier frames
    // (commands apply at the end of the frame), so two in one frame overlap.
    let Some((text, vo, sound, alien_race, emotion, delay, pos)) = events.0.iter().rev().find_map(|e| match e {
        WorldEvent::BarkLine { text, vo, sound, alien_race, emotion, delay, pos, .. } if !text.trim().is_empty() => {
            Some((text, vo, sound, alien_race, emotion, delay, *pos))
        }
        _ => None,
    }) else {
        return;
    };
    let audio = vo
        .as_deref()
        .and_then(|r| voice.play_voice(r))
        .or_else(|| sound.as_deref().and_then(|r| voice.play_voice(r)))
        .or_else(|| (*alien_race != 0).then(|| voice.play_alien(*alien_race, *emotion)).flatten());
    bubble.left = match audio {
        // A Delay longer than the voice-over holds the bubble, as for lines.
        Some(d) => (d.as_secs_f32() + VO_PAD).max(delay.filter(|&d| d > 0 && d < 60).map_or(0.0, |d| d as f32)),
        None => read_time(text, *delay),
    };
    bubble.total = bubble.left;
    if let Some(old) = bubble.screen.take() {
        commands.entity(old).despawn();
    }
    // Centred, not anchored: the root panel (the bubble's border) is drawn
    // only for a centred layout.
    let Some(screen) = open_screen(&mut commands, &game.0, BUBBLE, Placement::Center, Z) else {
        error!("bark: no layout {BUBBLE}");
        return;
    };
    commands.entity(screen).insert(BubbleScreen);
    let line = text.clone();
    let origin = pos.map(|(x, y)| [x as f32, y as f32]);
    commands.queue(move |world: &mut bevy::prelude::World| {
        if let Some(mut scr) = world.get_mut::<GuiScreen>(screen) {
            scr.set_origin(origin);
            scr.set_text(LINE, line);
        }
    });
    bubble.screen = Some(screen);
    info!("bark: {text} ({:.1}s)", bubble.left);
}

/// Counts the bubble down in real time (frozen by the pause menu and movies)
/// and takes it down at once when a conversation starts.
fn tick_bubble(
    mut commands: Commands,
    time: Res<Time<Real>>,
    paused: Res<Paused>,
    in_conversation: Res<InConversation>,
    cards: Res<MovieCards>,
    mut bubble: ResMut<Bubble>,
) {
    let Some(screen) = bubble.screen else { return };
    if in_conversation.0 {
        bubble.left = 0.0;
    } else if !paused.0 && !crate::movie::movie_up(&cards) {
        bubble.left -= time.delta_secs();
    }
    if bubble.left <= 0.0 {
        commands.entity(screen).despawn();
        bubble.screen = None;
    }
}

fn hide_bubble(mut commands: Commands, mut bubble: ResMut<Bubble>, screens: Query<Entity, With<BubbleScreen>>) {
    for e in &screens {
        commands.entity(e).despawn();
    }
    *bubble = Bubble::default();
}
