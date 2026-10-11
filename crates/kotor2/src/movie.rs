//! Bink movies (steamassets/movies/*.bik), decoded by an `ffmpeg`
//! subprocess.
//!
//! Video: `ffmpeg -f rawvideo -pix_fmt rgba` to a pipe, read on a thread
//! one frame at a time into a bounded channel (ffmpeg stalls when we are
//! ahead), and shown on a fullscreen UI image paced by the stream's frame
//! rate. Audio: the track is decoded to a temp WAV first, and video and
//! audio start together once it is ready.
//!
//! The queue is `game::MovieCards`: script `PlayMovie`/`QueueMovie` events
//! and transition movies land there, and the world does not tick while it
//! is non-empty. With `ffmpeg` missing the old one-second name card is used.
//! Esc, Space, Enter or a click skips. On launch to the main menu the logo
//! movies play first unless `--no-logos`. `--movie NAME` queues one movie at
//! startup and `--skip-movies` drops every movie unplayed (both for tests).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};

use bevy::asset::RenderAssetUsages;
use bevy::audio::{AudioSink, PlaybackMode, SpatialAudioSink};
use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui::FocusPolicy;

use crate::Args;
use crate::game::MovieCards;
use crate::model::GameRes;

/// Switches owned by this module (no value).
pub const SWITCHES: [&str; 2] = ["--no-logos", "--skip-movies"];
/// Flags owned by this module, each taking one value.
pub const FLAGS: [&str; 1] = ["--movie"];

/// Logo movies on launch, in the order shown (the Aspyr build's exact order
/// is not verified).
const LOGOS: [&str; 4] = ["legal", "leclogo", "obsidianent", "aspyr"];

pub struct MoviePlugin;

impl Plugin for MoviePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(MovieSys { ffmpeg: ffmpeg_available(), warned: false, playing: None })
            .init_resource::<Hold>()
            .add_systems(Startup, queue_startup_movies)
            .add_systems(PreUpdate, skip_input.after(InputSystems))
            .add_systems(Update, (start_movie, update_movie, hold_game, report_movie_audio).chain());
    }
}

#[derive(Resource)]
struct MovieSys {
    ffmpeg: bool,
    warned: bool,
    playing: Option<Playback>,
}

struct Playback {
    name: String,
    child: Child,
    frames: Mutex<Receiver<Vec<u8>>>,
    audio: Mutex<Receiver<Option<Vec<u8>>>>,
    audio_done: bool,
    fps: f64,
    duration: f64,
    /// Real time (s) when video and audio started together.
    started: Option<f64>,
    asked: f64,
    shown: u64,
    eof: bool,
    image: Handle<Image>,
    ui: Entity,
    sound: Option<Entity>,
    skip: bool,
    updates: u64,
}

/// What the game had running when a movie took over, to restore after.
/// While a movie is up: virtual time is paused (animation, emitters,
/// cutscene cameras), plain sinks (music, ambience, voice, UI sfx) are
/// paused and spatial sinks (sound objects, positional sfx) are muted.
/// The world tick and dialog timers check `MovieCards` themselves.
#[derive(Resource, Default)]
struct Hold {
    active: bool,
    paused_time: bool,
    plain: Vec<Entity>,
    spatial: Vec<Entity>,
}

/// True while a movie owns the screen (or is queued to).
pub fn movie_up(cards: &MovieCards) -> bool {
    cards.real && cards.current().is_some()
}

/// What the screen is doing, for the control API's info verb. A movie covers
/// everything; then loading a module or save, the main menu, a conversation, a
/// minigame, the pause menu, and the game itself.
pub fn phase(in_game: bool, movie: bool, loading: bool, dialog: bool, minigame: bool, paused: bool) -> &'static str {
    if movie {
        "movie"
    } else if loading {
        "loading"
    } else if !in_game {
        "menu"
    } else if dialog {
        "dialog"
    } else if minigame {
        "minigame"
    } else if paused {
        "paused"
    } else {
        "game"
    }
}

fn hold_game(
    sys: Res<MovieSys>,
    cards: Res<MovieCards>,
    mut hold: ResMut<Hold>,
    mut time: ResMut<Time<Virtual>>,
    mut plain: Query<(Entity, &mut AudioSink), Without<MovieSound>>,
    mut spatial: Query<(Entity, &mut SpatialAudioSink)>,
) {
    let up = sys.playing.is_some() || movie_up(&cards);
    if up {
        if !hold.active {
            hold.active = true;
            let playing = plain.iter().filter(|(_, s)| !s.is_paused()).count();
            let audible = spatial.iter().filter(|(_, s)| !s.is_muted() && !s.is_paused()).count();
            info!("movie: holding game audio ({playing} plain playing, {audible} spatial live), time paused before={}", time.is_paused());
            if !time.is_paused() {
                time.pause();
                hold.paused_time = true;
            }
        }
        // Every frame, so sinks that start mid-movie are caught too.
        for (e, s) in &mut plain {
            if !s.is_paused() {
                s.pause();
                hold.plain.push(e);
            }
        }
        for (e, mut s) in &mut spatial {
            if !s.is_muted() {
                s.mute();
                hold.spatial.push(e);
            }
        }
        return;
    }
    if !hold.active {
        return;
    }
    let mut h = std::mem::take(&mut *hold);
    let (mut resumed, mut unmuted) = (0, 0);
    for e in h.plain.drain(..) {
        if let Ok((_, s)) = plain.get_mut(e) {
            s.play();
            resumed += 1;
        }
    }
    for e in h.spatial.drain(..) {
        if let Ok((_, mut s)) = spatial.get_mut(e) {
            s.unmute();
            unmuted += 1;
        }
    }
    if h.paused_time {
        time.unpause();
    }
    info!("movie: released game audio ({resumed} plain resumed, {unmuted} spatial unmuted, time resumed={})", h.paused_time);
}

/// Marks the movie's own audio so it is not paused with the rest.
#[derive(Component)]
struct MovieSound;

/// Logs the movie audio sink's live state 2 s in (proof it plays).
#[allow(clippy::too_many_arguments)]
fn report_movie_audio(
    time: Res<Time<Real>>,
    sys: Res<MovieSys>,
    hold: Res<Hold>,
    vtime: Res<Time<Virtual>>,
    sinks: Query<&AudioSink, With<MovieSound>>,
    others: Query<&AudioSink, Without<MovieSound>>,
    spatial: Query<&SpatialAudioSink>,
    mut done: Local<Option<String>>,
) {
    let Some(p) = &sys.playing else { return };
    let Some(start) = p.started else { return };
    if done.as_deref() == Some(p.name.as_str()) || time.elapsed_secs_f64() - start < 2.0 {
        return;
    }
    *done = Some(p.name.clone());
    info!(
        "movie: during {}: {} other plain sinks playing, {} spatial audible, held {} plain + {} spatial, virtual time paused={}",
        p.name,
        others.iter().filter(|s| !s.is_paused()).count(),
        spatial.iter().filter(|s| !s.is_muted() && !s.is_paused()).count(),
        hold.plain.len(),
        hold.spatial.len(),
        vtime.is_paused()
    );
    match (p.sound, sinks.iter().next()) {
        (None, _) => info!("movie: {} has no audio track", p.name),
        (Some(_), None) => warn!("movie: {} audio entity has no sink 2 s in", p.name),
        (Some(_), Some(s)) => info!(
            "movie: {} audio sink paused={} muted={} volume={:.4} position={:.2}s empty={}",
            p.name,
            s.is_paused(),
            s.is_muted(),
            s.volume().to_linear(),
            s.position().as_secs_f32(),
            s.empty()
        ),
    }
}

fn ffmpeg_available() -> bool {
    let ok = |bin: &str| Command::new(bin).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    ok("ffmpeg") && ok("ffprobe")
}

fn has_switch(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

fn arg_value(name: &str) -> Option<String> {
    let argv: Vec<String> = std::env::args().collect();
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned()
}

fn queue_startup_movies(args: Res<Args>, mut cards: ResMut<MovieCards>, sys: Res<MovieSys>) {
    cards.real = sys.ffmpeg;
    if !sys.ffmpeg {
        warn!("movie: ffmpeg/ffprobe not found; Bink movies are skipped with a name card");
    }
    let to_menu = args.module.is_none() && !args.newgame && args.screenshot.is_none() && !crate::ctl::wants_continue();
    if to_menu && !has_switch("--no-logos") {
        cards.queue.extend(LOGOS.iter().map(|s| (*s).to_owned()));
    }
    if let Some(m) = arg_value("--movie") {
        cards.queue.push(m.to_ascii_lowercase());
    }
}

/// The .bik for a movie name, case-insensitively, under the install's
/// movies/ dir.
fn movie_path(res: &GameRes, name: &str) -> Option<PathBuf> {
    let dir = res.0.install.dir("movies")?;
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case(name))
            && p.extension().and_then(|s| s.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("bik"))
    })
}

struct Probe {
    width: u32,
    height: u32,
    fps: f64,
    duration: f64,
    has_audio: bool,
}

fn probe(path: &Path) -> Option<Probe> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type,width,height,r_frame_rate:format=duration", "-of", "default=nw=1"])
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (mut width, mut height, mut fps, mut duration, mut has_audio) = (0, 0, 0.0, 0.0, false);
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "codec_type" if v == "audio" => has_audio = true,
            "width" if width == 0 => width = v.parse().unwrap_or(0),
            "height" if height == 0 => height = v.parse().unwrap_or(0),
            "r_frame_rate" if fps == 0.0 => {
                fps = match v.split_once('/') {
                    Some((n, d)) => n.parse::<f64>().unwrap_or(0.0) / d.parse::<f64>().unwrap_or(1.0).max(1e-9),
                    None => v.parse().unwrap_or(0.0),
                }
            }
            "duration" => duration = v.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    (width > 0 && height > 0).then_some(Probe { width, height, fps: if fps > 0.0 { fps } else { 30.0 }, duration, has_audio })
}

#[allow(clippy::too_many_arguments)]
fn start_movie(
    mut commands: Commands,
    time: Res<Time<Real>>,
    res: Res<GameRes>,
    mut sys: ResMut<MovieSys>,
    mut cards: ResMut<MovieCards>,
    mut images: ResMut<Assets<Image>>,
    windows: Query<&Window>,
) {
    if sys.playing.is_some() || !sys.ffmpeg {
        if !sys.ffmpeg && !sys.warned {
            sys.warned = true;
        }
        return;
    }
    let Some(name) = cards.current().map(str::to_owned) else { return };
    if has_switch("--skip-movies") {
        info!("movie: {name} skipped (--skip-movies)");
        cards.queue.remove(0);
        return;
    }
    let Some(path) = movie_path(&res, &name) else {
        warn!("movie: {name}.bik not found, skipped");
        cards.queue.remove(0);
        return;
    };
    let Some(p) = probe(&path) else {
        warn!("movie: ffprobe could not read {}", path.display());
        cards.queue.remove(0);
        return;
    };
    let frame_len = p.width as usize * p.height as usize * 4;
    let child = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(&path)
        .args(["-an", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            warn!("movie: ffmpeg failed to start for {name}: {e}");
            cards.queue.remove(0);
            return;
        }
    };
    let mut stdout = child.stdout.take().expect("piped stdout");
    let (ftx, frx) = sync_channel::<Vec<u8>>(3);
    std::thread::spawn(move || {
        loop {
            let mut buf = vec![0u8; frame_len];
            if stdout.read_exact(&mut buf).is_err() || ftx.send(buf).is_err() {
                break;
            }
        }
    });
    let (atx, arx) = sync_channel::<Option<Vec<u8>>>(1);
    if p.has_audio {
        let path = path.clone();
        let wav = std::env::temp_dir().join(format!("kotor2-movie-{}.wav", std::process::id()));
        std::thread::spawn(move || {
            let ok = Command::new("ffmpeg")
                .args(["-v", "error", "-nostdin", "-y", "-i"])
                .arg(&path)
                .args(["-vn", "-ac", "2", "-ar", "44100", "-acodec", "pcm_s16le"])
                .arg(&wav)
                .stdin(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            let _ = atx.send(if ok { std::fs::read(&wav).ok() } else { None });
        });
    } else {
        let _ = atx.send(None);
    }

    let image = images.add(Image::new_fill(
        Extent3d { width: p.width, height: p.height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    ));
    // Fit the movie inside the window, letterboxed.
    let win_aspect = windows.iter().next().map_or(16.0 / 9.0, |w| w.width() / w.height().max(1.0));
    let aspect = p.width as f32 / p.height as f32;
    let fit = if aspect >= win_aspect {
        Node { width: Val::Percent(100.0), aspect_ratio: Some(aspect), ..default() }
    } else {
        Node { height: Val::Percent(100.0), aspect_ratio: Some(aspect), ..default() }
    };
    let ui = commands
        .spawn((
            Name::new(format!("movie {name}")),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::BLACK),
            GlobalZIndex(1000),
            FocusPolicy::Block,
            Interaction::default(),
        ))
        .with_children(|c| {
            c.spawn((fit, ImageNode::new(image.clone())));
        })
        .id();
    info!(
        "movie: {name} {}x{} {:.2} fps {:.1}s audio={} ({})",
        p.width,
        p.height,
        p.fps,
        p.duration,
        p.has_audio,
        path.display()
    );
    sys.playing = Some(Playback {
        name,
        child,
        frames: Mutex::new(frx),
        audio: Mutex::new(arx),
        audio_done: false,
        fps: p.fps,
        duration: p.duration,
        started: None,
        asked: time.elapsed_secs_f64(),
        shown: 0,
        eof: false,
        image,
        ui,
        sound: None,
        skip: false,
        updates: 0,
    });
}

/// Skip keys; while a movie is up, the keys it uses are swallowed so the
/// game (pause menu, dialog skip) does not also react.
fn skip_input(mut sys: ResMut<MovieSys>, mut keys: ResMut<ButtonInput<KeyCode>>, mut mouse: ResMut<ButtonInput<MouseButton>>) {
    let Some(p) = &mut sys.playing else { return };
    let mut skip = false;
    for k in [KeyCode::Escape, KeyCode::Space, KeyCode::Enter] {
        if keys.just_pressed(k) {
            skip = true;
        }
        keys.clear_just_pressed(k);
    }
    if mouse.just_pressed(MouseButton::Left) {
        skip = true;
        mouse.clear_just_pressed(MouseButton::Left);
    }
    // Ignore input in the first instant so a held key does not skip a
    // chain of movies at once.
    if skip && p.started.is_some() {
        p.skip = true;
    }
}

#[allow(clippy::too_many_arguments)]
fn update_movie(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut sys: ResMut<MovieSys>,
    mut cards: ResMut<MovieCards>,
    mut images: ResMut<Assets<Image>>,
    mut audio_assets: ResMut<Assets<bevy::audio::AudioSource>>,
) {
    let now = time.elapsed_secs_f64();
    let Some(p) = &mut sys.playing else { return };

    if p.started.is_none() {
        // Wait for the audio decode (up to 4 s), then start both clocks.
        let audio = match p.audio.get_mut().map(|rx| rx.try_recv()) {
            Ok(Ok(a)) => Some(a),
            Ok(Err(TryRecvError::Empty)) if now - p.asked < 4.0 => return,
            _ => Some(None),
        };
        p.audio_done = true;
        if let Some(Some(bytes)) = audio {
            let h = audio_assets.add(bevy::audio::AudioSource { bytes: bytes.into() });
            let e = commands
                .spawn((
                    Name::new(format!("movie audio {}", p.name)),
                    MovieSound,
                    AudioPlayer::new(h),
                    PlaybackSettings { mode: PlaybackMode::Despawn, ..PlaybackSettings::ONCE },
                ))
                .id();
            p.sound = Some(e);
        }
        p.started = Some(now);
        info!("movie: {} started after {:.2}s prep", p.name, now - p.asked);
    }

    let elapsed = now - p.started.unwrap_or(now);
    let target = (elapsed * p.fps) as u64;
    p.updates += 1;
    let prev = elapsed - time.delta_secs_f64();
    if (elapsed / 5.0).floor() > (prev / 5.0).floor() {
        info!("movie: {} t {:.1}s shown {} target {} ({:.1} updates/s)", p.name, elapsed, p.shown, target, p.updates as f64 / elapsed.max(0.001));
    }
    let mut latest = None;
    if let Ok(rx) = p.frames.get_mut() {
        while p.shown <= target {
            match rx.try_recv() {
                Ok(f) => {
                    latest = Some(f);
                    p.shown += 1;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    p.eof = true;
                    break;
                }
            }
        }
    }
    if let Some(f) = latest
        && let Some(mut img) = images.get_mut(&p.image)
    {
        img.data = Some(f);
    }
    // Done when the decoder ran dry and the last frame has had its time,
    // or well past the probed duration (a stuck decoder).
    let shown_time = p.shown as f64 / p.fps;
    let finished = (p.eof && elapsed >= shown_time) || (p.duration > 0.0 && elapsed > p.duration + 2.0);
    if !(finished || p.skip) {
        return;
    }
    let mut p = sys.playing.take().expect("playing");
    info!(
        "movie: {} {} at {:.1}s ({} frames, {:.1} fps shown, {:.1} updates/s)",
        p.name,
        if p.skip { "skipped" } else { "ended" },
        elapsed,
        p.shown,
        p.shown as f64 / elapsed.max(0.001),
        p.updates as f64 / elapsed.max(0.001)
    );
    let _ = p.child.kill();
    let _ = p.child.wait();
    commands.entity(p.ui).despawn();
    if let Some(e) = p.sound {
        commands.entity(e).despawn();
    }
    images.remove(&p.image);
    if cards.current() == Some(p.name.as_str()) {
        cards.queue.remove(0);
    }
    cards.shown = 0.0;
}
