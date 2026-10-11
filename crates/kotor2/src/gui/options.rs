//! The graphics options screens: optgraphics_p (brightness, shadows, grass,
//! force speed effects, resolution, Advanced), optgraphicadv_p (texture
//! quality, anti-aliasing, anisotropy, frame buffer effects, soft shadows,
//! V-Sync) and the optresolution_p popup. Every control edits the live
//! `GraphicsSettings` resource at once (graphics.rs applies it); Cancel on
//! the advanced screen puts back the values it opened with, and the settings
//! file is written when the player leaves the graphics screens.

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use bevy::window::Monitor;

use super::menus::{MenuScreen, MenusLogic, OpenMenu, open_menu};
use super::{GuiClick, GuiScale, GuiScreen, GuiSet, GuiSlide, Placement, open_screen};
use crate::game::{AppState, WorldRes};
use crate::graphics::{ANISO_STEPS, GraphicsSettings, MSAA_STEPS, ShadowQuality, TEXTURE_QUALITY};
use crate::model::GameRes;
use crate::uisfx::UiSfx;

pub struct OptionsPlugin;

impl Plugin for OptionsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OptionsUi>().add_systems(
            Update,
            (open_hooks, clicks, fill).chain().in_set(GuiSet::Logic).after(MenusLogic).run_if(in_state(AppState::Game)),
        );
        app.add_systems(Last, persist);
    }
}

/// What the screens remember between frames.
#[derive(Resource, Default)]
struct OptionsUi {
    /// The settings when the current graphics screen opened (Cancel).
    snapshot: Option<GraphicsSettings>,
    /// Rows of the open resolution popup.
    choices: Vec<Choice>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Choice {
    Fullscreen,
    Windowed(u32, u32),
}

/// The resolution popup (a MenuScreen, so Esc closes it with the rest).
#[derive(Component)]
struct ResPopup;

/// Description strrefs shown in LB_DESC while a control is hovered.
const DESC: &[(&str, u32)] = &[
    ("BTN_RESOLUTION", 48707),
    ("CB_SHADOWS", 48708),
    ("CB_GRASS", 48709),
    ("BTN_ADVANCED", 48710),
    ("CB_SOFTSHADOWS", 48711),
    ("BTN_TEXQUAL", 48712),
    ("BTN_TEXQUALLEFT", 48712),
    ("BTN_TEXQUALRIGHT", 48712),
    ("BTN_ANTIALIAS", 48713),
    ("BTN_ANTIALIASLEFT", 48713),
    ("BTN_ANTIALIASRIGHT", 48713),
    ("CB_FRAMEBUFF", 48714),
    ("BTN_ANISOTROPY", 48715),
    ("BTN_ANISOTROPYLEFT", 48715),
    ("BTN_ANISOTROPYRIGHT", 48715),
    ("CB_VSYNC", 48716),
];

fn is_main(name: &str) -> bool {
    name.starts_with("optgraphics_")
}

fn is_adv(name: &str) -> bool {
    name.starts_with("optgraphicadv")
}

fn aa_label(samples: u32) -> String {
    match samples {
        1 => "No Anti-aliasing".to_owned(),
        n => format!("{n} sample AA"),
    }
}

fn aniso_label(n: u32) -> String {
    if n <= 1 { "Off".to_owned() } else { format!("{n}X") }
}

fn res_label(s: &GraphicsSettings) -> String {
    if s.fullscreen { "Fullscreen".to_owned() } else { format!("{} x {}", s.width, s.height) }
}

fn edit(s: &mut ResMut<GraphicsSettings>, f: impl FnOnce(&mut GraphicsSettings)) {
    let mut n = (**s).clone();
    f(&mut n);
    n.sanitize();
    if n != **s {
        **s = n;
    }
}

/// Moves `v` along `steps` by `dir` (wrapping when `wrap`).
fn step(steps: &[u32], v: u32, dir: i32, wrap: bool) -> u32 {
    let i = steps.iter().position(|&x| x == v).unwrap_or(0) as i32;
    let n = steps.len() as i32;
    let j = if wrap { (i + dir).rem_euclid(n) } else { (i + dir).clamp(0, n - 1) };
    steps[j as usize]
}

/// The popup's rows: fullscreen, then the windowed sizes (a standard set
/// plus whatever the monitors list), always including the current size.
fn choices(s: &GraphicsSettings, monitors: &Query<&Monitor>) -> Vec<Choice> {
    let mut sizes: Vec<(u32, u32)> = vec![(800, 600), (1024, 768), (1280, 720), (1280, 800), (1366, 768), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160)];
    for m in monitors {
        for v in &m.video_modes {
            sizes.push((v.physical_size.x, v.physical_size.y));
        }
    }
    sizes.push((s.width, s.height));
    sizes.retain(|&(w, h)| w >= crate::graphics::MIN_WIDTH && h >= crate::graphics::MIN_HEIGHT);
    sizes.sort_unstable();
    sizes.dedup();
    std::iter::once(Choice::Fullscreen).chain(sizes.into_iter().map(|(w, h)| Choice::Windowed(w, h))).collect()
}

fn choice_text(c: Choice) -> String {
    match c {
        Choice::Fullscreen => "Fullscreen".to_owned(),
        Choice::Windowed(w, h) => format!("{w} x {h}"),
    }
}

/// A screen that just opened remembers the settings for Cancel.
fn open_hooks(mut ui: ResMut<OptionsUi>, s: Res<GraphicsSettings>, new: Query<&GuiScreen, Added<GuiScreen>>) {
    for scr in &new {
        if is_main(&scr.name) || is_adv(&scr.name) {
            ui.snapshot = Some(s.clone());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn clicks(
    mut commands: Commands,
    mut clicks: MessageReader<GuiClick>,
    mut slides: MessageReader<GuiSlide>,
    game: Res<GameRes>,
    mut open: ResMut<OpenMenu>,
    existing: Query<Entity, With<MenuScreen>>,
    mut s: ResMut<GraphicsSettings>,
    mut ui: ResMut<OptionsUi>,
    popup: Query<&GuiScreen, With<ResPopup>>,
    popup_entity: Query<Entity, With<ResPopup>>,
    monitors: Query<&Monitor>,
    mut sfx: MessageWriter<UiSfx>,
) {
    for v in slides.read() {
        if is_main(&v.screen) && v.tag == "SLI_GAMMA" {
            let g = (v.value * 100.0).round() as u32;
            edit(&mut s, |n| n.gamma = g);
        }
    }
    for c in clicks.read() {
        let name = c.screen.as_str();
        if name.starts_with("optresolution") {
            match c.tag.as_str() {
                "BTN_OK" => {
                    let pick = popup.iter().next().and_then(|p| p.list_selected("LB_RESOLUTIONS")).and_then(|i| ui.choices.get(i).copied());
                    if let Some(ch) = pick {
                        edit(&mut s, |n| match ch {
                            Choice::Fullscreen => n.fullscreen = true,
                            Choice::Windowed(w, h) => {
                                n.fullscreen = false;
                                n.width = w;
                                n.height = h;
                            }
                        });
                    }
                    for e in &popup_entity {
                        commands.entity(e).despawn();
                    }
                }
                "BTN_CANCEL" => {
                    for e in &popup_entity {
                        commands.entity(e).despawn();
                    }
                }
                _ => {}
            }
            continue;
        }
        let main = is_main(name);
        let adv = is_adv(name);
        if !main && !adv {
            continue;
        }
        let toggle = |sfx: &mut MessageWriter<UiSfx>| {
            sfx.write(UiSfx("Checkbox_Check"));
        };
        match c.tag.as_str() {
            "CB_SHADOWS" => {
                toggle(&mut sfx);
                edit(&mut s, |n| n.shadows = if n.shadows.is_on() { ShadowQuality::Off } else { ShadowQuality::High });
            }
            "CB_GRASS" => {
                toggle(&mut sfx);
                edit(&mut s, |n| n.grass = !n.grass);
            }
            "CB_FORCESPEED" => {
                toggle(&mut sfx);
                edit(&mut s, |n| n.force_speed_fx = !n.force_speed_fx);
            }
            "CB_SOFTSHADOWS" => {
                toggle(&mut sfx);
                edit(&mut s, |n| n.soft_shadows = !n.soft_shadows);
            }
            "CB_FRAMEBUFF" => {
                toggle(&mut sfx);
                edit(&mut s, |n| n.frame_buffer_fx = !n.frame_buffer_fx);
            }
            "CB_VSYNC" => {
                toggle(&mut sfx);
                edit(&mut s, |n| n.vsync = !n.vsync);
            }
            "BTN_TEXQUAL" => edit(&mut s, |n| n.texture_quality = (n.texture_quality + 1) % TEXTURE_QUALITY.len() as u32),
            "BTN_TEXQUALLEFT" => edit(&mut s, |n| n.texture_quality = n.texture_quality.saturating_sub(1)),
            "BTN_TEXQUALRIGHT" => edit(&mut s, |n| n.texture_quality = (n.texture_quality + 1).min(TEXTURE_QUALITY.len() as u32 - 1)),
            "BTN_ANTIALIAS" => edit(&mut s, |n| n.msaa = step(&MSAA_STEPS, n.msaa, 1, true)),
            "BTN_ANTIALIASLEFT" => edit(&mut s, |n| n.msaa = step(&MSAA_STEPS, n.msaa, -1, false)),
            "BTN_ANTIALIASRIGHT" => edit(&mut s, |n| n.msaa = step(&MSAA_STEPS, n.msaa, 1, false)),
            "BTN_ANISOTROPY" => edit(&mut s, |n| n.anisotropy = step(&ANISO_STEPS, n.anisotropy, 1, true)),
            "BTN_ANISOTROPYLEFT" => edit(&mut s, |n| n.anisotropy = step(&ANISO_STEPS, n.anisotropy, -1, false)),
            "BTN_ANISOTROPYRIGHT" => edit(&mut s, |n| n.anisotropy = step(&ANISO_STEPS, n.anisotropy, 1, false)),
            "BTN_ADVANCED" if main => open_menu(&mut commands, &game, &mut open, &existing, "optgraphicadv"),
            "BTN_RESOLUTION" if main && popup_entity.is_empty() => {
                ui.choices = choices(&s, &monitors);
                open_popup(&mut commands, &game, &ui.choices, &s);
            }
            "BTN_DEFAULT" => {
                let d = GraphicsSettings::default();
                edit(&mut s, |n| {
                    if main {
                        n.gamma = d.gamma;
                        n.shadows = d.shadows;
                        n.grass = d.grass;
                        n.force_speed_fx = d.force_speed_fx;
                        n.fullscreen = d.fullscreen;
                        n.width = d.width;
                        n.height = d.height;
                    } else {
                        n.texture_quality = d.texture_quality;
                        n.msaa = d.msaa;
                        n.anisotropy = d.anisotropy;
                        n.frame_buffer_fx = d.frame_buffer_fx;
                        n.soft_shadows = d.soft_shadows;
                        n.vsync = d.vsync;
                    }
                });
            }
            "BTN_CANCEL" if adv => {
                if let Some(snap) = ui.snapshot.take() {
                    edit(&mut s, |n| {
                        n.texture_quality = snap.texture_quality;
                        n.msaa = snap.msaa;
                        n.anisotropy = snap.anisotropy;
                        n.frame_buffer_fx = snap.frame_buffer_fx;
                        n.soft_shadows = snap.soft_shadows;
                        n.vsync = snap.vsync;
                    });
                }
                open_menu(&mut commands, &game, &mut open, &existing, "optgraphics");
            }
            "BTN_BACK" if adv => open_menu(&mut commands, &game, &mut open, &existing, "optgraphics"),
            "BTN_BACK" => open_menu(&mut commands, &game, &mut open, &existing, "optionsingame"),
            _ => {}
        }
    }
}

fn open_popup(commands: &mut Commands, game: &GameRes, rows: &[Choice], s: &GraphicsSettings) {
    let Some(e) = open_screen(commands, &game.0, "optresolution", Placement::Center, 160) else { return };
    commands.entity(e).insert((MenuScreen, ResPopup, BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.4)), Interaction::default(), FocusPolicy::Block));
    let texts: Vec<String> = rows.iter().map(|&c| choice_text(c)).collect();
    let now = if s.fullscreen { Choice::Fullscreen } else { Choice::Windowed(s.width, s.height) };
    let sel = rows.iter().position(|&c| c == now);
    commands.queue(move |world: &mut World| {
        if let Some(mut scr) = world.get_mut::<GuiScreen>(e) {
            // Authored at 0,0: centre the small panel.
            let r = scr.def.root.rect;
            if r[0] == 0.0 && r[1] == 0.0 {
                let mut d = (*scr.def).clone();
                d.root.rect[0] = ((super::LAYOUT_W - r[2]) * 0.5).round();
                d.root.rect[1] = ((super::LAYOUT_H - r[3]) * 0.5).round();
                scr.def = std::sync::Arc::new(d);
            }
            // Show the current row: the box fits about five.
            if let Some(i) = scr.index("LB_RESOLUTIONS") {
                scr.ctl[i].scroll = sel.unwrap_or(0).saturating_sub(2);
            }
            scr.set_items("LB_RESOLUTIONS", texts);
            scr.set_list_selected("LB_RESOLUTIONS", sel);
        }
        // The panel art has no opaque fill: black it out so the screen
        // underneath does not show through.
        let sc = *world.resource::<GuiScale>();
        let Some(r) = world.get::<GuiScreen>(e).map(|g| g.def.root.rect) else { return };
        let (ox, oy) = ((sc.win.x - super::LAYOUT_W * sc.s) * 0.5, (sc.win.y - super::LAYOUT_H * sc.s) * 0.5);
        world.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(ox + r[0] * sc.s),
                top: Val::Px(oy + r[1] * sc.s),
                width: Val::Px(r[2] * sc.s),
                height: Val::Px(r[3] * sc.s),
                ..default()
            },
            BackgroundColor(Color::BLACK),
            ZIndex(-1),
            ChildOf(e),
        ));
    });
}

/// Keeps the controls in step with the settings and shows the description
/// of the hovered control. `K2_GUI_HOVER=TAG` fakes the hover for
/// screenshots.
fn fill(s: Res<GraphicsSettings>, world: Res<WorldRes>, mut screens: Query<&mut GuiScreen, With<MenuScreen>>, mut fake: Local<Option<Option<String>>>) {
    let fake = fake.get_or_insert_with(|| std::env::var("K2_GUI_HOVER").ok().map(|v| v.to_ascii_uppercase()));
    for mut scr in &mut screens {
        let name = scr.name.clone();
        if !is_main(&name) && !is_adv(&name) {
            continue;
        }
        if is_main(&name) {
            scr.set_progress("SLI_GAMMA", s.gamma as f32 / 100.0);
            scr.set_selected("CB_SHADOWS", s.shadows.is_on());
            scr.set_selected("CB_GRASS", s.grass);
            scr.set_selected("CB_FORCESPEED", s.force_speed_fx);
            scr.set_text("CB_FORCESPEED", "Force Speed Effects");
            let label = format!("Screen Resolution: {}", res_label(&s));
            scr.set_text("BTN_RESOLUTION", label);
        } else {
            scr.set_selected("CB_FRAMEBUFF", s.frame_buffer_fx);
            scr.set_selected("CB_SOFTSHADOWS", s.soft_shadows);
            scr.set_selected("CB_VSYNC", s.vsync);
            scr.set_text("BTN_TEXQUAL", format!("Texture Quality: {}", TEXTURE_QUALITY[s.texture_quality as usize]));
            scr.set_text("BTN_ANTIALIAS", format!("Anti-Aliasing: {}", aa_label(s.msaa)));
            scr.set_text("BTN_ANISOTROPY", format!("Anisotropy: {}", aniso_label(s.anisotropy)));
        }
        let hovered = DESC.iter().find(|(tag, _)| scr.is_hovered(tag) || fake.as_deref() == Some(*tag)).map(|&(_, r)| r);
        let text = hovered.and_then(|r| world.0.talk.as_ref().and_then(|t| t.get(r))).unwrap_or("").to_owned();
        scr.set_text("LB_DESC", text);
    }
}

/// Writes the settings file once the player leaves the graphics screens (or
/// the game quits).
fn persist(s: Res<GraphicsSettings>, open: Res<OpenMenu>, mut exit: MessageReader<AppExit>, mut dirty: Local<bool>, mut seen: Local<bool>) {
    if s.is_changed() {
        *dirty |= *seen;
        *seen = true;
    }
    let in_gfx = open.0.as_deref().is_some_and(|m| m.starts_with("optgraphic"));
    let quitting = exit.read().count() > 0;
    if *dirty && (!in_gfx || quitting) {
        s.save();
        *dirty = false;
    }
}
