//! Party management: K2's party selection screen (partyselect_p), opened
//! when a script calls ShowPartySelectionGUI, and switching the controlled
//! character with Tab or the HUD party portraits (BTN_CHAR2 / BTN_CHAR3).
//!
//! The rules live in k2-world (party.rs).
//!
//! Test hooks: K2_PARTY_NPCS="6:p_kreia,0:p_atton,..." makes those NPC slots
//! available at the first area load; K2_PARTYSELECT=1 then opens the screen
//! as ShowPartySelectionGUI would; K2_PARTY_TAB=SECS cycles the leader as
//! Tab does, SECS after the area load.

use bevy::prelude::*;
use k2_world::party::{MAX_FOLLOWERS, RosterEntry};

use crate::game::{AppState, GameSet, InConversation, WorldRes};
use crate::gui::hud::portrait_for;
use crate::gui::menus::{MenuScreen, OpenMenu, close_menu};
use crate::gui::{GuiClick, GuiScreen, GuiSet, Placement, open_screen};
use crate::model::GameRes;
use crate::uisfx::UiSfx;

pub struct PartyUiPlugin;

impl Plugin for PartyUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PartyUi>()
            .add_systems(OnExit(AppState::Game), |mut ui: ResMut<PartyUi>| *ui = PartyUi::default())
            .add_systems(Update, test_hooks.in_set(GameSet::Input).run_if(in_state(AppState::Game)))
            .add_systems(Update, (party_select, switch_leader).chain().in_set(GuiSet::Logic).run_if(in_state(AppState::Game)));
    }
}

#[derive(Resource, Default)]
struct PartyUi {
    open: bool,
    roster: Vec<RosterEntry>,
    /// NPC slots picked so far.
    picked: Vec<i32>,
    /// The portrait last clicked.
    focus: Option<i32>,
}

#[derive(Component)]
struct PartyScreen;

#[allow(clippy::too_many_arguments)]
fn party_select(
    mut commands: Commands,
    game: Res<GameRes>,
    mut world: ResMut<WorldRes>,
    mut ui: ResMut<PartyUi>,
    mut open_menu: ResMut<OpenMenu>,
    mut clicks: MessageReader<GuiClick>,
    mut sfx: MessageWriter<UiSfx>,
    existing: Query<Entity, With<MenuScreen>>,
    mut screens: Query<&mut GuiScreen, With<PartyScreen>>,
) {
    let w = &mut world.0;
    if !ui.open {
        if w.party_select.is_none() {
            return;
        }
        // A script asked for it: open on the current party.
        close_menu(&mut commands, &mut open_menu, &existing);
        if let Some(e) = open_screen(&mut commands, &game.0, "partyselect", Placement::Center, 170) {
            commands.entity(e).insert((MenuScreen, PartyScreen, BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.75))));
        }
        open_menu.0 = Some("partyselect".into());
        let roster = w.party_roster();
        ui.picked = roster.iter().filter(|r| r.in_party || r.forced).map(|r| r.npc).collect();
        ui.focus = roster.first().map(|r| r.npc);
        ui.roster = roster;
        ui.open = true;
        sfx.write(UiSfx("Fullscreen_Panel_Added"));
        return;
    }
    // Esc or another menu closed it: same as Back.
    if open_menu.0.as_deref() != Some("partyselect") {
        ui.open = false;
        w.cancel_party_selection();
        return;
    }
    let forced = |ui: &PartyUi, n: i32| ui.roster.iter().any(|r| r.npc == n && r.forced);
    let mut done = None;
    for c in clicks.read() {
        if !c.screen.starts_with("partyselect") {
            continue;
        }
        let tag = c.tag.as_str();
        if let Some(n) = tag.strip_prefix("BTN_NPC").and_then(|n| n.parse::<i32>().ok()) {
            if ui.roster.iter().any(|r| r.npc == n) {
                // Second click on the focused member toggles it, as Accept does.
                if ui.focus == Some(n) {
                    toggle(&mut ui, n, &mut sfx, forced);
                }
                ui.focus = Some(n);
            } else {
                sfx.write(UiSfx("Error_Default"));
            }
            continue;
        }
        match tag {
            "BTN_ACCEPT" => {
                if let Some(n) = ui.focus {
                    toggle(&mut ui, n, &mut sfx, forced);
                }
            }
            "BTN_DONE" => done = Some(true),
            "BTN_BACK" => done = Some(false),
            _ => {}
        }
    }
    if let Some(apply) = done {
        let picked = ui.picked.clone();
        if apply {
            if let Err(e) = w.apply_party_selection(&picked) {
                warn!("party: {e}");
                sfx.write(UiSfx("Error_Default"));
                return;
            }
            info!("party: {:?}", picked);
        } else {
            w.cancel_party_selection();
        }
        ui.open = false;
        close_menu(&mut commands, &mut open_menu, &existing);
        sfx.write(UiSfx("Fullscreen_Panel_Removed"));
        return;
    }
    let Some(mut scr) = screens.iter_mut().next() else { return };
    let picked = ui.picked.clone();
    for n in 0..12 {
        let entry = ui.roster.iter().find(|r| r.npc == n);
        let portrait = entry.and_then(|r| {
            r.portrait
                .and_then(|p| w.tables.get(&w.res, "portraits").ok().and_then(|t| t.str(p as usize, "baseresref").map(str::to_ascii_lowercase)))
                .or_else(|| portrait_for(w, r.appearance))
        });
        scr.set_fill(&format!("BTN_NPC{n}"), portrait);
        scr.set_enabled(&format!("BTN_NPC{n}"), entry.is_some_and(|r| r.selectable || r.forced));
        scr.set_selected(&format!("BTN_NPC{n}"), picked.contains(&n));
        // LBL_NAn marks slots the party doesn't have (yet).
        scr.set_visible(&format!("LBL_NA{n}"), entry.is_none());
        scr.set_selected(&format!("LBL_CHAR{n}"), picked.contains(&n));
    }
    scr.set_visible("LBL_3D", false);
    let focus = ui.focus.and_then(|n| ui.roster.iter().find(|r| r.npc == n));
    scr.set_text("LBL_NPC_NAME", focus.map(|r| r.name.clone()).unwrap_or_default());
    scr.set_text("LBL_NPC_LEVEL", focus.map(|r| format!("Level {}", r.level)).unwrap_or_default());
    scr.set_text("LBL_COUNT", (MAX_FOLLOWERS.saturating_sub(picked.len())).to_string());
    // Accept toggles the focused member: Add / Remove.
    let focused_in = ui.focus.is_some_and(|n| picked.contains(&n));
    scr.set_text("BTN_ACCEPT", if focused_in { "Remove" } else { "Add" });
    let can_cancel = w.party_select.as_ref().is_some_and(|r| r.allow_cancel);
    scr.set_visible("BTN_BACK", can_cancel);
}

fn toggle(ui: &mut PartyUi, n: i32, sfx: &mut MessageWriter<UiSfx>, forced: impl Fn(&PartyUi, i32) -> bool) {
    if forced(ui, n) {
        sfx.write(UiSfx("Error_Default"));
        return;
    }
    if let Some(p) = ui.picked.iter().position(|&x| x == n) {
        ui.picked.remove(p);
        sfx.write(UiSfx("Clicked_Default"));
    } else if ui.picked.len() < MAX_FOLLOWERS && ui.roster.iter().any(|r| r.npc == n && r.selectable) {
        ui.picked.push(n);
        sfx.write(UiSfx("Clicked_Default"));
    } else {
        sfx.write(UiSfx("Error_Default"));
    }
}

/// Tab cycles the controlled character; clicking a HUD party portrait
/// takes control of that member.
fn switch_leader(
    keys: Res<ButtonInput<KeyCode>>,
    conv: Res<InConversation>,
    open_menu: Res<OpenMenu>,
    mut world: ResMut<WorldRes>,
    mut clicks: MessageReader<GuiClick>,
) {
    let w = &mut world.0;
    if conv.0 || open_menu.0.is_some() {
        clicks.clear();
        return;
    }
    if keys.just_pressed(KeyCode::Tab)
        && let Some(id) = w.cycle_party_leader()
    {
        info!("party: {} takes control", w.describe(id));
    }
    for c in clicks.read() {
        let slot = match c.tag.as_str() {
            "BTN_CHAR2" => 0,
            "BTN_CHAR3" => 1,
            _ => continue,
        };
        // HUD order: leader, then the others in party order.
        let leader = w.party_leader();
        let others: Vec<_> = w.party.iter().copied().filter(|&p| p != leader).collect();
        if let Some(&id) = others.get(slot)
            && w.set_party_leader(id)
        {
            info!("party: {} takes control", w.describe(id));
        }
    }
}

fn test_hooks(time: Res<Time>, mut world: ResMut<WorldRes>, mut done: Local<(bool, bool)>, mut since: Local<f32>) {
    let w = &mut world.0;
    if w.module.is_none() {
        return;
    }
    *since += time.delta_secs();
    if !done.0 && *since > 1.0 {
        done.0 = true;
        if let Ok(list) = std::env::var("K2_PARTY_NPCS") {
            for spec in list.split(',') {
                if let Some((n, tpl)) = spec.split_once(':')
                    && let Ok(n) = n.trim().parse::<i32>()
                {
                    w.available_npcs.insert(n, k2_world::AvailableNpc { template: tpl.trim().to_ascii_lowercase(), selectable: true, ..Default::default() });
                }
            }
        }
        // K2_PARTY_JOIN="6,1": apply that selection directly (no screen).
        if let Ok(list) = std::env::var("K2_PARTY_JOIN") {
            let npcs: Vec<i32> = list.split(',').filter_map(|n| n.trim().parse().ok()).collect();
            match w.apply_party_selection(&npcs) {
                Ok(()) => info!("party: joined {npcs:?} (test)"),
                Err(e) => warn!("party: {e}"),
            }
        }
        if std::env::var_os("K2_PARTYSELECT").is_some() {
            w.party_select = Some(k2_world::party::PartySelectRequest { allow_cancel: true, forced: [-1, -1], ..Default::default() });
        }
    }
    if let Some(at) = std::env::var("K2_PARTY_TAB").ok().and_then(|v| v.parse::<f32>().ok())
        && !done.1
        && *since > at
    {
        done.1 = true;
        // Same call the Tab key makes.
        if let Some(id) = w.cycle_party_leader() {
            info!("party: {} takes control (test)", w.describe(id));
        }
    }
}
