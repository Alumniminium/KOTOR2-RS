//! Game start: New Game (from the main menu GUI or `--newgame`) and the
//! direct `--module` start. The menus themselves are the game's own GUIs
//! (`gui::menus`).

use bevy::prelude::*;

use crate::Args;
use crate::game::{AppState, Fade, NeedAreaLoad, WorldRes, enter_module};

/// The module New Game starts in: the K2 prologue on the Ebon Hawk.
pub const NEW_GAME_MODULE: &str = "001ebo";

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, auto_new_game.run_if(in_state(AppState::Menu)));
    }
}

pub fn start_new_game(world: &mut WorldRes, need: &mut NeedAreaLoad, fade: &mut Fade, next: &mut NextState<AppState>) {
    // Test hook: K2_NEWGAME_MODULE starts the new game elsewhere (e.g. 101per
    // to see the generated PC, who sits out the 001ebo prologue).
    let module = std::env::var("K2_NEWGAME_MODULE").unwrap_or_else(|_| NEW_GAME_MODULE.to_owned());
    info!("new game: entering {module}");
    fade.alpha = 1.0;
    fade.to(0.0, 0.3, 0.8, [0.0; 3]);
    if enter_module(&mut world.0, need, &module, None) {
        next.set(AppState::Game);
    }
}

/// The editor saves into the mods folder (editor::mod_path), so a control run
/// (--control, the MCP supervisor's) never opens it.
pub fn editor_allowed() -> bool {
    !crate::remote::enabled()
}

/// The main menu's Editor button: enters the editor's default start area
/// (the editor opens on its Load Map list, see editor::Editor::request_from_menu).
/// The request is made only when the module is entered, so a failed start
/// does not open the editor on the next area load.
pub fn start_editor(world: &mut WorldRes, need: &mut NeedAreaLoad, fade: &mut Fade, next: &mut NextState<AppState>, editor: &mut crate::editor::Editor) {
    if !editor_allowed() {
        warn!("editor: not opened in a control run");
        return;
    }
    fade.alpha = 1.0;
    fade.to(0.0, 0.3, 0.8, [0.0; 3]);
    if enter_module(&mut world.0, need, crate::editor::START_MODULE, None) {
        editor.request_from_menu();
        next.set(AppState::Game);
    }
}

/// `--newgame`: press New Game on the first menu frame. `--chargen` presses it
/// too, but opens character creation instead of starting with the Quick defaults.
fn auto_new_game(
    args: Res<Args>,
    mut done: Local<bool>,
    mut world: ResMut<WorldRes>,
    mut need: ResMut<NeedAreaLoad>,
    mut fade: ResMut<Fade>,
    mut next: ResMut<NextState<AppState>>,
    mut chargen: ResMut<crate::chargen_ui::ChargenUi>,
) {
    if !(args.newgame || args.chargen) || *done {
        return;
    }
    *done = true;
    if args.chargen {
        chargen.request = true;
        return;
    }
    crate::chargen_ui::quick_default(&mut world.0);
    start_new_game(&mut world, &mut need, &mut fade, &mut next);
}

/// Direct start with `--module`: skip the menu.
pub fn direct_start(
    args: Res<Args>,
    mut world: ResMut<WorldRes>,
    mut need: ResMut<NeedAreaLoad>,
    mut fade: ResMut<Fade>,
    mut next: ResMut<NextState<AppState>>,
) {
    let Some(m) = args.module.clone() else { return };
    crate::chargen_ui::quick_default(&mut world.0);
    fade.alpha = 0.0;
    fade.target = 0.0;
    if enter_module(&mut world.0, &mut need, &m, None) {
        next.set(AppState::Game);
    }
}
