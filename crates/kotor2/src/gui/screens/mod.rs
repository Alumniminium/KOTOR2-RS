//! The in-game screens behind the menu bar and the world's screen requests:
//! inventory, equipment, character, abilities, journal, messages, map (plus
//! the HUD minimap), container loot, store, and the workbench / lab station
//! screens. Each fills its K2 .gui layout from k2-world every frame (the
//! GuiScreen setters only redraw what changed) and turns clicks into world
//! calls.

mod craft;
mod inventory;
mod log;
mod map;
mod sheet;
mod trade;

use bevy::prelude::*;
use k2_script::ObjectId;
use k2_world::craft::Station;
use k2_world::inventory::ItemView;
use k2_world::{World, WorldEvent};

use super::GuiSet;
use super::menus::{MenuScreen, OpenMenu, open_menu};
use crate::game::{AppState, ApplyEvents, FrameEvents, GameSet, WorldRes};
use crate::model::GameRes;
use crate::uisfx::UiSfx;

pub use log::MessageLog;

pub struct ScreensPlugin;

impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScreenState>()
            .init_resource::<MessageLog>()
            .add_systems(OnExit(AppState::Game), |mut s: ResMut<ScreenState>| *s = ScreenState::default())
            .add_systems(Update, (screen_requests, log::collect).chain().in_set(GameSet::Ui).after(ApplyEvents))
            .add_systems(
                Update,
                (
                    track_close,
                    inventory::inventory,
                    inventory::equip,
                    trade::container,
                    trade::store,
                    sheet::character,
                    sheet::abilities,
                    log::journal,
                    log::messages,
                    map::map,
                    craft::upgrade_select,
                    craft::upgrade,
                    craft::create,
                )
                    .chain()
                    .in_set(GuiSet::Logic)
                    .after(super::menus::MenusLogic)
                    .run_if(in_state(AppState::Game)),
            )
            .add_systems(Update, map::minimap.in_set(GuiSet::Logic).run_if(in_state(AppState::Game)))
            .add_systems(Update, env_tests.in_set(GameSet::Input))
            // Any state: chargen runs in the menu.
            .add_systems(Update, env_clicks.in_set(GuiSet::Input));
    }
}

/// Screens opened without the top bar (world requests, not menu tabs).
pub const STANDALONE: &[&str] = &["container", "store", "upgradesel", "upgrade", "component", "chemical"];

/// Inventory / store filter tabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Filter {
    #[default]
    All,
    Datapads,
    Weapons,
    Armor,
    Useable,
    Quests,
    Misc,
}

/// The workbench / lab station session the world asked for.
#[derive(Clone, Debug)]
pub struct CraftUi {
    pub station: Station,
    pub character: ObjectId,
    pub recipes: String,
    pub item_creation: bool,
    pub upgrade: bool,
    /// Item being upgraded (upgrade_p), and its selected slot.
    pub item: Option<ObjectId>,
    pub slot: Option<usize>,
    /// upgradesel filter: -1 all, else 0 ranged, 1 melee, 2 armour, 3 saber.
    pub kind: i32,
    /// component/chemical: creating (true) or breaking down.
    pub creating: bool,
    /// Recipe group (create tabs) or breakdown filter.
    pub group: i32,
    pub status: String,
}

#[derive(Resource, Default)]
pub struct ScreenState {
    /// Party index whose inventory/equipment/sheet the menus show.
    pub member: usize,
    pub inv_filter: Filter,
    pub equip_slot: Option<i32>,
    /// equip_p: slot whose candidates are previewed because the mouse is on
    /// it (or just left it for the list), and seconds since it was last
    /// hovered.
    pub equip_hover: Option<(i32, f32)>,
    /// 0 skills, 1 powers, 2 feats.
    pub abil_tab: u8,
    /// 0 dialog, 1 feedback, 2 combat, 3 effects.
    pub msg_tab: u8,
    pub journal_completed: bool,
    /// 0 time, 1 name, 2 priority, 3 planet.
    pub journal_sort: u8,
    pub container: Option<ObjectId>,
    /// A container opened while a conversation ran (its talk on use): its
    /// window opens when the conversation ends.
    pub container_held: Option<ObjectId>,
    pub store: Option<ObjectId>,
    pub store_sell: bool,
    pub store_filter: Filter,
    pub craft: Option<CraftUi>,
    /// Last menu seen open, to run cleanup when it closes.
    last_open: Option<String>,
    /// Short status text shown on the screen that set it.
    pub status: Option<String>,
}

/// Party members in HUD order (leader first).
pub fn party(w: &World) -> Vec<ObjectId> {
    let mut v = w.party.clone();
    if v.is_empty() {
        v.push(w.party_leader());
    }
    v
}

/// The member the menu screens show.
pub fn member(w: &World, st: &ScreenState) -> ObjectId {
    let p = party(w);
    p.get(st.member).copied().unwrap_or_else(|| w.party_leader())
}

pub fn classify(w: &mut World, v: &ItemView) -> Filter {
    let label = v.base_label.to_ascii_lowercase();
    if label.contains("datapad") || label.contains("data_pad") || label.contains("data pad") {
        Filter::Datapads
    } else if v.plot {
        Filter::Quests
    } else if v.equip_slots & ((1 << 4) | (1 << 5)) != 0 {
        Filter::Weapons
    } else if v.equip_slots & ((1 << 0) | (1 << 1) | (1 << 3) | (1 << 7) | (1 << 8) | (1 << 9) | (1 << 10)) != 0 {
        Filter::Armor
    } else if w.is_usable_item(v.id) {
        Filter::Useable
    } else {
        Filter::Misc
    }
}

pub fn passes(f: Filter, class: Filter) -> bool {
    match f {
        Filter::All => true,
        Filter::Misc => matches!(class, Filter::Misc | Filter::Useable | Filter::Datapads | Filter::Quests),
        other => other == class,
    }
}

pub fn item_row(v: &ItemView) -> super::ListItem {
    super::ListItem::text(v.name.clone()).with_icon(v.icon.clone()).with_count(u32::try_from(v.stack).ok())
}

pub fn describe(v: &ItemView) -> String {
    if v.description.is_empty() { v.name.clone() } else { format!("{}\n\n{}", v.name, v.description) }
}

/// Opens screens the world asked for: container loot, stores, the
/// workbench / lab station.
#[allow(clippy::too_many_arguments)]
fn screen_requests(
    mut commands: Commands,
    events: Res<FrameEvents>,
    game: Res<GameRes>,
    mut world: ResMut<WorldRes>,
    mut st: ResMut<ScreenState>,
    mut open: ResMut<OpenMenu>,
    existing: Query<Entity, With<MenuScreen>>,
) {
    for e in &events.0 {
        match e {
            WorldEvent::ContainerOpened { container, .. } => {
                if world.0.conversation.is_some() {
                    st.container_held = Some(*container);
                } else {
                    st.container = Some(*container);
                    open_menu(&mut commands, &game, &mut open, &existing, "container");
                }
            }
            WorldEvent::ConversationEnded { aborted, .. } => {
                // An aborted talk, or a container that is gone or no longer the open
                // one, opens nothing. A hold that opens nothing still leaves the world's
                // open container set, which blocks saving, so it is closed in the world.
                if let Some(container) = st.container_held.take() {
                    if !*aborted && world.0.is_valid(container) && world.0.inventory.open_container == Some(container) {
                        st.container = Some(container);
                        open_menu(&mut commands, &game, &mut open, &existing, "container");
                    } else if world.0.inventory.open_container == Some(container) {
                        let leader = world.0.party_leader();
                        world.0.close_container(container, leader);
                    }
                }
            }
            WorldEvent::ModuleEntered { .. } => st.container_held = None,
            WorldEvent::OpenStore { store } => {
                st.store = Some(*store);
                st.store_sell = false;
                st.store_filter = Filter::All;
                open_menu(&mut commands, &game, &mut open, &existing, "store");
            }
            WorldEvent::UpgradeScreen { item, character, item_creation, recipes } => {
                let station = world.0.craft.screen.as_ref().map_or(Station::Workbench, |s| s.station);
                st.craft = Some(CraftUi {
                    station,
                    character: *character,
                    recipes: recipes.clone(),
                    item_creation: *item_creation,
                    upgrade: true,
                    item: *item,
                    slot: None,
                    kind: -1,
                    creating: true,
                    group: 0,
                    status: String::new(),
                });
                let base = if item.is_some() { "upgrade" } else { "upgradesel" };
                open_menu(&mut commands, &game, &mut open, &existing, base);
            }
            WorldEvent::ItemCreateScreen { station, character, recipes } => {
                st.craft = Some(CraftUi {
                    station: *station,
                    character: *character,
                    recipes: recipes.clone(),
                    item_creation: true,
                    upgrade: false,
                    item: None,
                    slot: None,
                    kind: -1,
                    creating: true,
                    group: 0,
                    status: String::new(),
                });
                let base = if *station == Station::LabStation { "chemical" } else { "component" };
                open_menu(&mut commands, &game, &mut open, &existing, base);
            }
            _ => {}
        }
    }
    let _ = &mut world;
}

/// Tells the world when a container / store / craft screen went away
/// (Close, Esc, or another screen replacing it).
fn track_close(open: Res<OpenMenu>, mut world: ResMut<WorldRes>, mut st: ResMut<ScreenState>, mut sfx: MessageWriter<UiSfx>) {
    if open.0 == st.last_open {
        return;
    }
    sfx.write(UiSfx(if open.0.is_some() { "Fullscreen_Panel_Added" } else { "Fullscreen_Panel_Removed" }));
    let was = st.last_open.take();
    st.last_open = open.0.clone();
    st.status = None;
    let now = open.0.as_deref().unwrap_or("");
    let w = &mut world.0;
    match was.as_deref() {
        Some("container") if now != "container" => {
            if let Some(c) = st.container.take() {
                let by = w.party_leader();
                w.close_container(c, by);
            }
        }
        Some("store") if now != "store" => {
            st.store = None;
            w.close_store();
        }
        Some("upgradesel" | "upgrade" | "component" | "chemical") if !matches!(now, "upgradesel" | "upgrade" | "component" | "chemical") => {
            st.craft = None;
            w.close_craft_screen();
        }
        _ => {}
    }
    if open.0.is_none() {
        st.equip_slot = None;
        st.equip_hover = None;
    }
}

/// Test hooks, fired once 60 frames into the game:
/// `K2_GUI_USE=TAG` makes the leader use that object (containers, stores
/// behind dialogs); `K2_GUI_SCRIPT=NAME` runs a script as the leader
/// (k_workbench, k_medstation); `K2_GUI_STORE=TAG` opens that store;
/// `K2_GUI_CREDITS=N`, `K2_GUI_JOURNAL=plot:state,...`,
/// `K2_GUI_GLOBAL=name:number,...` seed the party and globals.
fn env_tests(mut world: ResMut<WorldRes>, mut frames: Local<u32>) {
    *frames += 1;
    if *frames != 60 {
        return;
    }
    let w = &mut world.0;
    let leader = w.party_leader();
    if let Some(n) = std::env::var("K2_GUI_CREDITS").ok().and_then(|v| v.parse().ok()) {
        w.add_credits(n);
    }
    if let Ok(list) = std::env::var("K2_GUI_GLOBAL") {
        for spec in list.split(',') {
            if let Some((name, v)) = spec.split_once(':') {
                w.globals.set_number(name, v.parse().unwrap_or(0));
            }
        }
    }
    if let Ok(list) = std::env::var("K2_GUI_JOURNAL") {
        for spec in list.split(',') {
            if let Some((plot, state)) = spec.split_once(':') {
                w.journal.insert(plot.to_ascii_lowercase(), state.parse().unwrap_or(1));
            }
        }
    }
    if let Ok(tag) = std::env::var("K2_GUI_USE") {
        match w.by_tag(&tag, 0) {
            Some(t) => w.use_object(leader, t),
            None => warn!("K2_GUI_USE: no object tagged {tag}"),
        }
    }
    if let Ok(tag) = std::env::var("K2_GUI_STORE") {
        match w.by_tag(&tag, 0) {
            Some(t) => w.open_store(t, leader, 0, 0),
            None => warn!("K2_GUI_STORE: no object tagged {tag}"),
        }
    }
    if let Ok(name) = std::env::var("K2_GUI_SCRIPT") {
        w.run_script(&name, leader, k2_world::ScriptCtx { last_used_by: leader, ..Default::default() });
        w.pump();
    }
}

/// Test hook: `K2_GUI_CLICKS=screen:TAG[#row],...` plays one synthetic
/// click every 30 frames from frame 120 (a row also selects it), so the
/// screens can be driven in screenshot runs without a mouse.
fn env_clicks(
    mut frames: Local<u32>,
    mut clicks: MessageWriter<super::GuiClick>,
    mut screens: Query<&mut super::GuiScreen>,
) {
    *frames += 1;
    let Ok(list) = std::env::var("K2_GUI_CLICKS") else { return };
    if *frames < 120 || (*frames - 120) % 30 != 0 {
        return;
    }
    let n = ((*frames - 120) / 30) as usize;
    let Some(spec) = list.split(',').nth(n) else { return };
    let Some((screen, rest)) = spec.split_once(':') else { return };
    let (tag, item) = match rest.split_once('#') {
        Some((t, r)) => (t, r.parse::<usize>().ok()),
        None => (rest, None),
    };
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with(screen)) else {
        warn!("K2_GUI_CLICKS: no screen {screen}");
        return;
    };
    if item.is_some() {
        scr.set_list_selected(tag, item);
    }
    info!("test click {} {tag} {item:?}", scr.name);
    clicks.write(super::GuiClick { screen: scr.name.clone(), tag: tag.to_owned(), item });
}
