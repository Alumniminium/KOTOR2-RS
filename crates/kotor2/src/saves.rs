//! Save and load: quick save (F5) / quick load (F9), the game's own
//! `saveload` and `savename` screens (Save Game / Load Game in the options
//! menu, Load Game in the main menu), autosave on module entry.
//!
//! The world state comes from `k2_world::World::save`; slots live under
//! `$XDG_DATA_HOME/kotor2-rs/saves/<slot>/` (`K2_SAVE_DIR` overrides it)
//! with `save.bin`, `meta.json` and `thumb.png`. Loading builds a fresh
//! `World` from the slot and reloads the area like a transition does.
//!
//! Test hooks (environment): `K2_SAVE_TEST=quick` quick-saves 3 s after the
//! first area load, quick-loads 3 s later and logs the state digests;
//! `K2_SAVE_OPEN=load|save` opens that screen once the menu or game is up;
//! `K2_SAVE_LOAD=SLOT` loads a slot once (from the main menu too).
//! See docs/notes/saves.md.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use k2_world::save::{self, SaveData, SlotInfo};

use crate::game::{AppState, Fade, InConversation, MovieCards, NeedAreaLoad, Paused, PendingBarks, PendingUse, Transition, WorldRes};
use crate::gui::menus::{MenuScreen, MenusLogic, OpenMenu, PauseState};
use crate::gui::{GuiClick, GuiScreen, GuiSet, Placement, open_screen};
use crate::messages::AreaLoaded;
use crate::model::GameRes;
use crate::uisfx::UiSfx;

pub const QUICK_SLOT: &str = "quicksave";
pub const AUTO_SLOT: &str = "autosave";
/// Rolling autosave every `TIMED_SAVE_SECS` of play (own slot, so autosave,
/// quicksave and restart stay as they are).
pub const TIMED_SLOT: &str = "timed";
const TIMED_SAVE_SECS: f32 = 120.0;
/// Retry delay when saving is not allowed at the due time.
const TIMED_RETRY_SECS: f32 = 10.0;

// dialog.tlk strings the original screens use.
const STR_SAVE_GAME: u32 = 1588;
const STR_LOAD_GAME: u32 = 1585;
const STR_SAVE: u32 = 1587;
const STR_LOAD: u32 = 1589;
const STR_NEW_SLOT: u32 = 1590;
const STR_AUTO_SAVE: u32 = 1593;
const STR_QUICK_SAVE: u32 = 48752;

pub struct SavesPlugin;

impl Plugin for SavesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SaveRoot>()
            .init_resource::<SaveJob>()
            .init_resource::<PendingLoad>()
            .init_resource::<SaveUi>()
            .init_resource::<AutoSave>()
            .init_resource::<TimedSave>()
            .init_resource::<SaveTest>()
            .add_systems(
                Update,
                (open_from_menus, saveload_clicks, refresh_details, name_entry, env_hooks)
                    .chain()
                    .in_set(GuiSet::Logic)
                    .before(MenusLogic),
            )
            .add_systems(Update, (quick_keys, autosave, timed_autosave, save_test).run_if(in_state(AppState::Game)))
            .add_systems(Update, run_save_job)
            .add_systems(Last, apply_load);
    }
}

/// Where slots live.
#[derive(Resource)]
pub struct SaveRoot(pub PathBuf);

impl Default for SaveRoot {
    /// K2_SAVE_DIR, else the user's save folder. Scripted test runs
    /// (`--screenshot`) default to a temp folder so their autosaves never
    /// overwrite the player's.
    fn default() -> Self {
        let test_run = std::env::args().any(|a| a == "--screenshot");
        Self(std::env::var_os("K2_SAVE_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
            if test_run { std::env::temp_dir().join("kotor2-rs-test-saves") } else { save::default_save_root() }
        }))
    }
}

/// A save in progress: the state is written at once; the thumbnail is
/// taken a frame later with the UI hidden.
#[derive(Resource, Default)]
pub struct SaveJob {
    pub(crate) dir: Option<PathBuf>,
    frame: u32,
    hidden: Vec<Entity>,
    done: Option<Arc<AtomicBool>>,
}

/// A slot to load at the end of this frame.
#[derive(Resource, Default)]
pub struct PendingLoad(pub Option<String>);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Save,
    Load,
}

/// The open saveload screen (and the name prompt over it).
#[derive(Resource, Default)]
pub struct SaveUi {
    screen: Option<(Entity, Mode)>,
    slots: Vec<SlotInfo>,
    /// List row -> slot (None = the "New Slot" row).
    rows: Vec<Option<usize>>,
    shown: Option<Option<usize>>,
    thumb: Option<Entity>,
    naming: Option<Naming>,
}

struct Naming {
    screen: Entity,
    text: String,
    slot: String,
}

/// Marks the save screens so the main menu can close them itself.
#[derive(Component)]
struct SaveScreen;

#[derive(Resource, Default)]
struct AutoSave {
    /// Seconds since the area loaded; None = no autosave pending.
    due: Option<f32>,
    /// The next area load comes from a loaded save: no autosave for it.
    skip_next: bool,
}

#[derive(Resource, Default)]
pub(crate) struct SaveTest {
    t: f32,
    stage: u8,
    digest: u64,
    /// Digest of the last loaded world, right after `World::load`.
    pub(crate) loaded: Option<u64>,
}

fn talk(w: &k2_world::World, strref: u32, fallback: &str) -> String {
    w.talk.as_ref().and_then(|t| t.get(strref)).filter(|s| !s.is_empty()).unwrap_or(fallback).to_owned()
}

/// Writes the current world to `slot`. Returns the slot dir, or why not.
pub fn save_now(world: &k2_world::World, root: &SaveRoot, slot: &str, name: &str, job: &mut SaveJob) -> Result<PathBuf, String> {
    if let Some(why) = world.save_blocked() {
        return Err(why.to_owned());
    }
    let t0 = std::time::Instant::now();
    let data = world.save();
    let meta = world.save_meta(name);
    let dir = save::write_slot(&root.0, slot, &data, &meta).map_err(|e| e.to_string())?;
    info!("save: {slot} \"{name}\" {} bytes in {:?} -> {}", data.state.len(), t0.elapsed(), dir.display());
    job.dir = Some(dir.clone());
    job.frame = 0;
    Ok(dir)
}

fn feedback(world: &k2_world::World, barks: &mut PendingBarks, text: String) {
    info!("save: {text}");
    barks.0.push((world.party_leader(), text));
}

/// F5 quick save, F9 quick load.
fn quick_keys(
    keys: Res<ButtonInput<KeyCode>>,
    world: Res<WorldRes>,
    root: Res<SaveRoot>,
    mut job: ResMut<SaveJob>,
    mut load: ResMut<PendingLoad>,
    mut barks: ResMut<PendingBarks>,
    ui: Res<SaveUi>,
) {
    if ui.naming.is_some() {
        return;
    }
    if keys.just_pressed(KeyCode::F5) {
        let name = talk(&world.0, STR_QUICK_SAVE, "Quick Save");
        let msg = match save_now(&world.0, &root, QUICK_SLOT, &name, &mut job) {
            Ok(_) => "Game saved.".to_owned(),
            Err(why) => format!("Cannot save: {why}."),
        };
        feedback(&world.0, &mut barks, msg);
    }
    if keys.just_pressed(KeyCode::F9) {
        if root.0.join(QUICK_SLOT).join("save.bin").is_file() {
            load.0 = Some(QUICK_SLOT.to_owned());
        } else {
            feedback(&world.0, &mut barks, "No quick save.".to_owned());
        }
    }
}

/// Autosaves once per module entry, as soon as saving is allowed (within
/// 30 s; area-entry cutscenes block it until they end).
fn autosave(
    time: Res<Time<Real>>,
    mut loaded: MessageReader<AreaLoaded>,
    mut auto: ResMut<AutoSave>,
    world: Res<WorldRes>,
    root: Res<SaveRoot>,
    mut job: ResMut<SaveJob>,
) {
    for _ in loaded.read() {
        auto.due = if std::mem::take(&mut auto.skip_next) { None } else { Some(0.0) };
    }
    let Some(t) = auto.due.as_mut() else { return };
    *t += time.delta_secs();
    if *t < 2.0 || job.dir.is_some() {
        return;
    }
    if world.0.save_blocked().is_none() {
        let name = talk(&world.0, STR_AUTO_SAVE, "Auto Save");
        if let Err(e) = save_now(&world.0, &root, AUTO_SLOT, &name, &mut job) {
            warn!("autosave: {e}");
        }
        auto.due = None;
    } else if *t > 30.0 {
        auto.due = None;
    }
}

/// Real seconds since the last timed autosave.
#[derive(Resource, Default)]
struct TimedSave {
    elapsed: f32,
}

impl TimedSave {
    /// Advances by `dt`; true when a save is due now and `blocked` is false.
    /// A due save that is blocked (conversation, scripts, ...) is retried
    /// `TIMED_RETRY_SECS` later; a save that happens restarts the interval.
    fn step(&mut self, dt: f32, blocked: bool) -> bool {
        self.elapsed += dt;
        if self.elapsed < TIMED_SAVE_SECS {
            return false;
        }
        if blocked {
            self.elapsed = TIMED_SAVE_SECS - TIMED_RETRY_SECS;
            return false;
        }
        self.elapsed = 0.0;
        true
    }
}

/// Writes the timed slot: the same save as an autosave, but at once and
/// without a thumbnail (no UI flicker every two minutes).
pub fn write_timed(world: &k2_world::World, root: &std::path::Path) -> Result<PathBuf, String> {
    if let Some(why) = world.save_blocked() {
        return Err(why.to_owned());
    }
    let dir = save::write_slot(root, TIMED_SLOT, &world.save(), &world.save_meta("Timed Auto Save")).map_err(|e| e.to_string())?;
    let _ = std::fs::write(dir.join("digest.txt"), format!("{:#x}\n", world.state_digest()));
    Ok(dir)
}

/// Rolling autosave every two minutes of real time while in game.
fn timed_autosave(time: Res<Time<Real>>, mut timed: ResMut<TimedSave>, args: Res<crate::Args>, need: Res<NeedAreaLoad>, world: Res<WorldRes>, root: Res<SaveRoot>) {
    if args.edit {
        return;
    }
    let blocked = need.0 || world.0.save_blocked().is_some();
    if !timed.step(time.delta_secs(), blocked) {
        return;
    }
    let t0 = std::time::Instant::now();
    match write_timed(&world.0, &root.0) {
        Ok(dir) => info!("save: timed autosave {} in {:?} -> {}", TIMED_SLOT, t0.elapsed(), dir.display()),
        Err(why) => warn!("save: timed autosave skipped: {why}"),
    }
}

/// Thumbnail: hide the UI for a frame, capture, shrink to 256x144 and
/// write thumb.png into the slot.
fn run_save_job(
    mut commands: Commands,
    mut job: ResMut<SaveJob>,
    mut roots: Query<(Entity, &mut Visibility), (With<Node>, Without<ChildOf>)>,
) {
    let Some(dir) = job.dir.clone() else { return };
    job.frame += 1;
    match job.frame {
        1 => {
            let mut hidden = Vec::new();
            for (e, mut v) in &mut roots {
                if *v != Visibility::Hidden {
                    *v = Visibility::Hidden;
                    hidden.push(e);
                }
            }
            job.hidden = hidden;
        }
        3 => {
            let done = Arc::new(AtomicBool::new(false));
            job.done = Some(done.clone());
            let path = dir.join("thumb.png");
            commands.spawn(Screenshot::primary_window()).observe(move |shot: On<ScreenshotCaptured>| {
                match shot.image.clone().try_into_dynamic() {
                    Ok(img) => {
                        if let Err(e) = img.thumbnail(256, 144).to_rgb8().save(&path) {
                            warn!("save thumbnail: {e}");
                        }
                    }
                    Err(e) => warn!("save thumbnail: {e}"),
                }
                done.store(true, Ordering::Release);
            });
        }
        f if f > 3 => {
            let finished = job.done.as_ref().is_some_and(|d| d.load(Ordering::Acquire));
            if finished || f > 120 {
                for e in std::mem::take(&mut job.hidden) {
                    if let Ok((_, mut v)) = roots.get_mut(e) {
                        *v = Visibility::Inherited;
                    }
                }
                job.dir = None;
                job.done = None;
            }
        }
        _ => {}
    }
}

// -------------------------------------------------------------------
// The saveload screen
// -------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn open_saveload(
    commands: &mut Commands,
    game: &GameRes,
    world: &k2_world::World,
    root: &SaveRoot,
    ui: &mut SaveUi,
    open: &mut OpenMenu,
    existing: &Query<Entity, With<MenuScreen>>,
    mode: Mode,
    in_game: bool,
) {
    // In game the open menu (options, or this screen when it is rebuilt)
    // goes; the main menu stays under the screen.
    let gone: Vec<Entity> = if in_game { existing.iter().collect() } else { Vec::new() };
    for &e in &gone {
        commands.entity(e).despawn();
    }
    for e in [ui.screen.take().map(|s| s.0), ui.naming.take().map(|n| n.screen)].into_iter().flatten() {
        if !gone.contains(&e)
            && let Ok(mut ec) = commands.get_entity(e)
        {
            ec.despawn();
        }
    }
    ui.thumb = None;
    let Some(e) = open_screen(commands, &game.0, "saveload", Placement::Center, 210) else {
        warn!("gui: no saveload layout");
        return;
    };
    commands.entity(e).insert((SaveScreen, BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.75))));
    if in_game {
        commands.entity(e).insert(MenuScreen);
        open.0 = Some("saveload".into());
    }
    ui.slots = save::list_slots(&root.0);
    ui.rows = Vec::new();
    let mut items = Vec::new();
    if mode == Mode::Save {
        ui.rows.push(None);
        items.push(crate::gui::ListItem::text(talk(world, STR_NEW_SLOT, "New Slot")));
    }
    for (i, s) in ui.slots.iter().enumerate() {
        // Saving over the autosave is not offered; it is the game's.
        if mode == Mode::Save && (s.slot == AUTO_SLOT || s.slot == TIMED_SLOT) {
            continue;
        }
        ui.rows.push(Some(i));
        let mut label = if s.meta.name.is_empty() { s.slot.clone() } else { s.meta.name.clone() };
        if mode == Mode::Load && s.incompatible.is_some() {
            label.push_str(" (incompatible)");
        }
        items.push(crate::gui::ListItem::text(label));
    }
    let panel = talk(world, if mode == Mode::Save { STR_SAVE_GAME } else { STR_LOAD_GAME }, "");
    let button = talk(world, if mode == Mode::Save { STR_SAVE } else { STR_LOAD }, "");
    let first = (!ui.rows.is_empty()).then_some(0);
    commands.queue(move |bw: &mut World| {
        if let Some(mut scr) = bw.get_mut::<GuiScreen>(e) {
            for t in ["BTN_FILTER", "CB_CLOUDSAVE"] {
                scr.set_visible(t, false);
            }
            scr.set_text("LBL_PANELNAME", panel);
            scr.set_text("BTN_SAVELOAD", button);
            scr.set_list("LB_GAMES", items);
            scr.set_list_selected("LB_GAMES", first);
        }
    });
    ui.screen = Some((e, mode));
    ui.shown = None;
    info!("saveload: {:?} with {} slots", mode, ui.slots.len());
}

fn close_saveload(commands: &mut Commands, ui: &mut SaveUi) {
    if let Some((e, _)) = ui.screen.take()
        && let Ok(mut ec) = commands.get_entity(e)
    {
        ec.despawn();
    }
    if let Some(n) = ui.naming.take()
        && let Ok(mut ec) = commands.get_entity(n.screen)
    {
        ec.despawn();
    }
    ui.thumb = None;
    ui.shown = None;
}

/// Save Game / Load Game in the options menu, Load Game in the main menu.
#[allow(clippy::too_many_arguments)]
fn open_from_menus(
    mut commands: Commands,
    mut clicks: MessageReader<GuiClick>,
    state: Res<State<AppState>>,
    game: Res<GameRes>,
    world: Res<WorldRes>,
    root: Res<SaveRoot>,
    mut ui: ResMut<SaveUi>,
    mut open: ResMut<OpenMenu>,
    mut barks: ResMut<PendingBarks>,
    existing: Query<Entity, With<MenuScreen>>,
) {
    for c in clicks.read() {
        let in_game = *state.get() == AppState::Game;
        let mode = match (c.screen.as_str(), c.tag.as_str()) {
            (s, "BTN_LOADGAME") if s.starts_with("mainmenu") || s.starts_with("optionsingame") || s.starts_with("gameover") => Mode::Load,
            (s, "BTN_SAVEGAME") if s.starts_with("optionsingame") => Mode::Save,
            _ => continue,
        };
        if mode == Mode::Save
            && let Some(why) = world.0.save_blocked()
        {
            feedback(&world.0, &mut barks, format!("Cannot save: {why}."));
            continue;
        }
        open_saveload(&mut commands, &game, &world.0, &root, &mut ui, &mut open, &existing, mode, in_game);
    }
}

fn next_free_slot(slots: &[SlotInfo]) -> String {
    (1..).map(|n| format!("save-{n:04}")).find(|s| !slots.iter().any(|x| &x.slot == s)).expect("a free slot")
}

/// Buttons on the saveload and savename screens.
#[allow(clippy::too_many_arguments)]
fn saveload_clicks(
    mut commands: Commands,
    mut sfx: MessageWriter<UiSfx>,
    mut clicks: MessageReader<GuiClick>,
    game: Res<GameRes>,
    world: Res<WorldRes>,
    root: Res<SaveRoot>,
    mut ui: ResMut<SaveUi>,
    mut job: ResMut<SaveJob>,
    mut load: ResMut<PendingLoad>,
    mut open: ResMut<OpenMenu>,
    mut barks: ResMut<PendingBarks>,
    screens: Query<&GuiScreen>,
    existing: Query<Entity, With<MenuScreen>>,
) {
    let mut close = false;
    for c in clicks.read() {
        if c.screen.starts_with("savename") {
            match c.tag.as_str() {
                "BTN_OK" => {
                    if let Some(n) = ui.naming.take() {
                        commands.entity(n.screen).despawn();
                        let name = if n.text.trim().is_empty() { n.slot.clone() } else { n.text.trim().to_owned() };
                        match save_now(&world.0, &root, &n.slot, &name, &mut job) {
                            Ok(_) => feedback(&world.0, &mut barks, "Game saved.".into()),
                            Err(why) => feedback(&world.0, &mut barks, format!("Cannot save: {why}.")),
                        }
                        close = true;
                    }
                }
                "BTN_CANCEL" => {
                    if let Some(n) = ui.naming.take() {
                        commands.entity(n.screen).despawn();
                    }
                }
                _ => {}
            }
            continue;
        }
        if !c.screen.starts_with("saveload") {
            continue;
        }
        let Some((scr_e, mode)) = ui.screen else { continue };
        let selected = screens.get(scr_e).ok().and_then(|s| s.list_selected("LB_GAMES"));
        let row = selected.and_then(|i| ui.rows.get(i).copied());
        match c.tag.as_str() {
            // In game menus.rs closes it (Back closes any open menu).
            "BTN_BACK" if open.0.as_deref() == Some("saveload") => {
                ui.screen = None;
                ui.thumb = None;
            }
            "BTN_BACK" => close = true,
            "BTN_SAVELOAD" => match (mode, row) {
                (Mode::Load, Some(Some(i))) if ui.slots[i].incompatible.is_some() => {
                    // Refused here, with the reason on the panel (refresh_details).
                    sfx.write(UiSfx("Error_Default"));
                    warn!("load {}: {}", ui.slots[i].slot, ui.slots[i].incompatible.as_deref().unwrap_or_default());
                }
                (Mode::Load, Some(Some(i))) => {
                    load.0 = Some(ui.slots[i].slot.clone());
                    close = true;
                }
                (Mode::Save, Some(r)) => {
                    let (slot, text) = match r {
                        Some(i) => (ui.slots[i].slot.clone(), ui.slots[i].meta.name.clone()),
                        None => {
                            let slot = next_free_slot(&ui.slots);
                            let area = world.0.get(world.0.area_id).map(|a| a.name.clone()).unwrap_or_default();
                            (slot, area)
                        }
                    };
                    if let Some(e) = open_screen(&mut commands, &game.0, "savename", Placement::Center, 220) {
                        commands.entity(e).insert((SaveScreen, MenuScreen));
                        let shown = format!("{text}_");
                        commands.queue(move |bw: &mut World| {
                            if let Some(mut scr) = bw.get_mut::<GuiScreen>(e) {
                                scr.set_text("EDITBOX", shown);
                            }
                        });
                        ui.naming = Some(Naming { screen: e, text, slot });
                    }
                }
                _ => {}
            },
            "BTN_DELETE" => {
                if let Some(Some(i)) = row {
                    let slot = ui.slots[i].clone();
                    // Deleted slots move aside rather than vanish.
                    let trash = root.0.join(".deleted");
                    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    let r = std::fs::create_dir_all(&trash).and_then(|_| std::fs::rename(&slot.dir, trash.join(format!("{}-{stamp}", slot.slot))));
                    match r {
                        Ok(()) => info!("saveload: deleted {} (kept in {})", slot.slot, trash.display()),
                        Err(e) => warn!("saveload: delete {}: {e}", slot.slot),
                    }
                    let in_game = open.0.is_some();
                    open_saveload(&mut commands, &game, &world.0, &root, &mut ui, &mut open, &existing, mode, in_game);
                }
            }
            _ => {}
        }
    }
    if close {
        let in_game = open.0.as_deref() == Some("saveload");
        close_saveload(&mut commands, &mut ui);
        if in_game {
            open.0 = None;
        }
    }
    // Esc (menus.rs close_menu) despawns menu screens: forget them.
    if let Some((e, _)) = ui.screen
        && commands.get_entity(e).is_err()
    {
        ui.screen = None;
        ui.thumb = None;
        ui.naming = None;
    }
}

/// Shows the selected slot: thumbnail, names, play time, party portraits.
fn refresh_details(
    mut commands: Commands,
    mut ui: ResMut<SaveUi>,
    world: Res<WorldRes>,
    mut images: ResMut<Assets<Image>>,
    mut screens: Query<&mut GuiScreen>,
) {
    let Some((e, _)) = ui.screen else { return };
    let Ok(mut scr) = screens.get_mut(e) else { return };
    let row = scr.list_selected("LB_GAMES").and_then(|i| ui.rows.get(i).copied()).flatten();
    if ui.shown == Some(row) {
        return;
    }
    // The control nodes exist once the screen is built.
    let Some(shot) = scr.control("LBL_SCREENSHOT") else { return };
    ui.shown = Some(row);
    if let Some(t) = ui.thumb.take()
        && let Ok(mut ec) = commands.get_entity(t)
    {
        ec.despawn();
    }
    let slot = row.and_then(|i| ui.slots.get(i)).cloned();
    let m = slot.as_ref().map(|s| s.meta.clone()).unwrap_or_default();
    let hours = (m.play_time / 3600.0) as u64;
    let mins = ((m.play_time % 3600.0) / 60.0) as u64;
    scr.set_text("LBL_PCNAME", m.party.first().map(|p| p.0.clone()).unwrap_or_default());
    // A slot this build cannot read says so where the area name goes.
    let why = slot.as_ref().and_then(|s| s.incompatible.clone());
    scr.set_text("LBL_AREANAME", why.map_or_else(|| m.area_name.clone(), |w| format!("Cannot load: {w}.")));
    scr.set_text("LBL_PLANETNAME", planet_name(&m.module));
    scr.set_text("LBL_TIMEPLAYED", if slot.is_some() { format!("{hours}h {mins:02}m") } else { String::new() });
    for (i, tag) in ["LBL_PM1", "LBL_PM2", "LBL_PM3"].iter().enumerate() {
        let portrait = m.party.get(i).and_then(|p| crate::gui::hud::portrait_for(&world.0, p.1));
        scr.set_fill(tag, portrait);
    }
    if let Some(path) = slot.and_then(|s| s.thumbnail)
        && let Ok(bytes) = std::fs::read(&path)
        && let Ok(img) = Image::from_buffer(
            &bytes,
            ImageType::Extension("png"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::default(),
            RenderAssetUsages::default(),
        )
    {
        let h = images.add(img);
        let t = commands
            .spawn((
                Name::new("save thumbnail"),
                ImageNode::new(h),
                Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
                ZIndex(50),
                ChildOf(shot),
            ))
            .id();
        ui.thumb = Some(t);
    }
}

/// Planet for the save list, from the module code's suffix.
fn planet_name(module: &str) -> String {
    let m = module.to_ascii_lowercase();
    let name = match m.get(3..6).unwrap_or("") {
        "per" => "Peragus",
        "tel" => "Telos",
        "nar" => "Nar Shaddaa",
        "dxn" => "Dxun",
        "ond" => "Onderon",
        "dan" => "Dantooine",
        "kor" => "Korriban",
        "nih" => "Ravager",
        "mal" => "Malachor V",
        "ebo" => "Ebon Hawk",
        _ => "",
    };
    name.to_owned()
}

/// Typing into the savename EDITBOX. Swallows key presses so the menu
/// shortcuts (I, J, M, ...) do not fire while typing.
fn name_entry(
    mut commands: Commands,
    mut keys_in: MessageReader<KeyboardInput>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut ui: ResMut<SaveUi>,
    world: Res<WorldRes>,
    root: Res<SaveRoot>,
    mut job: ResMut<SaveJob>,
    mut barks: ResMut<PendingBarks>,
    mut open: ResMut<OpenMenu>,
    mut screens: Query<&mut GuiScreen>,
) {
    let Some(n) = ui.naming.as_mut() else {
        keys_in.clear();
        return;
    };
    let Ok(mut scr) = screens.get_mut(n.screen) else {
        ui.naming = None;
        return;
    };
    let mut submit = false;
    for k in keys_in.read() {
        if !k.state.is_pressed() {
            continue;
        }
        match &k.logical_key {
            Key::Character(s) => {
                for ch in s.chars() {
                    if (ch.is_ascii_graphic() || ch == ' ') && n.text.len() < 40 {
                        n.text.push(ch);
                    }
                }
            }
            Key::Space if n.text.len() < 40 => n.text.push(' '),
            Key::Backspace => {
                n.text.pop();
            }
            Key::Enter => submit = true,
            _ => {}
        }
    }
    scr.set_text("EDITBOX", format!("{}_", n.text));
    // Esc still cancels (menus.rs); everything else is ours.
    let esc = keys.just_pressed(KeyCode::Escape);
    keys.reset_all();
    if esc {
        keys.press(KeyCode::Escape);
    }
    if submit {
        let n = ui.naming.take().expect("naming");
        commands.entity(n.screen).despawn();
        let name = if n.text.trim().is_empty() { n.slot.clone() } else { n.text.trim().to_owned() };
        match save_now(&world.0, &root, &n.slot, &name, &mut job) {
            Ok(_) => feedback(&world.0, &mut barks, "Game saved.".into()),
            Err(why) => feedback(&world.0, &mut barks, format!("Cannot save: {why}.")),
        }
        let in_game = open.0.as_deref() == Some("saveload");
        close_saveload(&mut commands, &mut ui);
        if in_game {
            open.0 = None;
        }
    }
}

/// Replaces the world with a loaded slot and reloads the area. Runs last
/// in the frame so nothing else sees a half-swapped state.
fn apply_load(bw: &mut World) {
    let Some(slot) = bw.resource_mut::<PendingLoad>().0.take() else { return };
    let root = bw.resource::<SaveRoot>().0.clone();
    let install = bw.resource::<GameRes>().0.install.clone();
    let t0 = std::time::Instant::now();
    let loaded = save::read_slot(&root, &slot).and_then(|d: SaveData| k2_world::World::load(install, &d));
    let w = match loaded {
        Ok(w) => w,
        Err(e) => {
            error!("load {slot}: {e}");
            let leader = bw.resource::<WorldRes>().0.party_leader();
            bw.resource_mut::<PendingBarks>().0.push((leader, format!("Cannot load: {e}")));
            return;
        }
    };
    let digest = w.state_digest();
    info!("load: {slot} module {} in {:?}, digest {digest:#x}", w.module.as_ref().map_or("", |m| m.name.as_str()), t0.elapsed());
    bw.resource_mut::<SaveTest>().loaded = Some(digest);
    bw.resource_mut::<WorldRes>().0 = w;
    bw.resource_mut::<NeedAreaLoad>().0 = true;
    bw.resource_mut::<Transition>().0 = None;
    bw.resource_mut::<PendingBarks>().0.clear();
    bw.resource_mut::<PendingUse>().0 = None;
    bw.resource_mut::<InConversation>().0 = false;
    if let Some(mut t) = bw.get_resource_mut::<crate::hud::LastTarget>() {
        t.0 = None;
    }
    {
        let mut cards = bw.resource_mut::<MovieCards>();
        cards.queue.clear();
        cards.shown = 0.0;
    }
    {
        let mut fade = bw.resource_mut::<Fade>();
        fade.alpha = 1.0;
        fade.to(0.0, 0.2, 0.6, [0.0; 3]);
    }
    bw.resource_mut::<AutoSave>().skip_next = true;
    // Close every menu; the game resumes unpaused.
    let menus: Vec<Entity> = bw.query_filtered::<Entity, Or<(With<MenuScreen>, With<SaveScreen>)>>().iter(bw).collect();
    for e in menus {
        bw.despawn(e);
    }
    {
        let mut ui = bw.resource_mut::<SaveUi>();
        ui.screen = None;
        ui.naming = None;
        ui.thumb = None;
    }
    bw.resource_mut::<OpenMenu>().0 = None;
    *bw.resource_mut::<PauseState>() = PauseState::default();
    bw.resource_mut::<Paused>().0 = false;
    bw.resource_mut::<Time<Virtual>>().unpause();
    if *bw.resource::<State<AppState>>().get() != AppState::Game {
        bw.resource_mut::<NextState<AppState>>().set(AppState::Game);
    }
}

/// `K2_SAVE_OPEN=load|save`: open the screen once (main menu or game).
#[allow(clippy::too_many_arguments)]
fn env_hooks(
    mut commands: Commands,
    mut done: Local<bool>,
    mut frames: Local<u32>,
    state: Res<State<AppState>>,
    game: Res<GameRes>,
    world: Res<WorldRes>,
    root: Res<SaveRoot>,
    mut ui: ResMut<SaveUi>,
    mut open: ResMut<OpenMenu>,
    mut load: ResMut<PendingLoad>,
    existing: Query<Entity, With<MenuScreen>>,
) {
    if *done {
        return;
    }
    if let Ok(slot) = std::env::var("K2_SAVE_LOAD") {
        *frames += 1;
        if *frames >= 30 {
            *done = true;
            load.0 = Some(slot);
        }
        return;
    }
    let Ok(v) = std::env::var("K2_SAVE_OPEN") else {
        *done = true;
        return;
    };
    *frames += 1;
    let in_game = *state.get() == AppState::Game;
    // Give the main menu (or the area) a moment to come up first.
    if *frames < 30 || (in_game && world.0.module.is_none()) {
        return;
    }
    *done = true;
    let mode = if v.eq_ignore_ascii_case("save") { Mode::Save } else { Mode::Load };
    open_saveload(&mut commands, &game, &world.0, &root, &mut ui, &mut open, &existing, mode, in_game);
}

/// `K2_SAVE_TEST=quick`: quick save 3 s into the game, quick load 3 s
/// later, log the digests (they must match).
fn save_test(
    time: Res<Time<Real>>,
    mut t: ResMut<SaveTest>,
    world: Res<WorldRes>,
    root: Res<SaveRoot>,
    mut job: ResMut<SaveJob>,
    mut load: ResMut<PendingLoad>,
    need: Res<NeedAreaLoad>,
) {
    if std::env::var("K2_SAVE_TEST").ok().as_deref() != Some("quick") || need.0 || world.0.module.is_none() {
        return;
    }
    t.t += time.delta_secs();
    match t.stage {
        0 if t.t > 3.0 => match save_now(&world.0, &root, QUICK_SLOT, "Quick Save", &mut job) {
            Ok(dir) => {
                t.digest = world.0.state_digest();
                info!("save test: quick saved to {} digest {:#x}", dir.display(), t.digest);
                t.stage = 1;
                t.t = 0.0;
            }
            Err(why) => {
                if t.t > 30.0 {
                    error!("save test: never allowed to save ({why})");
                    t.stage = 9;
                }
            }
        },
        1 if t.t > 3.0 => {
            info!("save test: quick load (world now {:#x})", world.0.state_digest());
            load.0 = Some(QUICK_SLOT.to_owned());
            t.stage = 2;
        }
        2 if t.loaded.is_some() => {
            let d = t.loaded.take().unwrap_or_default();
            if d == t.digest {
                info!("save test: PASS loaded digest {d:#x} matches the save");
            } else {
                error!("save test: FAIL loaded digest {d:#x} != saved {:#x}", t.digest);
            }
            t.stage = 3;
        }
        _ => {}
    }
}

#[cfg(test)]
mod timed_tests;
