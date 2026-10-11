//! Level up and Force power UI.
//!
//! - Level up: the HUD shows `LBL_LEVELUPn` over party members with enough
//!   XP; the character screen's Level Up button opens K2's flow:
//!   `leveluppnl_p` (the step hub) and its steps `abchrgen_p` (attribute),
//!   `skchrgen_p` (skills), `ftchrgen_p` (feats) and `pwrlvlup_p` (Force
//!   powers). Finish applies it through `World::apply_level_up`.
//! - Action bar: the HUD's `BTN_ACTION0..3` slots (arrows cycle): 0 attack /
//!   combat feats on the target, 1 hostile Force powers, 2 Force powers on
//!   yourself, 3 items (medpacs on yourself, grenades on an enemy). Keys 1-4
//!   use a slot. A door or placeable under the cursor puts its own entries
//!   first: Open, Close or Use in slot 0, Unlock or Security in slot 1, Attack
//!   when it can be bashed in slot 2 (`object_entries`). A living friendly
//!   creature puts Talk in slot 0 (Use when it only has a dialogue script), a
//!   dead container Open, and hostiles nothing (`creature_entries`).
//! - Casting: SpellCastStarted plays the cast clip, SpellImpact flashes a
//!   light on the target.
//!
//! Test flags: `--levelup-test LEVEL[,STEP]` gives the PC the XP for LEVEL
//! and opens the level up (STEP 0 hub, 1-4 a step screen);
//! `--autocast SPELL` makes the leader cast SPELL at the nearest enemy (or
//! itself for a friendly power) once the area is up.

use bevy::prelude::*;
use k2_script::{ObjectId, OBJECT_INVALID};
use k2_world::powers::{TalentView, TALENT_FEAT, TALENT_SPELL};
use k2_world::progression::{LevelUpChoice, LevelUpOptions};
use k2_world::WorldEvent;

use crate::anim::AnimPlayer;
use crate::game::{AppState, ApplyEvents, CreatureAnim, FrameEvents, GameSet, InConversation, ObjBody, ObjEntities, ObjId, PendingUse, PendingVerb, WorldRes};
use crate::gui::hud::GameHud;
use crate::gui::menus::{MenuScreen, OpenMenu, close_menu};
use crate::gui::{GuiClick, GuiScreen, GuiSet, ListItem, Placement, open_screen};
use crate::hover::Target;
use crate::hud::LastTarget;
use crate::model::GameRes;

/// Flags taking one value.
pub const FLAGS: [&str; 2] = ["--levelup-test", "--autocast"];

fn arg_value(name: &str) -> Option<String> {
    let argv: Vec<String> = std::env::args().collect();
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1).cloned())
}

pub struct ProgressUiPlugin;

impl Plugin for ProgressUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LevelUpUi>()
            .init_resource::<ActionBar>()
            .add_systems(OnExit(AppState::Game), |mut ui: ResMut<LevelUpUi>| *ui = LevelUpUi::default())
            .add_systems(Update, (cast_visuals, fade_fx).chain().in_set(GameSet::Ui).after(ApplyEvents))
            .add_systems(
                Update,
                (levelup_flow, hud_levelup, action_bar, test_hooks)
                    .chain()
                    .after(GuiSet::Logic)
                    .before(GuiSet::Input)
                    .run_if(in_state(AppState::Game)),
            );
    }
}

// ---------------------------------------------------------------------------
// Level up
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Step {
    #[default]
    Hub,
    Attribute,
    Skills,
    Feats,
    Powers,
}

impl Step {
    fn screen(self) -> &'static str {
        match self {
            Step::Hub => "leveluppnl_p",
            Step::Attribute => "abchrgen_p",
            Step::Skills => "skchrgen_p",
            Step::Feats => "ftchrgen_p",
            Step::Powers => "pwrlvlup_p",
        }
    }
}

#[derive(Resource, Default)]
pub struct LevelUpUi {
    creature: Option<ObjectId>,
    opts: Option<LevelUpOptions>,
    choice: LevelUpChoice,
    /// Choice as it was when the current step opened (Back restores it).
    saved: LevelUpChoice,
    step: Step,
    /// Pending screen switch (applied next frame).
    show: Option<Step>,
    done: [bool; 4],
    sel: Option<usize>,
    /// Skill the description pane shows.
    skill: usize,
}

impl LevelUpUi {
    /// Whether the level-up screen is up (a creature is being levelled).
    pub(crate) fn is_open(&self) -> bool {
        self.creature.is_some()
    }
}

/// skchrgen_p controls per SKILL_* index: (points button, +/- prefix).
const SKILL_TAGS: [(&str, &str); 8] = [
    ("COMPUTER_USE_POINTS_BTN", "COM"),
    ("DEMOLITIONS_POINTS_BTN", "DEM"),
    ("STEALTH_POINTS_BTN", "STE"),
    ("AWARENESS_POINTS_BTN", "AWA"),
    ("PERSUADE_POINTS_BTN", "PER"),
    ("REPAIR_POINTS_BTN", "REP"),
    ("SECURITY_POINTS_BTN", "SEC"),
    ("TREAT_INJURY_POINTS_BTN", "TRE"),
];

const ABILITY_TAGS: [&str; 6] = ["STR", "DEX", "CON", "INT", "WIS", "CHA"];

#[derive(Component)]
struct LevelUpScreen;

/// Opens the level up for `id` (no-op without enough XP).
pub fn begin_level_up(ui: &mut LevelUpUi, w: &mut k2_world::World, id: ObjectId) -> bool {
    if !w.level_up_available(id) {
        return false;
    }
    let Some(opts) = w.level_up_options(id) else { return false };
    *ui = LevelUpUi {
        creature: Some(id),
        opts: Some(opts.clone()),
        show: Some(Step::Hub),
        // Steps with nothing to pick count as done.
        done: [opts.attribute_points == 0, false, opts.feat_picks == 0 || opts.feats.is_empty(), opts.power_picks == 0 || opts.powers.is_empty()],
        ..Default::default()
    };
    true
}

fn tlk(w: &k2_world::World, strref: Option<i32>) -> String {
    match (strref, &w.talk) {
        (Some(s), Some(t)) if s >= 0 => t.get(s as u32).unwrap_or("").to_owned(),
        _ => String::new(),
    }
}

fn skill_desc(w: &k2_world::World, skill: usize) -> String {
    let t = w.tables.get(&w.res, "skills").ok();
    tlk(w, t.and_then(|t| t.int(skill, "description")))
}

fn ability_desc(w: &k2_world::World, a: usize) -> String {
    // The chargen screen's descriptions: strrefs of the ability names.
    let names = ["Strength", "Dexterity", "Constitution", "Intelligence", "Wisdom", "Charisma"];
    let _ = w;
    format!("{}\n\nOne attribute point every four levels.", names[a.min(5)])
}

#[allow(clippy::too_many_arguments)]
fn levelup_flow(
    mut commands: Commands,
    game: Res<GameRes>,
    mut world: ResMut<WorldRes>,
    mut ui: ResMut<LevelUpUi>,
    mut open: ResMut<OpenMenu>,
    mut clicks: MessageReader<GuiClick>,
    existing: Query<Entity, With<MenuScreen>>,
    mut screens: Query<&mut GuiScreen, With<LevelUpScreen>>,
) {
    let Some(id) = ui.creature else {
        clicks.clear();
        return;
    };
    // Esc (or another menu) closed it.
    if ui.show.is_none() && open.0.as_deref() != Some("levelup") {
        *ui = LevelUpUi::default();
        return;
    }
    if let Some(step) = ui.show.take() {
        close_menu(&mut commands, &mut open, &existing);
        if let Some(e) = open_screen(&mut commands, &game.0, step.screen(), Placement::Center, 160) {
            commands.entity(e).insert((MenuScreen, LevelUpScreen, BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7))));
        }
        open.0 = Some("levelup".into());
        ui.step = step;
        ui.saved = ui.choice.clone();
        ui.sel = None;
        return;
    }
    let w = &mut world.0;
    let Some(o) = ui.opts.clone() else { return };
    let cur = ui.step.screen();
    let mut go: Option<Step> = None;
    for c in clicks.read() {
        if !c.screen.starts_with(cur) {
            continue;
        }
        let tag = c.tag.as_str();
        match ui.step {
            Step::Hub => match tag {
                "BTN_STEPNAME1" if o.attribute_points > 0 => go = Some(Step::Attribute),
                "BTN_STEPNAME2" => go = Some(Step::Skills),
                "BTN_STEPNAME3" if o.feat_picks > 0 && !o.feats.is_empty() => go = Some(Step::Feats),
                "BTN_STEPNAME4" if o.power_picks > 0 && !o.powers.is_empty() => go = Some(Step::Powers),
                "BTN_STEPNAME5" => {
                    let choice = ui.choice.clone();
                    match w.apply_level_up(id, &choice) {
                        Ok(()) => {
                            info!("levelup: {} is now level {}", w.describe(id), w.level_of(id));
                            close_menu(&mut commands, &mut open, &existing);
                            let mut next = LevelUpUi::default();
                            // Several levels banked: straight into the next.
                            if begin_level_up(&mut next, w, id) {
                                *ui = next;
                            } else {
                                *ui = LevelUpUi::default();
                            }
                            return;
                        }
                        Err(e) => warn!("levelup: {e}"),
                    }
                }
                "BTN_BACK" => {
                    close_menu(&mut commands, &mut open, &existing);
                    *ui = LevelUpUi::default();
                    return;
                }
                _ => {}
            },
            Step::Attribute => {
                let used = i32::from(ui.choice.attribute.is_some());
                for (i, a) in ABILITY_TAGS.iter().enumerate() {
                    if tag == format!("{a}_PLUS_BTN") && used < o.attribute_points {
                        ui.choice.attribute = Some(i);
                    } else if tag == format!("{a}_MINUS_BTN") && ui.choice.attribute == Some(i) {
                        ui.choice.attribute = None;
                    } else if tag == format!("{a}_POINTS_BTN") {
                        ui.skill = i;
                    }
                }
                match tag {
                    "BTN_RECOMMENDED" => ui.choice.attribute = Some(o.primary_ability),
                    "BTN_ACCEPT" => {
                        ui.done[0] = true;
                        go = Some(Step::Hub);
                    }
                    "BTN_BACK" => {
                        ui.choice = ui.saved.clone();
                        go = Some(Step::Hub);
                    }
                    _ => {}
                }
            }
            Step::Skills => {
                let spent: i32 = o.skills.iter().map(|s| ui.choice.skills[s.id] * s.cost).sum();
                let left = o.skill_points - spent;
                for (s, (points, pre)) in SKILL_TAGS.iter().enumerate() {
                    let sc = &o.skills[s];
                    if tag == format!("{pre}_PLUS_BTN") {
                        ui.skill = s;
                        if left >= sc.cost && sc.rank + ui.choice.skills[s] < sc.max_rank {
                            ui.choice.skills[s] += 1;
                        }
                    } else if tag == format!("{pre}_MINUS_BTN") {
                        ui.skill = s;
                        if ui.choice.skills[s] > 0 {
                            ui.choice.skills[s] -= 1;
                        }
                    } else if tag == *points {
                        ui.skill = s;
                    }
                }
                match tag {
                    "BTN_RECOMMENDED" => ui.choice.skills = w.auto_choice(&o).skills,
                    "BTN_ACCEPT" => {
                        ui.done[1] = true;
                        go = Some(Step::Hub);
                    }
                    "BTN_BACK" => {
                        ui.choice = ui.saved.clone();
                        go = Some(Step::Hub);
                    }
                    _ => {}
                }
            }
            Step::Feats | Step::Powers => {
                let feats = ui.step == Step::Feats;
                let list = if feats { "LB_FEATS" } else { "LB_POWERS" };
                if tag == list
                    && let Some(i) = c.item
                {
                    ui.sel = Some(i);
                }
                let select = if feats { "BTN_SELECT" } else { "SELECT_BTN" };
                let accept = if feats { "BTN_ACCEPT" } else { "ACCEPT_BTN" };
                let back = if feats { "BTN_BACK" } else { "BACK_BTN" };
                let recommended = if feats { "BTN_RECOMMENDED" } else { "RECOMMENDED_BTN" };
                if tag == select
                    && let Some(i) = ui.sel
                {
                    if feats {
                        if let Some(f) = o.feats.get(i).map(|f| f.id as u16) {
                            if let Some(p) = ui.choice.feats.iter().position(|&x| x == f) {
                                ui.choice.feats.remove(p);
                            } else if (ui.choice.feats.len() as i32) < o.feat_picks {
                                ui.choice.feats.push(f);
                            }
                        }
                    } else if let Some(p) = o.powers.get(i).map(|p| p.id) {
                        if let Some(k) = ui.choice.powers.iter().position(|&x| x == p) {
                            ui.choice.powers.remove(k);
                        } else if (ui.choice.powers.len() as i32) < o.power_picks {
                            ui.choice.powers.push(p);
                        }
                    }
                } else if tag == recommended {
                    let auto = w.auto_choice(&o);
                    if feats {
                        ui.choice.feats = auto.feats;
                    } else {
                        ui.choice.powers = auto.powers;
                    }
                } else if tag == accept {
                    ui.done[if feats { 2 } else { 3 }] = true;
                    go = Some(Step::Hub);
                } else if tag == back {
                    ui.choice = ui.saved.clone();
                    go = Some(Step::Hub);
                }
            }
        }
    }
    if let Some(s) = go {
        ui.show = Some(s);
        return;
    }
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with(cur)) else { return };
    match ui.step {
        Step::Hub => {
            let enabled = [o.attribute_points > 0, true, o.feat_picks > 0 && !o.feats.is_empty(), o.power_picks > 0 && !o.powers.is_empty()];
            for i in 0..4 {
                scr.set_enabled(&format!("BTN_STEPNAME{}", i + 1), enabled[i]);
                scr.set_selected(&format!("LBL_{}", i + 1), ui.done[i]);
            }
            scr.set_enabled("BTN_STEPNAME5", ui.done.iter().all(|d| *d));
        }
        Step::Attribute => {
            let c = w.get(id).and_then(|x| x.creature()).map(|c| c.abilities).unwrap_or([10; 6]);
            for (i, a) in ABILITY_TAGS.iter().enumerate() {
                let v = i32::from(c[i]) + i32::from(ui.choice.attribute == Some(i));
                scr.set_text(&format!("{a}_POINTS_BTN"), v.to_string());
                scr.set_text(&format!("LBL_BONUS_{a}"), format!("{:+}", k2_world::progression::ability_mod(v)));
            }
            let left = o.attribute_points - i32::from(ui.choice.attribute.is_some());
            scr.set_text("REMAINING_SELECTIONS_LBL", left.to_string());
            scr.set_text("COST_POINTS_LBL", "1");
            scr.set_text("LB_DESC", ability_desc(w, ui.choice.attribute.unwrap_or(ui.skill)));
        }
        Step::Skills => {
            let spent: i32 = o.skills.iter().map(|s| ui.choice.skills[s.id] * s.cost).sum();
            for (s, (points, _)) in SKILL_TAGS.iter().enumerate() {
                scr.set_text(points, (o.skills[s].rank + ui.choice.skills[s]).to_string());
            }
            let sc = &o.skills[ui.skill.min(7)];
            scr.set_text("REMAINING_SELECTIONS_LBL", (o.skill_points - spent).to_string());
            scr.set_text("COST_POINTS_LBL", sc.cost.to_string());
            scr.set_visible("CLASSSKL_LBL", sc.class_skill);
            scr.set_text("LB_DESC", format!("{}\n\nRank {} (max {}){}\n\n{}", sc.name, sc.rank + ui.choice.skills[sc.id], sc.max_rank, if sc.class_skill { ", class skill" } else { "" }, skill_desc(w, sc.id)));
        }
        Step::Feats | Step::Powers => {
            let feats = ui.step == Step::Feats;
            let (items, picks, chosen): (Vec<_>, i32, Vec<i32>) = if feats {
                (o.feats.clone(), o.feat_picks, ui.choice.feats.iter().map(|&f| i32::from(f)).collect())
            } else {
                (o.powers.clone(), o.power_picks, ui.choice.powers.clone())
            };
            let rows: Vec<ListItem> = items
                .iter()
                .map(|c| ListItem::text(if chosen.contains(&c.id) { format!("> {}", c.name) } else { c.name.clone() }).with_icon(c.icon.clone()))
                .collect();
            let list = if feats { "LB_FEATS" } else { "LB_POWERS" };
            scr.set_list(list, rows);
            scr.set_list_selected(list, ui.sel);
            let left = (picks - chosen.len() as i32).max(0).to_string();
            scr.set_text(if feats { "STD_REMAINING_SELECTIONS_LBL" } else { "REMAINING_SELECTIONS_LBL" }, left);
            let sel = ui.sel.and_then(|i| items.get(i));
            scr.set_text(if feats { "LBL_NAME" } else { "LBL_POWER" }, sel.map(|c| c.name.clone()).unwrap_or_default());
            scr.set_text("LB_DESC", sel.map(|c| c.description.clone()).unwrap_or_default());
        }
    }
}

/// HUD level up markers, and the character screen's Level Up button.
#[allow(clippy::too_many_arguments)]
fn hud_levelup(
    mut world: ResMut<WorldRes>,
    mut ui: ResMut<LevelUpUi>,
    open: Res<OpenMenu>,
    st: Res<crate::gui::screens::ScreenState>,
    mut clicks: MessageReader<GuiClick>,
    mut huds: Query<&mut GuiScreen, (With<GameHud>, Without<MenuScreen>)>,
    mut menus: Query<&mut GuiScreen, (With<MenuScreen>, Without<GameHud>)>,
) {
    let w = &mut world.0;
    let leader = w.party_leader();
    let mut party = vec![leader];
    party.extend(w.party.iter().copied().filter(|&p| p != leader));
    if let Ok(mut hud) = huds.single_mut() {
        for slot in 0..3 {
            let up = party.get(slot).is_some_and(|&id| w.level_up_available(id));
            hud.set_visible(&format!("LBL_LEVELUP{}", slot + 1), up);
        }
    }
    if open.0.as_deref() != Some("character") {
        clicks.clear();
        return;
    }
    let me = crate::gui::screens::member(w, &st);
    let available = w.level_up_available(me);
    if let Some(mut scr) = menus.iter_mut().find(|s| s.name.starts_with("character")) {
        scr.set_visible("BTN_LEVELUP", available);
    }
    for c in clicks.read() {
        if c.screen.starts_with("character") && c.tag == "BTN_LEVELUP" && available {
            begin_level_up(&mut ui, w, me);
        }
    }
}

// ---------------------------------------------------------------------------
// Action bar
// ---------------------------------------------------------------------------

const SLOTS: usize = 4;

/// One action bar entry. `ty` < 0 is a plain attack.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    ty: i32,
    id: i32,
    name: String,
    icon: Option<String>,
    cost: i32,
    on_target: bool,
}

#[derive(Resource, Default)]
pub struct ActionBar {
    slots: [Vec<Entry>; SLOTS],
    idx: [usize; SLOTS],
    target: Option<ObjectId>,
    /// The door or placeable under the cursor (or nearest in front).
    object: Option<ObjectId>,
    refresh_in: f32,
    desc: Option<(String, f32)>,
}

// Mines, traps and stealth (k2-world traps.rs / stealth.rs). Entry `ty`
// codes below -1; `id` carries the item or trap object.
const ENTRY_SET_MINE: i32 = -2;
const ENTRY_DISABLE_MINE: i32 = -3;
const ENTRY_RECOVER_MINE: i32 = -4;
const ENTRY_STEALTH: i32 = -5;
// Door and placeable quick actions (`id` is the object).
const ENTRY_USE_OBJECT: i32 = -6;
const ENTRY_UNLOCK_OBJECT: i32 = -7;
const ENTRY_BASH_OBJECT: i32 = -8;

/// Talk / Use or Open for a creature, in slot 0 (see `creature_entries`).
/// Hostile creatures get no entry here; the target's Attack covers them.
fn creature_entries(w: &mut k2_world::World, leader: ObjectId, obj: ObjectId, slots: &mut [Vec<Entry>; SLOTS]) {
    let Some(o) = w.get(obj) else { return };
    let Some(dead) = o.creature().map(|c| c.dead) else { return };
    let conversation = o.conversation.as_deref().is_some_and(|s| !s.is_empty());
    let dialogue = o.script("ScriptDialogue").is_some();
    let mk = |name: &str, icon: &str| Entry { ty: ENTRY_USE_OBJECT, id: obj as i32, name: name.into(), icon: Some(icon.into()), cost: 0, on_target: false };
    if dead {
        if w.is_container(obj) {
            slots[0].insert(0, mk("Open", "gui_mp_useu"));
        }
    } else if !w.is_enemy(leader, obj) {
        if conversation {
            slots[0].insert(0, mk("Talk", "gui_mp_talku"));
        } else if dialogue {
            slots[0].insert(0, mk("Use", "gui_mp_useu"));
        }
    }
}

/// Open / Use, Security and Attack for a door or placeable, one per slot
/// (0, 1, 2), ahead of whatever else those slots hold.
fn object_entries(w: &mut k2_world::World, leader: ObjectId, obj: ObjectId, slots: &mut [Vec<Entry>; SLOTS]) {
    if w.get(obj).is_some_and(|o| o.creature().is_some()) {
        return creature_entries(w, leader, obj, slots);
    }
    let Some(o) = w.get(obj) else { return };
    let (is_door, open, useable) = match (o.door(), o.placeable()) {
        (Some(d), _) => (true, d.open, !d.is_static),
        (_, Some(p)) => (false, p.open, p.useable),
        _ => return,
    };
    let container = w.is_container(obj);
    let info = w.lock_info(leader, obj);
    let locked = info.as_ref().is_some_and(|i| i.locked);
    let mk = |ty: i32, name: String, icon: &str| Entry { ty, id: obj as i32, name, icon: Some(icon.into()), cost: 0, on_target: false };
    if w.can_bash(obj) {
        slots[2].insert(0, mk(ENTRY_BASH_OBJECT, "Attack".into(), "i_attack"));
    }
    if let Some(i) = info.filter(|i| i.locked)
        && (i.has_key || (!i.key_required && i.can_pick))
    {
        let name = if i.has_key { "Unlock".to_owned() } else { format!("Security (DC {})", i.dc) };
        slots[1].insert(0, mk(ENTRY_UNLOCK_OBJECT, name, "isk_security"));
    }
    if useable && !locked {
        let name = match (is_door, container, open) {
            (true, _, true) => "Close",
            (true, _, false) | (_, true, _) => "Open",
            _ => "Use",
        };
        slots[0].insert(0, mk(ENTRY_USE_OBJECT, name.into(), "gui_mp_useu"));
    }
}

/// Set Mine (items slot) per mine kind in the party inventory; Disable /
/// Recover Mine (first slot) for a known trap within reach; the stealth
/// toggle when a field generator is equipped.
fn systems_entries(w: &mut k2_world::World, leader: ObjectId, slots: &mut [Vec<Entry>; SLOTS]) {
    let mut seen = std::collections::HashSet::new();
    for (item, _) in w.party_mines() {
        let Some(def) = w.item_def_of(item) else { continue };
        if !seen.insert(def.name.clone()) {
            continue;
        }
        let name = format!("{}: {}", w.talk_str(1530), def.name);
        slots[3].push(Entry { ty: ENTRY_SET_MINE, id: item as i32, name, icon: def.icon.clone(), cost: 0, on_target: false });
    }
    if let Some(pos) = w.get(leader).map(|o| o.position)
        && let Some(trap) = w.nearest_known_trap(pos, k2_world::traps::USE_RANGE)
    {
        slots[0].insert(0, Entry { ty: ENTRY_RECOVER_MINE, id: trap as i32, name: w.talk_str(1531), icon: Some("i_recovermine".into()), cost: 0, on_target: false });
        slots[0].insert(0, Entry { ty: ENTRY_DISABLE_MINE, id: trap as i32, name: w.talk_str(1529), icon: Some("i_disablemine".into()), cost: 0, on_target: false });
    }
    if w.has_stealth_unit(leader) {
        let name = if w.is_stealthed(leader) { "Exit Stealth".to_owned() } else { w.talk_str(247) };
        slots[0].push(Entry { ty: ENTRY_STEALTH, id: 0, name, icon: Some("isk_stealth".into()), cost: 0, on_target: false });
    }
}

fn use_systems_entry(w: &mut k2_world::World, leader: ObjectId, e: &Entry) {
    let r = match e.ty {
        ENTRY_SET_MINE => w.set_mine(leader, e.id as ObjectId).map(|_| ()),
        ENTRY_DISABLE_MINE => w.disarm_trap(leader, e.id as ObjectId, false).map(|_| ()),
        ENTRY_RECOVER_MINE => w.disarm_trap(leader, e.id as ObjectId, true).map(|_| ()),
        ENTRY_STEALTH => {
            let on = !w.is_stealthed(leader);
            w.set_stealth(leader, on)
        }
        _ => Ok(()),
    };
    match r {
        Ok(()) => info!("action: {}", e.name),
        Err(err) => info!("action: {} refused: {err}", e.name),
    }
}

fn entry(t: &TalentView, on_target: bool) -> Entry {
    Entry { ty: t.ty, id: t.id, name: t.name.clone(), icon: t.icon.clone(), cost: t.cost, on_target }
}

#[allow(clippy::too_many_arguments)]
fn action_bar(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut world: ResMut<WorldRes>,
    mut bar: ResMut<ActionBar>,
    conv: Res<InConversation>,
    open: Res<OpenMenu>,
    hovered: Res<Target>,
    last: Res<LastTarget>,
    ids: Query<&ObjId>,
    mut pending: ResMut<PendingUse>,
    mut clickwalk: ResMut<crate::play::ClickWalk>,
    mut verb: ResMut<PendingVerb>,
    mut clicks: MessageReader<GuiClick>,
    mut huds: Query<&mut GuiScreen, With<GameHud>>,
) {
    let Ok(mut hud) = huds.single_mut() else { return };
    let w = &mut world.0;
    let leader = w.party_leader();
    // Target: hovered enemy, else the creature being fought.
    let alive_enemy = |w: &mut k2_world::World, id: ObjectId| w.get(id).and_then(|o| o.creature()).is_some_and(|c| !c.dead) && w.is_enemy(leader, id);
    let hov = hovered.0.and_then(|e| ids.get(e).ok()).map(|i| i.0);
    let target = [hov, last.0].into_iter().flatten().find(|&id| alive_enemy(w, id));
    let object = hov.filter(|&id| id != leader && w.get(id).is_some_and(|o| o.door().is_some() || o.placeable().is_some() || o.creature().is_some()));
    bar.refresh_in -= time.delta_secs();
    if bar.refresh_in <= 0.0 || target != bar.target || object != bar.object {
        bar.refresh_in = 0.5;
        bar.target = target;
        bar.object = object;
        let mut slots: [Vec<Entry>; SLOTS] = Default::default();
        if let Some(t) = target {
            slots[0].push(Entry { ty: -1, id: 0, name: "Attack".into(), icon: Some("i_attack".into()), cost: 0, on_target: true });
            for v in w.usable_talents(leader, t) {
                let slot = if v.ty == TALENT_FEAT {
                    0
                } else if v.cost > 0 {
                    1
                } else {
                    3
                };
                slots[slot].push(entry(&v, true));
            }
        }
        for v in w.usable_talents(leader, leader) {
            if v.ty != TALENT_SPELL {
                continue;
            }
            let slot = if v.cost > 0 { 2 } else { 3 };
            if slot == 3 && target.is_some() && !slots[3].is_empty() && slots[3].iter().all(|e| e.on_target) {
                // Grenades first while fighting; healing still reachable.
            }
            slots[slot].push(entry(&v, false));
        }
        systems_entries(w, leader, &mut slots);
        if let Some(obj) = object {
            object_entries(w, leader, obj, &mut slots);
        }
        for (i, s) in slots.into_iter().enumerate() {
            if bar.slots[i] != s {
                debug!("action bar slot {i}: {:?}", s.iter().map(|e| e.name.as_str()).collect::<Vec<_>>());
                // Keep the selection on the same entry when the list moves.
                let keep = bar.slots[i].get(bar.idx[i]).and_then(|e| s.iter().position(|x| x.ty == e.ty && x.id == e.id));
                bar.idx[i] = keep.unwrap_or(0);
                bar.slots[i] = s;
            }
        }
    }
    let mut use_slot: Option<usize> = None;
    for c in clicks.read() {
        if !c.screen.starts_with("mipc") {
            continue;
        }
        for i in 0..SLOTS {
            let n = bar.slots[i].len();
            if c.tag == format!("BTN_ACTION{i}") {
                use_slot = Some(i);
            } else if n > 0 && c.tag == format!("BTN_ACTIONUP{i}") {
                bar.idx[i] = (bar.idx[i] + n - 1) % n;
                let name = bar.slots[i][bar.idx[i]].name.clone();
                bar.desc = Some((name, 2.5));
            } else if n > 0 && c.tag == format!("BTN_ACTIONDOWN{i}") {
                bar.idx[i] = (bar.idx[i] + 1) % n;
                let name = bar.slots[i][bar.idx[i]].name.clone();
                bar.desc = Some((name, 2.5));
            }
        }
    }
    if !conv.0 && open.0.is_none() {
        for (i, k) in [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4].into_iter().enumerate() {
            if keys.just_pressed(k) {
                use_slot = Some(i);
            }
        }
    }
    if let Some(i) = use_slot
        && let Some(e) = bar.slots[i].get(bar.idx[i]).cloned()
    {
        let tgt = if e.on_target { bar.target.unwrap_or(OBJECT_INVALID) } else { leader };
        if matches!(e.ty, ENTRY_USE_OBJECT | ENTRY_UNLOCK_OBJECT | ENTRY_BASH_OBJECT) {
            let obj = e.id as ObjectId;
            // A new walk starts afresh (see game.rs `interact`).
            clickwalk.begin(w);
            match e.ty {
                ENTRY_BASH_OBJECT => {
                    pending.0 = None;
                    w.player_attack(obj);
                }
                ENTRY_UNLOCK_OBJECT => {
                    pending.0 = Some(obj);
                    *verb = PendingVerb::Unlock;
                }
                _ => {
                    pending.0 = Some(obj);
                    *verb = PendingVerb::Use;
                }
            }
            info!("action: {} {}", e.name, w.describe(obj));
            bar.refresh_in = 0.0;
        } else if e.ty <= ENTRY_SET_MINE {
            use_systems_entry(w, leader, &e);
            bar.refresh_in = 0.0;
        } else if e.ty < 0 {
            if tgt != OBJECT_INVALID {
                w.player_attack(tgt);
            }
        } else if tgt != OBJECT_INVALID {
            w.use_talent(leader, e.ty, e.id, tgt);
            info!("action: {} uses {} on {}", w.describe(leader), e.name, w.describe(tgt));
        }
        let cost = if e.cost > 0 { format!(" ({} FP)", e.cost) } else { String::new() };
        bar.desc = Some((format!("{}{cost}", e.name), 2.0));
    }
    for i in 0..6 {
        let entries = bar.slots.get(i).cloned().unwrap_or_default();
        let cur = entries.get(bar.idx.get(i).copied().unwrap_or(0));
        hud.set_fill(&format!("LBL_ACTION{i}"), cur.and_then(|e| e.icon.clone()));
        hud.set_enabled(&format!("BTN_ACTION{i}"), cur.is_some());
        hud.set_visible(&format!("BTN_ACTIONUP{i}"), entries.len() > 1);
        hud.set_visible(&format!("BTN_ACTIONDOWN{i}"), entries.len() > 1);
    }
    let desc = bar.desc.as_mut().map(|(t, left)| {
        *left -= time.delta_secs();
        (t.clone(), *left > 0.0)
    });
    let show = desc.as_ref().is_some_and(|d| d.1) && !conv.0;
    hud.set_visible("LBL_ACTIONDESC", show);
    hud.set_visible("LBL_ACTIONDESCBG", show);
    if let Some((t, true)) = desc {
        hud.set_text("LBL_ACTIONDESC", t);
    } else if bar.desc.as_ref().is_some_and(|d| d.1 <= 0.0) {
        bar.desc = None;
    }
}

// ---------------------------------------------------------------------------
// Cast animations and impact flashes
// ---------------------------------------------------------------------------

#[derive(Component)]
struct Fx {
    left: f32,
    life: f32,
    intensity: f32,
}

fn cast_clips(anim: &str) -> &'static [&'static str] {
    match anim.to_ascii_lowercase().as_str() {
        "self" | "" => &["castout1", "castoutlp1"],
        "dark" => &["castout2", "castout1"],
        "up" => &["castout3", "castout1"],
        "throw" => &["throwsab", "throwgren", "castout1"],
        "monster" | "monsterfury" => &["ca1", "b0a1", "castout1"],
        "inject" | "gui_inject" => &["inject", "castout1"],
        _ => &["castout1"],
    }
}

#[allow(clippy::too_many_arguments)]
fn cast_visuals(
    mut commands: Commands,
    events: Res<FrameEvents>,
    ents: Res<ObjEntities>,
    mut roots: Query<(&ObjBody, Option<&mut CreatureAnim>)>,
    mut players: Query<&mut AnimPlayer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    world: Res<WorldRes>,
) {
    for ev in &events.0 {
        match ev {
            WorldEvent::SpellCastStarted { caster, anim, spell, .. } => {
                debug!("power: cast {spell} by {caster} ({anim})");
                let Some(&root) = ents.0.get(caster) else { continue };
                let Ok((body, ca)) = roots.get_mut(root) else { continue };
                let (Some(b), Some(mut a)) = (body.0, ca) else { continue };
                let Ok(mut p) = players.get_mut(b) else { continue };
                // A thrown grenade has its own clip; "throw" alone is the saber throw.
                let grenade = anim.eq_ignore_ascii_case("throw") && k2_world::projectiles::spell_projectile(&world.0, *spell).is_some_and(|s| s.arc);
                let names: &[&str] = if grenade { &["throwgren", "castout1"] } else { cast_clips(anim) };
                if let Some(clip) = names.iter().find(|n| p.set.has(n)) {
                    p.fade_to(clip, false, 1.0, 0.1);
                    a.scripted = Some(((*clip).to_owned(), false, None));
                }
            }
            WorldEvent::SpellImpact { target, hostile, success: true, spell, .. } => {
                debug!("power: impact {spell} on {target}");
                let Some(&root) = ents.0.get(target) else { continue };
                let color = if *hostile { Color::srgb(0.75, 0.3, 1.0) } else { Color::srgb(0.35, 0.7, 1.0) };
                let mesh = meshes.add(Sphere::new(0.35));
                let mat = mats.add(StandardMaterial { base_color: color.with_alpha(0.35), emissive: color.to_linear() * 6.0, alpha_mode: AlphaMode::Add, unlit: true, ..default() });
                // Object roots are in game space (Z up) under the world root.
                commands.entity(root).with_children(|c| {
                    c.spawn((
                        Name::new("power fx"),
                        Fx { left: 0.7, life: 0.7, intensity: 60_000.0 },
                        PointLight { color, intensity: 60_000.0, range: 6.0, shadow_maps_enabled: false, ..default() },
                        Mesh3d(mesh),
                        MeshMaterial3d(mat),
                        Transform::from_xyz(0.0, 0.0, 1.1),
                    ));
                });
            }
            _ => {}
        }
    }
}

fn fade_fx(mut commands: Commands, time: Res<Time>, mut q: Query<(Entity, &mut Fx, &mut PointLight, &mut Transform)>) {
    for (e, mut fx, mut light, mut tf) in &mut q {
        fx.left -= time.delta_secs();
        if fx.left <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        let k = fx.left / fx.life;
        light.intensity = fx.intensity * k;
        tf.scale = Vec3::splat(0.5 + 1.5 * (1.0 - k));
    }
}

// ---------------------------------------------------------------------------
// Test hooks
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Hooks {
    frames: u32,
    levelup: bool,
    cast: bool,
}

fn test_hooks(mut world: ResMut<WorldRes>, mut ui: ResMut<LevelUpUi>, mut state: Local<Hooks>, ents: Res<ObjEntities>) {
    state.frames += 1;
    // Wait until the area is populated (the leader has a Bevy entity).
    let w = &mut world.0;
    let leader = w.party_leader();
    if !ents.0.contains_key(&leader) || state.frames < 90 {
        return;
    }
    if !state.levelup
        && let Some(v) = arg_value("--levelup-test")
    {
        state.levelup = true;
        let mut it = v.split(',');
        let level: i32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(2);
        let step: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let xp = w.xp_for_level(level);
        w.give_xp(leader, xp);
        if begin_level_up(&mut ui, w, leader) {
            ui.show = Some(match step {
                1 => Step::Attribute,
                2 => Step::Skills,
                3 => Step::Feats,
                4 => Step::Powers,
                _ => Step::Hub,
            });
            info!("levelup-test: {} has {xp} XP, level up open", w.describe(leader));
        }
    }
    if !state.cast
        && let Some(v) = arg_value("--autocast")
    {
        state.cast = true;
        let spell: i32 = v.parse().unwrap_or(9);
        w.grant_power(leader, spell);
        if let Some(c) = w.get_mut(leader).and_then(|o| o.creature_mut()) {
            c.max_force = c.max_force.max(100);
            c.force = c.max_force;
        }
        let ppos = w.get(leader).map_or(Vec3::ZERO, |o| o.position);
        let mut foes: Vec<(ObjectId, f32)> = Vec::new();
        let ids: Vec<ObjectId> = w.iter().filter(|o| o.creature().is_some_and(|c| !c.dead) && o.id != leader).map(|o| o.id).collect();
        for id in ids {
            if w.is_enemy(leader, id) {
                foes.push((id, w.get(id).map_or(f32::MAX, |o| o.position.distance(ppos))));
            }
        }
        foes.sort_by(|a, b| a.1.total_cmp(&b.1));
        let hostile = w.tables.get(&w.res, "spells").ok().and_then(|t| t.str(spell as usize, "forcehostile").map(|_| ())).is_some();
        let target = if hostile { foes.first().map_or(leader, |f| f.0) } else { leader };
        // Screenshots want the cast, not the run there: stand 5 m off.
        if target != leader
            && let Some(tp) = w.get(target).map(|o| o.position)
        {
            let dir = (ppos - tp).truncate().normalize_or(Vec2::X);
            w.place(leader, tp + (dir * 5.0).extend(0.0));
        }
        w.use_talent(leader, TALENT_SPELL, spell, target);
        info!("autocast: {} casts {spell} at {}", w.describe(leader), w.describe(target));
    }
}

#[cfg(test)]
mod tests;
