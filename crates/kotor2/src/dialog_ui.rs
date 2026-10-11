//! Conversation UI: the NPC line (top letterbox bar), numbered replies (bottom bar), voice
//! playback with auto-advance, and an over-the-shoulder camera.
//!
//! Lines whose only reply is "[Continue]" (or that have none) advance by
//! themselves after the voice-over (or a text-length timer when the line
//! has no audio); Space/Enter skips. Real replies wait for 1-9 or a click.
//!
//! A door's own line with no real reply is a message box instead (confirm_p.gui
//! with OK): no letterbox, no camera move, and it waits for OK, Enter or Space.
//! Escape ends the talk as aborted.

use bevy::prelude::*;
use k2_script::ObjectId;
use k2_world::ConversationView;

use crate::Args;
use crate::audio::Voice;
use crate::coords;
use crate::gui::{BitmapText, GuiClick, GuiScreen, Placement, open_screen};
use crate::model::GameRes;
use crate::uisfx::UiSfx;
use crate::game::{AutoReplyTimer, Fade, GameSet, ObjEntities, Paused, WorldRes, autoreply_tick};

/// The system that starts a new line's voice and sets its lip length
/// (`AlienLine`); systems that read the line's audio run after it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LineShown;

pub struct DialogUiPlugin;

impl Plugin for DialogUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LineState>()
            .init_resource::<AutoReplyTimer>()
            .add_systems(Startup, spawn_ui)
            .add_systems(Update, (dialog_input, show_dialog.in_set(LineShown), message_box, dialog_camera).chain().in_set(GameSet::Ui));
    }
}

/// The line on screen and when it may advance.
#[derive(Resource, Default)]
pub(crate) struct LineState {
    key: Option<(String, usize)>,
    shown: f32,
    wait: f32,
    choices: bool,
    talking: bool,
    /// The last line faded the screen out (FadeType 4); fade back in when
    /// the conversation ends.
    faded_out: bool,
    /// The line is a message box: it waits for OK, not for its text time, so it is not speech.
    boxed: bool,
    /// Counts the lines shown, so a new line is noticed once.
    serial: u64,
}

impl LineState {
    /// Seconds left in the NPC line on screen, while it is still being spoken. Its
    /// length (`wait`) is measured when the line starts: the decoded voice-over
    /// plus a pad, or the text time when it has no voice-over. None once it is over,
    /// and when no line is on screen.
    pub(crate) fn speaking_left(&self) -> Option<f32> {
        if self.key.is_none() || self.boxed {
            return None;
        }
        crate::remote::speech::left_of(self.wait, self.shown)
    }

    /// The line's measured length, while it is spoken (see `speaking_left`).
    pub(crate) fn speaking_total(&self) -> Option<f32> {
        self.speaking_left().map(|_| self.wait)
    }

    /// How many lines have been shown; a new line changes it.
    pub(crate) fn serial(&self) -> u64 {
        self.serial
    }
}

#[derive(Component)]
struct DialogRoot;
#[derive(Component)]
struct Letterbox;
#[derive(Component)]
struct LineText;
#[derive(Component)]
struct ReplyList;
#[derive(Component)]
struct ReplyButton(usize);
/// The door message box (confirm_p.gui): the line in LB_MESSAGE, OK to close it.
/// `text` is the line the box shows now, so the layout is set once per line.
#[derive(Component)]
struct MessageBox {
    text: String,
}

// From dialog_p.gui: LBL_MESSAGE and the LB_REPLIES proto item (font,
// colours, 544 px column). The NPC line sits in the top letterbox bar, the
// numbered replies in the bottom one, as in the game.
const DIALOG_FONT: &str = "fnt_d16x16";
const DIALOG_WIDTH: f32 = 544.0;
const LINE_COLOR: Color = Color::srgb(0.97, 1.0, 0.97);
const REPLY_COLOR: Color = Color::srgb(0.10, 0.70, 0.55);
const REPLY_HOVER: Color = Color::srgb(1.0, 0.92, 0.55);
/// Letterbox bar height: the dialog panel's 100 px of a 600 px layout.
const BAR: f32 = 100.0 / 6.0;
/// Seconds a node FadeType fade takes (the DLG has no duration; tuned).
const NODE_FADE: f32 = 0.8;

fn spawn_ui(mut commands: Commands) {
    for top in [true, false] {
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: if top { Val::Px(0.0) } else { Val::Auto },
                bottom: if top { Val::Auto } else { Val::Px(0.0) },
                height: Val::Percent(BAR),
                ..default()
            },
            BackgroundColor(Color::BLACK),
            Visibility::Hidden,
            GlobalZIndex(10),
            Letterbox,
        ));
    }
    // Centred columns: the line in the top bar, replies in the bottom bar.
    let column = |top: bool| Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        right: Val::Px(0.0),
        top: if top { Val::Percent(1.5) } else { Val::Percent(100.0 - BAR + 1.5) },
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        ..default()
    };
    commands.spawn((column(true), Visibility::Hidden, GlobalZIndex(20), DialogRoot)).with_child((
        Node::default(),
        BitmapText::new("", DIALOG_FONT, LINE_COLOR).wrap(DIALOG_WIDTH).align(2),
        LineText,
    ));
    commands.spawn((column(false), Visibility::Hidden, GlobalZIndex(20), DialogRoot)).with_child((
        Node { flex_direction: FlexDirection::Column, align_items: AlignItems::FlexStart, width: Val::Auto, ..default() },
        ReplyList,
    ));
}

fn advances_alone(v: &ConversationView) -> bool {
    v.replies.is_empty() || (v.replies.len() == 1 && v.replies[0].text.trim().is_empty())
}

/// Seconds a line without audio stays up (also the bark bubble, bark.rs).
pub(crate) fn read_time(text: &str, delay: Option<u32>) -> f32 {
    // Delay is in seconds in K2 DLGs; 0xFFFFFFFF (u32::MAX) means default.
    if let Some(d) = delay.filter(|&d| d > 0 && d < 60) {
        return d as f32;
    }
    if text.trim().is_empty() { 1.0 } else { (1.2 + text.len() as f32 * 0.055).clamp(2.0, 12.0) }
}

/// Advance / reply input, plus `--autoreply`.
#[allow(clippy::too_many_arguments)]
fn dialog_input(
    time: Res<Time<Real>>,
    args: Res<Args>,
    keys: Res<ButtonInput<KeyCode>>,
    paused: Res<Paused>,
    cards: Res<crate::game::MovieCards>,
    mut line: ResMut<LineState>,
    mut world: ResMut<WorldRes>,
    mut voice: Voice,
    mut auto: ResMut<AutoReplyTimer>,
    buttons: Query<(&Interaction, &ReplyButton), Changed<Interaction>>,
    mut clicks: MessageReader<GuiClick>,
    mut sfx: MessageWriter<UiSfx>,
) {
    // A movie holds the conversation (scripts PlayMovie mid-dialog).
    if paused.0 || crate::movie::movie_up(&cards) {
        return;
    }
    let box_ok = clicks.read().any(|c| c.screen.starts_with("confirm") && c.tag == "BTN_OK");
    let w = &mut world.0;
    let Some(v) = w.conversation_view() else { return };
    let dt = time.delta_secs();
    if autoreply_tick(dt, &args, w, &mut auto.0) {
        sfx.write(UiSfx("Clicked_Default"));
        voice.stop_voice();
        line.key = None;
        return;
    }
    if v.boxed {
        // A door's message box: OK (or Enter, Space) takes the next node, as the
        // conversation would; Escape ends the conversation as aborted.
        if keys.just_pressed(KeyCode::Escape) {
            voice.stop_voice();
            line.key = None;
            w.end_conversation(true);
        } else if box_ok || keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
            voice.stop_voice();
            line.key = None;
            w.advance_conversation();
        }
        return;
    }
    if advances_alone(&v) {
        let skip = (keys.just_pressed(KeyCode::Space) || keys.just_pressed(KeyCode::Enter)) && !v.unskippable;
        let timed_out = line.shown >= line.wait && !v.paused && line.key.is_some();
        if skip || timed_out {
            if skip {
                voice.stop_voice();
            }
            line.key = None;
            w.advance_conversation();
        }
        return;
    }
    const DIGITS: [KeyCode; 9] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    let mut pick = DIGITS.iter().position(|k| keys.just_pressed(*k)).filter(|&i| i < v.replies.len());
    for (i, b) in &buttons {
        match *i {
            Interaction::Pressed => pick = Some(b.0),
            Interaction::Hovered => {
                sfx.write(UiSfx("Entered_Default"));
            }
            Interaction::None => {}
        }
    }
    if let Some(i) = pick {
        sfx.write(UiSfx("Clicked_Default"));
        voice.stop_voice();
        info!("reply {}: {}", i + 1, v.replies[i].text);
        line.key = None;
        w.choose_reply(i);
    }
}

type ReplyColors = (&'static Interaction, &'static mut BitmapText);

/// Mirrors the current line into the UI, starts its audio and runs the
/// advance timer.
#[allow(clippy::too_many_arguments)]
fn show_dialog(
    mut commands: Commands,
    time: Res<Time<Real>>,
    paused: Res<Paused>,
    pause: Res<crate::gui::menus::PauseState>,
    cards: Res<crate::game::MovieCards>,
    world: Res<WorldRes>,
    mut line: ResMut<LineState>,
    mut fade: ResMut<Fade>,
    mut voice: Voice,
    mut alien_line: ResMut<crate::audio::AlienLine>,
    mut root: Query<&mut Visibility, (With<DialogRoot>, Without<Letterbox>)>,
    mut bars: Query<&mut Visibility, (With<Letterbox>, Without<DialogRoot>)>,
    mut text: Query<&mut BitmapText, (With<LineText>, Without<ReplyButton>)>,
    list: Query<Entity, With<ReplyList>>,
    mut reply_colors: Query<ReplyColors, (With<ReplyButton>, Without<LineText>)>,
) {
    let w = &world.0;
    // The area editor shows the area, not the conversation the paused
    // world is still in (--edit enters mid-opening-conversation).
    if pause.editor {
        // Rebuilt (and shown again) from the current line on leaving.
        line.key = None;
        for mut v in bars.iter_mut().chain(root.iter_mut()) {
            *v = Visibility::Hidden;
        }
        return;
    }
    let view = w.conversation_view();
    let on = view.is_some();
    // A message box is not the conversation screen: no letterbox bars, no lines.
    let boxed = view.as_ref().is_some_and(|v| v.boxed);
    for mut v in &mut bars {
        *v = if on && !boxed { Visibility::Visible } else { Visibility::Hidden };
    }
    let Some(v) = view else {
        if line.talking {
            voice.stop_voice();
            line.talking = false;
        }
        line.key = None;
        if line.faded_out {
            line.faded_out = false;
            fade.to(0.0, 0.0, 0.6, [0.0; 3]);
        }
        for mut r in &mut root {
            *r = Visibility::Hidden;
        }
        return;
    };
    line.talking = true;
    if !paused.0 && !crate::movie::movie_up(&cards) {
        line.shown += time.delta_secs();
    }
    for (i, mut t) in &mut reply_colors {
        let c = if *i == Interaction::None { REPLY_COLOR } else { REPLY_HOVER };
        if t.color != c {
            t.color = c;
        }
    }
    let key = (v.dialog.clone(), v.entry);
    if line.key.as_ref() == Some(&key) {
        return;
    }
    // New line.
    line.key = Some(key);
    line.serial += 1;
    line.shown = 0.0;
    line.boxed = v.boxed;
    // Aliens without a voice file of their own speak the generic alien set;
    // its clip length also times the lip track (cutscene_cam).
    let file = v.vo.as_deref().and_then(|r| voice.play_voice(r)).or_else(|| v.sound.as_deref().and_then(|r| voice.play_voice(r)));
    let alien = file.is_none().then(|| (v.alien_race != 0).then(|| voice.play_alien(v.alien_race, v.emotion)).flatten()).flatten();
    alien_line.line = alien.map(|_| (v.dialog.clone(), v.entry));
    alien_line.seconds = alien.map_or(0.0, |d| d.as_secs_f32());
    let audio = file.or(alien);
    line.wait = match audio {
        // A node Delay longer than the voice-over holds the line (101awake's
        // get-up node waits out its 8 s clip).
        Some(d) => (d.as_secs_f32() + 0.35).max(v.delay.filter(|&d| d > 0 && d < 60).map_or(0.0, |d| d as f32)),
        None => read_time(&v.text, v.delay),
    };
    // FadeType 3: the line fades in from black; 4: it fades out to black at
    // its end.
    match v.fade_type {
        3 => {
            fade.alpha = 1.0;
            fade.to(0.0, 0.0, NODE_FADE, [0.0; 3]);
            line.faded_out = false;
        }
        4 => {
            fade.to(1.0, (line.wait - NODE_FADE).max(0.0), NODE_FADE, [0.0; 3]);
            line.faded_out = true;
        }
        _ => {}
    }
    let choices = !advances_alone(&v);
    line.choices = choices;
    let name = w.get(v.speaker).map(|o| w.substitute(&o.name)).unwrap_or_default();
    info!("dialog {}[{}] {}: {} ({} replies, vo {:?}, wait {:.1}s)", v.dialog, v.entry, name, v.text, v.replies.len(), v.vo, line.wait);
    for mut t in &mut text {
        t.text = v.text.clone();
    }
    let show = !boxed && (!v.text.trim().is_empty() || choices);
    for mut r in &mut root {
        *r = if show { Visibility::Visible } else { Visibility::Hidden };
    }
    let Ok(list) = list.single() else { return };
    commands.entity(list).despawn_related::<Children>();
    if choices {
        for (i, r) in v.replies.iter().enumerate() {
            let label = if r.text.trim().is_empty() { "[Continue]".to_owned() } else { r.text.clone() };
            commands.spawn((
                Button,
                Node::default(),
                BitmapText::new(format!("{}. {label}", i + 1), DIALOG_FONT, REPLY_COLOR).wrap(DIALOG_WIDTH).align(1),
                ReplyButton(i),
                ChildOf(list),
            ));
        }
    }
}

/// Opens the door message box while the current line is one (a door's own line
/// with no real reply, `ConversationView::boxed`), and closes it when it is not.
/// The box is confirm_p.gui with the cancel button hidden; OK is read in
/// `dialog_input`. It is not a menu: Escape is the dialog's (an abort).
fn message_box(mut commands: Commands, game: Res<GameRes>, world: Res<WorldRes>, mut boxes: Query<(Entity, &mut GuiScreen, &mut MessageBox)>) {
    let Some(v) = world.0.conversation_view().filter(|v| v.boxed) else {
        for (e, _, _) in &boxes {
            commands.entity(e).despawn();
        }
        return;
    };
    if let Some((_, mut screen, mut shown)) = boxes.iter_mut().next() {
        if shown.text != v.text {
            screen.set_text("LB_MESSAGE", v.text.clone());
            screen.set_visible("BTN_CANCEL", false);
            shown.text = v.text;
        }
    } else if let Some(e) = open_screen(&mut commands, &game.0, "confirm", Placement::Center, 40) {
        commands.entity(e).insert(MessageBox { text: String::new() });
    } else {
        warn!("dialog: confirm_p.gui is missing, so the door line {} has no box", v.text);
    }
}

/// Over-the-shoulder shot: behind the listener, framing the speaker.
/// Beyond this distance (m) between speaker and listener the default shot
/// frames the speaker alone.
pub const SOLO_SHOT_DIST: f32 = 8.0;

fn dialog_camera(
    time: Res<Time>,
    world: Res<WorldRes>,
    ents: Res<ObjEntities>,
    objs: Query<&GlobalTransform>,
    mut cams: Query<&mut Transform, With<Camera3d>>,
    mut last: Local<Option<(ObjectId, ObjectId)>>,
    mode: Res<crate::play::CamMode>,
) {
    if *mode == crate::play::CamMode::Fly {
        return;
    }
    let w = &world.0;
    let Some(v) = w.conversation_view() else {
        *last = None;
        return;
    };
    // A door's message box moves no camera.
    if v.boxed {
        return;
    }
    let pos = |id: ObjectId| -> Option<Vec3> {
        // Rendered position (Bevy space) when the object has an entity.
        ents.0.get(&id).and_then(|e| objs.get(*e).ok()).map(|g| coords::to_game(g.translation())).or_else(|| w.get(id).map(|o| o.position))
    };
    let leader = w.party_leader();
    let speaker = v.speaker;
    // The other party in the exchange: listener, or the PC when the PC is
    // not the one speaking.
    let other = if speaker == leader { if v.listener != leader { v.listener } else { v.owner } } else { leader };
    let (Some(s), Some(o)) = (pos(speaker), pos(other)) else { return };
    let flat = (s - o).truncate();
    let Ok(mut t) = cams.single_mut() else { return };
    // Talking to itself (placeable intros, barks), or far from the other
    // party: hologram logs put the recorded speaker in an out-of-bounds
    // holo room (101per MedOff at WP_MEDOFF, ~60 m from the console), and
    // the shot must frame the speaker alone, not look at it through walls.
    let (eye, look) = if flat.length() < 0.3 || flat.length() > SOLO_SHOT_DIST {
        let b = w.get(speaker).map_or(0.0, |o| o.bearing + std::f32::consts::FRAC_PI_2);
        let f = Vec3::new(b.cos(), b.sin(), 0.0);
        (s + f * 2.6 + Vec3::Z * 1.7, s + Vec3::Z * 1.4)
    } else {
        let d = flat.normalize().extend(0.0);
        let right = Vec3::new(d.y, -d.x, 0.0);
        (o - d * 1.1 + right * 0.55 + Vec3::Z * 1.75, s + Vec3::Z * 1.55)
    };
    let target = Transform::from_translation(coords::to_bevy(eye)).looking_at(coords::to_bevy(look), Vec3::Y);
    let cut = *last != Some((speaker, other));
    *last = Some((speaker, other));
    if cut {
        *t = target;
    } else {
        let k = 1.0 - (-6.0 * time.delta_secs()).exp();
        t.translation = t.translation.lerp(target.translation, k);
        t.rotation = t.rotation.slerp(target.rotation, k);
    }
}

#[cfg(test)]
mod tests;
