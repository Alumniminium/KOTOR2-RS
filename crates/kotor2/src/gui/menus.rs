//! Full-screen GUIs: the main menu, the in-game options menu (Esc), the
//! pause box, and the in-game menu screens (equipment, inventory, ...) with
//! the shared top bar. Menu screens pause the game while open, like the
//! original.

use bevy::app::AppExit;
use bevy::prelude::*;

use super::layout::GuiDef;
use super::{GuiClick, GuiScreen, GuiSet, Placement, open_screen};
use crate::game::{AppState, InConversation, Paused, WorldRes};
use crate::model::GameRes;

pub struct MenusPlugin;

/// The menu bookkeeping systems; the screen fillers run after them.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct MenusLogic;

impl Plugin for MenusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PauseState>()
            .init_resource::<OpenMenu>()
            .add_systems(OnEnter(AppState::Menu), spawn_main_menu)
            .add_systems(OnExit(AppState::Menu), despawn::<MainMenu>)
            .add_systems(OnExit(AppState::Game), (despawn::<MenuScreen>, despawn::<PauseBox>, reset_menu))
            .add_systems(Update, main_menu_clicks.in_set(GuiSet::Logic).run_if(in_state(AppState::Menu)))
            .add_systems(
                Update,
                (env_open, menu_keys, menu_clicks, fill_shells, show_pause_box, apply_pause).chain().in_set(GuiSet::Logic).in_set(MenusLogic).run_if(in_state(AppState::Game)),
            );
    }
}

#[derive(Component)]
pub struct MainMenu;

/// An in-game menu screen (options, inventory, ...) or its top bar.
#[derive(Component)]
pub struct MenuScreen;

#[derive(Component)]
struct PauseBox;

/// Why the game is paused: the player's pause toggle, or a menu screen.
#[derive(Resource, Default)]
pub struct PauseState {
    pub user: bool,
    pub menu: bool,
    /// The area editor is open (F10 / --edit).
    pub editor: bool,
}

/// The in-game menu currently shown (base name, e.g. "inventory").
#[derive(Resource, Default)]
pub struct OpenMenu(pub Option<String>);

fn despawn<T: Component>(mut commands: Commands, q: Query<Entity, With<T>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

fn reset_menu(mut open: ResMut<OpenMenu>, mut pause: ResMut<PauseState>) {
    open.0 = None;
    *pause = PauseState::default();
}

/// Labels whose layout text is designer sample data ("ABCDEFG...", "99",
/// "CLASS 1"): blank them until a screen fills them in.
fn blank_samples(scr: &mut GuiScreen) {
    let tags: Vec<String> = scr
        .def
        .controls
        .iter()
        .filter(|c| c.kind != super::layout::Kind::Button && c.text.as_ref().is_some_and(|t| t.strref.is_none() && !t.text.is_empty()))
        .map(|c| c.tag.clone())
        .collect();
    for t in tags {
        scr.set_text(&t, "");
    }
}

fn spawn_main_menu(mut commands: Commands, game: Res<GameRes>) {
    let Some(e) = open_screen(&mut commands, &game.0, "mainmenu", Placement::Center, 200) else {
        error!("gui: no main menu layout");
        return;
    };
    commands.entity(e).insert((MainMenu, BackgroundColor(Color::BLACK)));
    commands.queue(move |world: &mut World| {
        if let Some(mut scr) = world.get_mut::<GuiScreen>(e) {
            // Aspyr/debug extras: the warp module list, promo buttons, DLC note.
            for tag in ["LB_MODULES", "BTN_WARP", "BTN_TSLRCM", "LBL_NEWCONTENT"] {
                scr.set_visible(tag, false);
            }
            // Load Game: saves.rs.
            for tag in ["BTN_MOVIES", "BTN_MUSIC", "BTN_OPTIONS"] {
                scr.set_enabled(tag, false);
            }
            add_editor_button(&mut scr, crate::menu::editor_allowed());
        }
    });
}

/// The Editor button takes the hidden More Games slot: that control is
/// renamed BTN_EDITOR and moved up to the Exit row, and Exit steps down into
/// the freed slot. It is the layout's own button, so it keeps the look. When
/// the editor is not allowed (a control run) the slot is only hidden.
fn add_editor_button(scr: &mut GuiScreen, allowed: bool) {
    let mut d = (*scr.def).clone();
    if !allowed || !editor_slot(&mut d) {
        scr.set_visible("BTN_MOREGAMES", false);
        return;
    }
    scr.def = std::sync::Arc::new(d);
    scr.set_text("BTN_EDITOR", "Editor");
}

/// The swap behind `add_editor_button`, on the layout. The keyboard
/// neighbours (MOVETO) follow the places (`GuiDef::swap_places`). False when
/// the layout lacks More Games or Exit.
fn editor_slot(d: &mut GuiDef) -> bool {
    let pos = |tag: &str| d.controls.iter().position(|c| c.tag == tag);
    let (Some(more), Some(exit)) = (pos("BTN_MOREGAMES"), pos("BTN_EXIT")) else {
        return false;
    };
    d.swap_places(exit, more);
    d.controls[more].tag = "BTN_EDITOR".into();
    true
}

#[cfg(test)]
mod tests;

#[allow(clippy::too_many_arguments)]
fn main_menu_clicks(
    args: Res<crate::Args>,
    mut chargen: ResMut<crate::chargen_ui::ChargenUi>,
    mut clicks: MessageReader<GuiClick>,
    mut world: ResMut<WorldRes>,
    mut need: ResMut<crate::game::NeedAreaLoad>,
    mut fade: ResMut<crate::game::Fade>,
    mut next: ResMut<NextState<AppState>>,
    mut editor: ResMut<crate::editor::Editor>,
    mut exit: MessageWriter<AppExit>,
) {
    for c in clicks.read() {
        if !c.screen.starts_with("mainmenu") {
            continue;
        }
        match c.tag.as_str() {
            "BTN_NEWGAME" if args.quickstart => {
                crate::chargen_ui::quick_default(&mut world.0);
                crate::menu::start_new_game(&mut world, &mut need, &mut fade, &mut next);
            }
            "BTN_NEWGAME" => chargen.request = true,
            "BTN_EDITOR" => crate::menu::start_editor(&mut world, &mut need, &mut fade, &mut next, &mut editor),
            "BTN_EXIT" => {
                exit.write(AppExit::Success);
            }
            _ => {}
        }
    }
}

/// Menu screen base names behind the HUD / top bar buttons.
pub fn menu_for_button(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "BTN_EQU" => "equip",
        "BTN_INV" => "inventory",
        "BTN_CHAR" => "character",
        "BTN_ABI" => "abilities",
        "BTN_MSG" => "messages",
        "BTN_JOU" => "journal",
        "BTN_MAP" => "map",
        "BTN_OPT" => "optionsingame",
        "BTN_GRAPHICS" => "optgraphics",
        _ => return None,
    })
}

/// Opens a menu screen (closing any other). The options menu stands alone;
/// the rest get the top bar with the screen buttons and party portraits.
pub fn open_menu(commands: &mut Commands, game: &GameRes, open: &mut OpenMenu, existing: &Query<Entity, With<MenuScreen>>, base: &str) {
    for e in existing {
        commands.entity(e).despawn();
    }
    open.0 = None;
    let Some(e) = open_screen(commands, &game.0, base, Placement::Center, 150) else {
        warn!("gui: no layout for menu {base}");
        return;
    };
    commands.entity(e).insert((MenuScreen, BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55))));
    let options = base == "optionsingame";
    let standalone = options || base.starts_with("optgraphic") || super::screens::STANDALONE.contains(&base);
    commands.queue(move |world: &mut World| {
        if let Some(mut scr) = world.get_mut::<GuiScreen>(e) {
            // Small panels (container_p is 305x327) are authored at 0,0:
            // centre them in the 800x600 space.
            let r = scr.def.root.rect;
            if r[0] == 0.0 && r[1] == 0.0 && (r[2] < 700.0 || r[3] < 500.0) {
                let mut d = (*scr.def).clone();
                d.root.rect[0] = ((super::LAYOUT_W - r[2]) * 0.5).round();
                d.root.rect[1] = ((super::LAYOUT_H - r[3]) * 0.5).round();
                scr.def = std::sync::Arc::new(d);
            }
            blank_samples(&mut scr);
            if options {
                // Save Game / Load Game: saves.rs.
                for t in ["BTN_GAMEPLAY", "BTN_FEEDBACK", "BTN_AUTOPAUSE", "BTN_SOUND"] {
                    scr.set_enabled(t, false);
                }
                scr.set_visible("LB_DESC", false);
            }
        }
    });
    if !standalone && let Some(top) = open_screen(commands, &game.0, "top", Placement::Center, 151) {
        commands.entity(top).insert(MenuScreen);
        commands.queue(move |world: &mut World| {
            if let Some(mut scr) = world.get_mut::<GuiScreen>(top) {
                blank_samples(&mut scr);
            }
        });
    }
    open.0 = Some(base.to_owned());
}

pub fn close_menu(commands: &mut Commands, open: &mut OpenMenu, existing: &Query<Entity, With<MenuScreen>>) {
    for e in existing {
        commands.entity(e).despawn();
    }
    open.0 = None;
}

/// Test hooks: `K2_GUI_OPEN=inventory` opens that menu once the game runs;
/// `K2_GUI_GIVE=template[:n],...` puts items in the party inventory first.
fn env_open(
    mut commands: Commands,
    game: Res<GameRes>,
    mut world: ResMut<WorldRes>,
    mut done: Local<bool>,
    mut open: ResMut<OpenMenu>,
    existing: Query<Entity, With<MenuScreen>>,
) {
    if *done {
        return;
    }
    *done = true;
    if let Ok(list) = std::env::var("K2_GUI_GIVE") {
        let w = &mut world.0;
        for spec in list.split(',').filter(|s| !s.is_empty()) {
            let (t, n) = spec.split_once(':').map_or((spec, 1), |(t, n)| (t, n.parse().unwrap_or(1)));
            let pc = w.player;
            w.give_new_item(t, pc, n, true);
        }
    }
    if let Ok(m) = std::env::var("K2_GUI_OPEN") {
        open_menu(&mut commands, &game, &mut open, &existing, &m.to_ascii_lowercase());
    }
}

/// Esc: close the open menu, or open the options menu. I inventory,
/// U equipment, C character, K abilities, L messages, J journal, M map
/// (toggle). Space: pause (not inside a minigame, where it jumps or fires).
#[allow(clippy::too_many_arguments)]
fn menu_keys(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    game: Res<GameRes>,
    world: Res<WorldRes>,
    conv: Res<InConversation>,
    mut open: ResMut<OpenMenu>,
    mut pause: ResMut<PauseState>,
    existing: Query<Entity, With<MenuScreen>>,
) {
    let shortcut = [
        (KeyCode::KeyI, "inventory"),
        (KeyCode::KeyU, "equip"),
        (KeyCode::KeyC, "character"),
        (KeyCode::KeyK, "abilities"),
        (KeyCode::KeyL, "messages"),
        (KeyCode::KeyJ, "journal"),
        (KeyCode::KeyM, "map"),
    ]
    .into_iter()
    .find(|(k, _)| keys.just_pressed(*k))
    .map(|(_, m)| m);
    // A door's message box takes Escape for itself (dialog_ui ends the talk).
    let box_up = world.0.conversation_view().is_some_and(|v| v.boxed);
    if keys.just_pressed(KeyCode::Escape) && !box_up {
        if open.0.is_some() {
            close_menu(&mut commands, &mut open, &existing);
        } else {
            open_menu(&mut commands, &game, &mut open, &existing, "optionsingame");
        }
    } else if let Some(m) = shortcut {
        if conv.0 {
            return;
        }
        if open.0.as_deref() == Some(m) {
            close_menu(&mut commands, &mut open, &existing);
        } else {
            open_menu(&mut commands, &game, &mut open, &existing, m);
        }
    } else if keys.just_pressed(KeyCode::Space) && !conv.0 && open.0.is_none() && world.0.minigame.is_none() {
        pause.user = !pause.user;
    }
}

#[allow(clippy::too_many_arguments)]
fn menu_clicks(
    mut commands: Commands,
    mut clicks: MessageReader<GuiClick>,
    game: Res<GameRes>,
    mut open: ResMut<OpenMenu>,
    mut pause: ResMut<PauseState>,
    existing: Query<Entity, With<MenuScreen>>,
    mut exit: MessageWriter<AppExit>,
) {
    for c in clicks.read() {
        let screen = c.screen.as_str();
        if screen.starts_with("mainmenu") {
            continue;
        }
        if screen.starts_with("optionsingame") && c.tag == "BTN_QUIT" {
            exit.write(AppExit::Success);
            continue;
        }
        if screen.starts_with("pause") && c.tag == "BTN_UNPAUSE" {
            pause.user = false;
            continue;
        }
        if screen.starts_with("mipc") && c.tag == "TB_PAUSE" {
            pause.user = !pause.user;
            continue;
        }
        // The craft screens handle their own Back (it steps back a screen).
        let own_back = ["upgrade", "component", "chemical", "optgraphic"].iter().any(|p| screen.starts_with(p));
        let store_cancel = screen.starts_with("store") && c.tag == "BTN_Cancel";
        if (store_cancel || matches!(c.tag.as_str(), "BTN_EXIT" | "BTN_BACK" | "BTN_CLOSE")) && open.0.is_some() && !(own_back && c.tag == "BTN_BACK") {
            close_menu(&mut commands, &mut open, &existing);
            continue;
        }
        if let Some(m) = menu_for_button(&c.tag) {
            if open.0.as_deref() == Some(m) {
                close_menu(&mut commands, &mut open, &existing);
            } else {
                open_menu(&mut commands, &game, &mut open, &existing, m);
            }
        }
    }
}

/// The top bar: the shown party member's name, class, portrait and bars,
/// the other members' portraits (click to switch), the current tab.
fn fill_shells(
    mut world: ResMut<WorldRes>,
    open: Res<OpenMenu>,
    mut st: ResMut<super::screens::ScreenState>,
    mut clicks: MessageReader<GuiClick>,
    mut screens: Query<&mut GuiScreen, With<MenuScreen>>,
) {
    if open.0.is_none() {
        clicks.clear();
        return;
    }
    let party = super::screens::party(&world.0);
    for c in clicks.read() {
        if c.screen.starts_with("top") {
            let k = match c.tag.as_str() {
                "BTN_CHANGE2" => 1,
                "BTN_CHANGE3" => 2,
                _ => continue,
            };
            if party.len() > 1 {
                st.member = (st.member + k) % party.len();
                st.equip_slot = None;
                st.equip_hover = None;
            }
        }
    }
    if st.member >= party.len() {
        st.member = 0;
    }
    let w = &mut world.0;
    let me = super::screens::member(w, &st);
    let sheet = w.character_sheet(me);
    let Some(o) = w.get(me) else { return };
    let Some(c) = o.creature() else { return };
    let portrait = w.portrait_resref(me);
    let others: Vec<Option<String>> = (1..3)
        .map(|k| {
            if party.len() <= k {
                return None;
            }
            let id = party[(st.member + k) % party.len()];
            w.portrait_resref(id)
        })
        .collect();
    let (hp, max_hp, force, max_force) = (c.hp, c.max_hp, c.force, c.max_force);
    for mut scr in &mut screens {
        if !scr.name.starts_with("top") {
            continue;
        }
        if let Some(sh) = &sheet {
            scr.set_text("LBL_CHARNAME", sh.name.clone());
            let class = |i: usize| sh.classes.get(i).cloned().unwrap_or_default();
            scr.set_text("LBL_TOP_CLASS1", class(0).0);
            scr.set_text("LBL_TOP_CLASS1LEVEL", if sh.classes.is_empty() { String::new() } else { class(0).1.to_string() });
            scr.set_text("LBL_TOP_CLASS2", class(1).0);
            scr.set_text("LBL_TOP_CLASS2LEVEL", if sh.classes.len() > 1 { class(1).1.to_string() } else { String::new() });
        }
        scr.set_fill("LBL_CHAR1", portrait.clone());
        scr.set_progress("PB_VIT1", hp as f32 / max_hp.max(1) as f32);
        scr.set_progress("PB_FORCE1", if max_force > 0 { force as f32 / max_force as f32 } else { 0.0 });
        for (k, tag) in [(0, "2"), (1, "3")] {
            let p = others[k].clone();
            scr.set_visible(&format!("LBL_CHAR{tag}"), p.is_some());
            scr.set_visible(&format!("BTN_CHANGE{tag}"), p.is_some());
            scr.set_visible(&format!("LBL_BACK{tag}"), p.is_some());
            scr.set_fill(&format!("LBL_CHAR{tag}"), p);
        }
        let current = open.0.as_deref().unwrap_or("");
        for tag in ["LBLH_EQU", "LBLH_INV", "LBLH_CHA", "LBLH_ABI", "LBLH_MSG", "LBLH_JOU", "LBLH_MAP", "LBLH_OPT"] {
            let btn = tag.replace("LBLH_", "BTN_").replace("BTN_CHA", "BTN_CHAR");
            scr.set_selected(tag, menu_for_button(&btn) == Some(current));
        }
        for t in ["LBL_DISABLE1", "LBL_DEBILATATED1", "LBL_LEVELUP1", "LBL_LEVELUP2", "LBL_LEVELUP3", "LBL_CMBTEFCTINC1", "LBL_CMBTEFCTRED1"] {
            scr.set_visible(t, false);
        }
    }
}

fn show_pause_box(mut commands: Commands, game: Res<GameRes>, pause: Res<PauseState>, boxes: Query<Entity, With<PauseBox>>) {
    if !pause.is_changed() {
        return;
    }
    let shown = !boxes.is_empty();
    if pause.user && !shown {
        if let Some(e) = open_screen(&mut commands, &game.0, "pause", Placement::Center, 120) {
            commands.entity(e).insert(PauseBox);
            commands.queue(move |world: &mut World| {
                if let Some(mut scr) = world.get_mut::<GuiScreen>(e) {
                    scr.set_text("LBL_PAUSEREASON", "PAUSED");
                }
            });
        }
    } else if !pause.user {
        for e in &boxes {
            commands.entity(e).despawn();
        }
    }
}

fn apply_pause(open: Res<OpenMenu>, mut pause: ResMut<PauseState>, mut paused: ResMut<Paused>, mut time: ResMut<Time<Virtual>>) {
    let menu = open.0.is_some();
    if pause.menu != menu {
        pause.menu = menu;
    }
    let on = pause.user || pause.menu || pause.editor;
    if paused.0 != on {
        paused.0 = on;
        if on {
            time.pause();
        } else {
            time.unpause();
        }
    }
}
