//! kotor2: the game binary.
//!
//!   kotor2 [--install DIR] [--module NAME] [--newgame | --chargen] [--cam x,y,z,yaw,pitch]
//!          [--screenshot PATH --frames N] [--fly] [--autowalk SECS[,YAWDEG]]
//!          [--player-appearance ROW] [--autouse SECS] [--autotalk TAG]
//!          [--autoreply] [--autotransition] [--autoattack] [--autoattack-door TAG] [--debug]
//!          [--edit [MODULE]] [--mod-name NAME] [--mod-dir PATH]... [--edit-selftest MODULE]
//!
//! Without --module the main menu opens; New Game enters 001EBO. --chargen
//! opens character creation from the menu (New Game with the Quick defaults
//! is --newgame).
//! --cam is in game coordinates (Z-up, yaw/pitch in degrees).

mod anim;
mod armor;
mod area;
mod chargen_ui;
mod party_ui;
mod audio;
mod bark;
mod coords;
mod ctl;
mod cutscene_cam;
mod dialog_ui;
mod editor;
mod emitter;
mod flycam;
mod game;
mod gameover;
mod graphics;
mod gui;
mod hover;
mod holo;
mod hud;
mod logging;
mod material;
mod menu;
mod messages;
mod minigame;
mod model;
mod movie;
mod play;
mod progress_ui;
mod presentation;
mod projectiles;
mod saber_throw;
mod remote;
mod remote_input;
mod saves;
mod sfx;
mod shadows;
mod systems_fx;
mod tpc;
mod uisfx;
mod weapons;

use std::path::PathBuf;

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use k2_res::{Install, Resources};

use coords::WorldRoot;
use material::KMaterialPlugin;
use model::{AssetCache, GameRes};

#[derive(Resource, Clone, Debug, Default)]
pub struct Args {
    install: String,
    /// Start directly in this module (skips the menu).
    module: Option<String>,
    cam: Option<[f32; 5]>,
    screenshot: Option<PathBuf>,
    frames: u32,
    /// Start in the free-fly camera instead of third-person play.
    fly: bool,
    /// Hold forward for SECS, camera turned YAWDEG from the entry direction.
    autowalk: Option<(f32, f32)>,
    /// appearance.2da row for the player character (default: P_MAL_A_MED_01).
    player_appearance: Option<u32>,
    /// Use the targeted object SECS seconds after an area load.
    autouse: Option<f32>,
    /// Use (talk to) the object with this tag after an area load.
    autotalk: Option<String>,
    /// Pick reply 0 in conversations.
    autoreply: bool,
    /// Step into the module's transition trigger after an area load.
    autotransition: bool,
    /// Attack the nearest hostile after an area load.
    autoattack: bool,
    /// Bash the door (or placeable) with this tag after an area load.
    autoattack_door: Option<String>,
    /// Press New Game in the main menu.
    newgame: bool,
    /// Press New Game in the main menu and open character creation (not the Quick defaults).
    chargen: bool,
    /// New Game skips character generation (Quick Character defaults).
    quickstart: bool,
    /// FPS and position in the HUD.
    debug: bool,
    /// `--edit MODULE`: start in that module with the area editor open.
    edit: bool,
    /// `--edit` without a module: the editor opens on its Load Map list.
    edit_pick: bool,
    /// Editor mod name (output folder ~/.local/share/kotor2-rs/mods/<name>/).
    mod_name: Option<String>,
    /// `--mod-dir PATH` (repeatable): loose-file mod folders searched ahead
    /// of override/.
    mod_dirs: Vec<PathBuf>,
    /// `--edit-selftest MODULE`: headless editor round trip.
    edit_selftest: Option<String>,
}

fn parse_args() -> Args {
    let mut a = Args {
        install: std::env::var("K2_INSTALL").unwrap_or_else(|_| "..".into()),
        module: None,
        cam: None,
        screenshot: None,
        frames: 30,
        fly: false,
        autowalk: None,
        player_appearance: None,
        autouse: None,
        autotalk: None,
        autoattack_door: None,
        autoreply: false,
        autotransition: false,
        autoattack: false,
        newgame: false,
        chargen: false,
        quickstart: false,
        debug: false,
        edit: false,
        edit_pick: false,
        mod_name: None,
        mod_dirs: Vec::new(),
        edit_selftest: None,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let val = argv.get(i + 1).cloned().unwrap_or_default();
        let switch = match argv[i].as_str() {
            f if audio::SWITCHES.contains(&f) || ctl::SWITCHES.contains(&f) || remote::SWITCHES.contains(&f) || sfx::SWITCHES.contains(&f) || movie::SWITCHES.contains(&f) || minigame::SWITCHES.contains(&f) => true,
            "--fly" => {
                a.fly = true;
                true
            }
            "--autoreply" => {
                a.autoreply = true;
                true
            }
            "--autotransition" => {
                a.autotransition = true;
                true
            }
            "--autoattack" => {
                a.autoattack = true;
                true
            }
            "--newgame" => {
                a.newgame = true;
                true
            }
            "--chargen" => {
                a.chargen = true;
                true
            }
            "--quickstart" => {
                a.quickstart = true;
                true
            }
            "--debug" => {
                a.debug = true;
                true
            }
            _ => false,
        };
        if switch {
            i += 1;
            continue;
        }
        if argv[i] == "--edit" && (val.is_empty() || val.starts_with("--")) {
            a.module = Some("101per".into());
            a.edit = true;
            a.edit_pick = true;
            i += 1;
            continue;
        }
        match argv[i].as_str() {
            "--autowalk" => {
                let mut v = val.split(',').map(|s| s.trim().parse::<f32>().unwrap_or(0.0));
                a.autowalk = Some((v.next().unwrap_or(3.0), v.next().unwrap_or(0.0)));
            }
            "--player-appearance" => a.player_appearance = val.parse().ok(),
            "--autouse" => a.autouse = val.parse().ok(),
            "--autotalk" => a.autotalk = Some(val),
            "--autoattack-door" => a.autoattack_door = Some(val),
            "--install" => a.install = val,
            "--module" => a.module = Some(val.to_ascii_lowercase()),
            "--edit" => {
                a.module = Some(val.to_ascii_lowercase());
                a.edit = true;
            }
            "--mod-name" => a.mod_name = Some(val),
            "--mod-dir" => a.mod_dirs.push(PathBuf::from(val)),
            "--edit-selftest" => a.edit_selftest = Some(val.to_ascii_lowercase()),
            "--screenshot" => a.screenshot = Some(PathBuf::from(val)),
            "--frames" => a.frames = val.parse().unwrap_or(30),
            "--cam" => {
                let v: Vec<f32> = val.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                if v.len() == 5 {
                    a.cam = Some([v[0], v[1], v[2], v[3], v[4]]);
                }
            }
            f if anim::viewer::FLAGS.contains(&f) || ctl::FLAGS.contains(&f) || audio::FLAGS.contains(&f) || movie::FLAGS.contains(&f) || cutscene_cam::FLAGS.contains(&f) || weapons::FLAGS.contains(&f) || progress_ui::FLAGS.contains(&f) => {}
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
        i += 2;
    }
    a
}

fn main() {
    let args = parse_args();
    let mut install = Install::discover(&args.install).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });
    // --mod-dir folders shadow override/ (and any K2_MOD_DIR ones).
    for (i, d) in args.mod_dirs.iter().enumerate() {
        install.mod_dirs.insert(i, d.clone());
    }
    if let Some(m) = &args.edit_selftest {
        std::process::exit(editor::selftest(install, m));
    }
    let res = Resources::open(install.clone()).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });

    if sfx::wants_survey() {
        sfx::run_survey(&res, &install);
        return;
    }
    if audio::wants_survey() {
        audio::run_survey(&res);
        return;
    }
    if let Some(m) = audio::audio_test_module() {
        let mut res = res;
        audio::run_audio_test(&mut res, &m);
        return;
    }
    let test_window = args.screenshot.is_some() || std::env::var_os("K2_TEST_WINDOW").is_some_and(|v| v == "1");
    let viewer = anim::viewer::from_env();
    let gfx = graphics::GraphicsSettings::load();
    logging::install();
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            // The user's Hyprland rule sends title ^kotor2-rs$ to a hidden
            // workspace, where no frames are delivered; test runs use a
            // different title so they render on the current workspace.
            title: if test_window { "kotor2-rs-test" } else { "kotor2-rs" }.into(),
            // Wayland app_id / X11 class, so Hyprland rules can match native
            // Wayland windows too (their initial title is empty).
            name: Some(if test_window { "kotor2-test" } else { "kotor2" }.into()),
            resolution: (gfx.width, gfx.height).into(),
            present_mode: gfx.present_mode(),
            mode: gfx.window_mode(),
            ..default()
        }),
        ..default()
    })
    // logging.rs installs the subscriber (its filter can change at runtime).
    .disable::<bevy::log::LogPlugin>())
    .insert_resource(gfx)
    .add_plugins((KMaterialPlugin, flycam::FlyCamPlugin, anim::AnimPlugin, graphics::GraphicsPlugin))
    .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.03)))
    .init_resource::<AssetCache>()
    .add_systems(Update, screenshot_mode);
    match viewer {
        Some(v) => {
            app.insert_resource(GameRes(res)).insert_resource(args).insert_resource(v).add_plugins((anim::viewer::ViewerPlugin, holo::HoloPlugin));
        }
        None => {
            let world = game::make_world(install.clone(), &args);
            app.insert_resource(GameRes(res))
                .insert_resource(args)
                .insert_resource(game::WorldRes(world))
                .insert_resource(area::GameData { tables: k2_data::Tables::new() })
                .add_plugins((
                    game::GamePlugin,
                    play::PlayPlugin,
                    hover::HoverPlugin,
                    audio::KAudioPlugin,
                    sfx::SfxPlugin,
                    projectiles::ProjectilesPlugin,
                    saber_throw::SaberThrowPlugin,
                    presentation::PresentationPlugin,
                    dialog_ui::DialogUiPlugin,
                    bark::BarkPlugin,
                    hud::HudPlugin,
                    menu::MenuPlugin,
                    weapons::WeaponsPlugin,
                    armor::ArmorPlugin,
                ))
                .add_plugins((movie::MoviePlugin, cutscene_cam::CutsceneCamPlugin, anim::lipsync::LipSyncPlugin, gui::GuiPlugin, saves::SavesPlugin, ctl::CtlPlugin, uisfx::UiSfxPlugin, chargen_ui::ChargenPlugin, party_ui::PartyUiPlugin, progress_ui::ProgressUiPlugin, holo::HoloPlugin))
                .add_plugins(editor::EditorPlugin)
                .add_plugins(gameover::GameOverPlugin)
                .add_plugins(systems_fx::SystemsFxPlugin)
                .add_plugins(shadows::ShadowPlugin)
                .add_plugins(minigame::MiniGamePlugin)
                .add_plugins(remote::RemotePlugin)
                .add_systems(Startup, (setup_scene, menu::direct_start).chain());
        }
    }
    app.run();
}

/// The permanent world root and camera; areas are loaded under it later.
fn setup_scene(mut commands: Commands, args: Res<Args>) {
    commands.spawn((Name::new("world"), WorldRoot, coords::world_root_transform(), Visibility::default()));
    play::spawn_camera(&mut commands, &args);
}

fn screenshot_mode(mut commands: Commands, args: Res<Args>, mut frame: Local<u32>, mut exit: MessageWriter<AppExit>) {
    let Some(path) = &args.screenshot else { return };
    *frame += 1;
    if *frame == args.frames {
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path.clone()));
    }
    // The capture lands a few frames later; wait for the file, then quit.
    if *frame > args.frames && path.exists() {
        exit.write(AppExit::Success);
    }
    if *frame > args.frames + 600 {
        error!("screenshot never arrived");
        exit.write(AppExit::error());
    }
}
