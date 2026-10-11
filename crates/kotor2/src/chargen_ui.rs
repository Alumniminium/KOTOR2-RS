//! Character generation on New Game, with K2's own screens:
//! classsel_p (class + gender) -> qorcpnl_p (Quick or Custom) -> the step
//! panel quickpnl_p / custpnl_p over maincg_p (the character summary), whose
//! steps open portcust_p, abchrgen_p, skchrgen_p, ftchrgen_p (+ pwrlvlup_p
//! when level 1 grants Force powers) and name_p. Play hands the result to
//! the world as its `PlayerSpec` and starts the prologue.
//!
//! The rules live in k2-world (chargen.rs); this file is the screens.
//! `--quickstart` (and `--newgame`) skip all of it with the Quick defaults.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use k2_world::PlayerSpec;
use k2_world::chargen::{BASE_SCORE, CLASS_BUTTONS, MAX_SCORE, POINT_BUY, PortraitOption, point_cost, points_spent};
use k2_world::craft::SKILL_COUNT;
use k2_world::progression::{LevelUpOptions, ability_mod};

use crate::game::{AppState, Fade, NeedAreaLoad, WorldRes};
use crate::gui::menus::MainMenu;
use crate::gui::{GuiClick, GuiScreen, GuiSet, ListItem, Placement, open_screen};
use crate::model::GameRes;

pub struct ChargenPlugin;

impl Plugin for ChargenPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChargenUi>()
            .add_systems(OnExit(AppState::Menu), close_all)
            .add_systems(Update, (chargen_flow, name_entry).chain().in_set(GuiSet::Logic).run_if(in_state(AppState::Menu)));
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Step {
    #[default]
    ClassSel,
    QuickOrCustom,
    Panel,
    Portrait,
    Attributes,
    Skills,
    Feats,
    Powers,
    Name,
}

/// Steps of the Custom panel (custpnl_p BTN_STEPNAME1..5; 6 is Play).
const CUSTOM_STEPS: [Step; 5] = [Step::Portrait, Step::Attributes, Step::Skills, Step::Feats, Step::Name];
/// Steps of the Quick panel (quickpnl_p BTN_STEPNAME1..2; 3 is Play).
const QUICK_STEPS: [Step; 2] = [Step::Portrait, Step::Name];

const ABILITY_TAGS: [&str; 6] = ["STR", "DEX", "CON", "INT", "WIS", "CHA"];
/// dialog.tlk descriptions of the six abilities.
const ABILITY_DESC: [u32; 6] = [222, 223, 224, 226, 225, 227];

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

/// The character being made, plus which screen shows.
#[derive(Resource, Default)]
pub struct ChargenUi {
    /// Set by the main menu's New Game.
    pub request: bool,
    active: bool,
    step: Step,
    show: Option<Step>,
    quick: bool,
    gender: i32,
    class: i32,
    portraits: Vec<PortraitOption>,
    portrait: usize,
    abilities: [u8; 6],
    skills: [i32; SKILL_COUNT],
    feats: Vec<u16>,
    powers: Vec<i32>,
    name: String,
    /// Completed Custom steps (index into CUSTOM_STEPS), or Quick steps.
    done: [bool; 5],
    /// Values when the current sub-screen opened (Back restores them).
    saved: Option<Box<Saved>>,
    opts: Option<LevelUpOptions>,
    /// Ability/skill the description pane shows.
    focus: usize,
    sel: Option<usize>,
}

#[derive(Clone)]
struct Saved {
    portrait: usize,
    abilities: [u8; 6],
    skills: [i32; SKILL_COUNT],
    feats: Vec<u16>,
    powers: Vec<i32>,
    name: String,
}

impl ChargenUi {
    fn snapshot(&self) -> Saved {
        Saved {
            portrait: self.portrait,
            abilities: self.abilities,
            skills: self.skills,
            feats: self.feats.clone(),
            powers: self.powers.clone(),
            name: self.name.clone(),
        }
    }

    fn restore(&mut self) {
        if let Some(s) = self.saved.take() {
            self.portrait = s.portrait;
            self.abilities = s.abilities;
            self.skills = s.skills;
            self.feats = s.feats;
            self.powers = s.powers;
            self.name = s.name;
        }
    }

    fn portrait_opt(&self) -> Option<&PortraitOption> {
        self.portraits.get(self.portrait)
    }

    fn spec(&self) -> PlayerSpec {
        PlayerSpec {
            appearance: self.portrait_opt().map_or(0, |p| k2_world::World::appearance_for(p, self.class)),
            gender: self.gender,
            name: self.name.trim().to_owned(),
            class: Some(self.class),
            portrait: self.portrait_opt().map(|p| p.row),
            abilities: Some(self.abilities),
            skills: Some(self.skills),
            feats: self.feats.clone(),
            powers: self.powers.clone(),
        }
    }

    fn steps(&self) -> &'static [Step] {
        if self.quick { &QUICK_STEPS } else { &CUSTOM_STEPS }
    }

    fn panel(&self) -> &'static str {
        if self.quick { "quickpnl_p" } else { "custpnl_p" }
    }

    fn all_done(&self) -> bool {
        self.done[..self.steps().len()].iter().all(|d| *d)
    }

    /// Picks the class (and gender) and resets everything after it.
    fn choose_class(&mut self, w: &mut k2_world::World, gender: i32, class: i32) {
        self.gender = gender;
        self.class = class;
        self.portraits = w.pc_portraits(gender);
        self.portrait = 0;
        self.abilities = w.class_default_abilities(class);
        let q = w.quick_spec(gender, class, self.portraits.first());
        self.skills = q.skills.unwrap_or([0; SKILL_COUNT]);
        self.feats = q.feats;
        self.powers = q.powers;
        self.name = w.random_name(gender);
        self.done = [false; 5];
        self.opts = None;
    }

    /// Level 1 options for the current abilities (INT changes skill points).
    fn refresh_opts(&mut self, w: &mut k2_world::World) {
        self.opts = w.chargen_options(&self.spec());
    }
}

/// Screens of the chargen.
#[derive(Component)]
struct ChargenScreen;

fn close_all(mut commands: Commands, mut ui: ResMut<ChargenUi>, q: Query<Entity, With<ChargenScreen>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
    *ui = ChargenUi::default();
}

fn tlk(w: &k2_world::World, strref: u32) -> String {
    w.talk.as_ref().and_then(|t| t.get(strref)).unwrap_or("").to_owned()
}

fn open(commands: &mut Commands, game: &GameRes, name: &str, z: i32, backdrop: bool) {
    if let Some(e) = open_screen(commands, &game.0, name, Placement::Center, z) {
        commands.entity(e).insert(ChargenScreen);
        if backdrop {
            commands.entity(e).insert(BackgroundColor(Color::BLACK));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn chargen_flow(
    mut commands: Commands,
    game: Res<GameRes>,
    mut world: ResMut<WorldRes>,
    mut ui: ResMut<ChargenUi>,
    mut clicks: MessageReader<GuiClick>,
    open_screens: Query<Entity, With<ChargenScreen>>,
    mut screens: Query<&mut GuiScreen, With<ChargenScreen>>,
    mut menu: Query<&mut Visibility, With<MainMenu>>,
    mut need: ResMut<NeedAreaLoad>,
    mut fade: ResMut<Fade>,
    mut next: ResMut<NextState<AppState>>,
) {
    let w = &mut world.0;
    if ui.request {
        ui.request = false;
        ui.active = true;
        let (g, c) = CLASS_BUTTONS[2];
        ui.choose_class(w, g, c);
        ui.show = Some(Step::ClassSel);
        for mut v in &mut menu {
            *v = Visibility::Hidden;
        }
    }
    if !ui.active {
        return;
    }
    if let Some(step) = ui.show.take() {
        for e in &open_screens {
            commands.entity(e).despawn();
        }
        match step {
            Step::ClassSel => open(&mut commands, &game, "classsel_p", 210, true),
            Step::QuickOrCustom => {
                open(&mut commands, &game, "maincg_p", 210, true);
                open(&mut commands, &game, "qorcpnl_p", 211, false);
            }
            Step::Panel => {
                open(&mut commands, &game, "maincg_p", 210, true);
                let panel = ui.panel();
                open(&mut commands, &game, panel, 211, false);
            }
            Step::Portrait => open(&mut commands, &game, "portcust_p", 210, true),
            Step::Attributes => open(&mut commands, &game, "abchrgen_p", 210, true),
            Step::Skills => open(&mut commands, &game, "skchrgen_p", 210, true),
            Step::Feats => open(&mut commands, &game, "ftchrgen_p", 210, true),
            Step::Powers => open(&mut commands, &game, "pwrlvlup_p", 210, true),
            Step::Name => {
                open(&mut commands, &game, "maincg_p", 210, true);
                open(&mut commands, &game, "name_p", 211, false);
            }
        }
        if !matches!(step, Step::ClassSel | Step::QuickOrCustom | Step::Panel) {
            ui.saved = Some(Box::new(ui.snapshot()));
        }
        if matches!(step, Step::Skills | Step::Feats | Step::Powers | Step::Panel | Step::QuickOrCustom) {
            ui.refresh_opts(w);
        }
        ui.step = step;
        ui.sel = None;
        ui.focus = 0;
        clicks.clear();
        return;
    }

    let mut go: Option<Step> = None;
    let mut cancel = false;
    let mut play = false;
    for c in clicks.read() {
        let tag = c.tag.as_str();
        let on = |s: &str| c.screen.starts_with(s);
        match ui.step {
            Step::ClassSel if on("classsel") => {
                if let Some(i) = tag.strip_prefix("BTN_SEL").and_then(|n| n.parse::<usize>().ok()).filter(|i| (1..=6).contains(i)) {
                    let (g, cl) = CLASS_BUTTONS[i - 1];
                    ui.choose_class(w, g, cl);
                    go = Some(Step::QuickOrCustom);
                } else if tag == "BTN_BACK" {
                    cancel = true;
                }
            }
            Step::QuickOrCustom if on("qorcpnl") => match tag {
                "QUICK_CHAR_BTN" => {
                    ui.quick = true;
                    let (g, cl) = (ui.gender, ui.class);
                    ui.choose_class(w, g, cl);
                    ui.quick = true;
                    go = Some(Step::Panel);
                }
                "CUST_CHAR_BTN" => {
                    ui.quick = false;
                    ui.done = [false; 5];
                    go = Some(Step::Panel);
                }
                "BTN_BACK" => go = Some(Step::ClassSel),
                _ => {}
            },
            Step::Panel if on(ui.panel()) => {
                let n = ui.steps().len();
                if let Some(i) = tag.strip_prefix("BTN_STEPNAME").and_then(|n| n.parse::<usize>().ok()) {
                    let i = i - 1;
                    if i < n && ui.done[..i].iter().all(|d| *d) {
                        go = Some(ui.steps()[i]);
                    } else if i == n && ui.all_done() {
                        play = true;
                    }
                }
                match tag {
                    "BTN_BACK" => go = Some(Step::QuickOrCustom),
                    "BTN_CANCEL" => cancel = true,
                    _ => {}
                }
            }
            Step::Portrait if on("portcust") => match tag {
                "BTN_ARRL" => {
                    let n = ui.portraits.len().max(1);
                    ui.portrait = (ui.portrait + n - 1) % n;
                }
                "BTN_ARRR" => {
                    let n = ui.portraits.len().max(1);
                    ui.portrait = (ui.portrait + 1) % n;
                }
                "BTN_ACCEPT" => {
                    mark_done(&mut ui, Step::Portrait);
                    go = Some(Step::Panel);
                }
                "BTN_BACK" => {
                    ui.restore();
                    go = Some(Step::Panel);
                }
                _ => {}
            },
            Step::Attributes if on("abchrgen") => {
                let left = POINT_BUY - points_spent(&ui.abilities);
                for (i, a) in ABILITY_TAGS.iter().enumerate() {
                    let v = ui.abilities[i];
                    if tag == format!("{a}_PLUS_BTN") {
                        ui.focus = i;
                        if v < MAX_SCORE && left >= point_cost(v) {
                            ui.abilities[i] += 1;
                        }
                    } else if tag == format!("{a}_MINUS_BTN") {
                        ui.focus = i;
                        if v > BASE_SCORE {
                            ui.abilities[i] -= 1;
                        }
                    } else if tag == format!("{a}_POINTS_BTN") || tag == format!("{a}_LBL") {
                        ui.focus = i;
                    }
                }
                match tag {
                    "BTN_RECOMMENDED" => ui.abilities = w.class_default_abilities(ui.class),
                    "BTN_ACCEPT" => {
                        let changed = ui.saved.as_ref().is_some_and(|s| s.abilities != ui.abilities);
                        mark_done(&mut ui, Step::Attributes);
                        if changed {
                            // Skill points and feat prerequisites follow the scores.
                            ui.skills = [0; SKILL_COUNT];
                            ui.feats.clear();
                            ui.powers.clear();
                            ui.done[2] = false;
                            ui.done[3] = false;
                        }
                        ui.saved = None;
                        go = Some(Step::Panel);
                    }
                    "BTN_BACK" => {
                        ui.restore();
                        go = Some(Step::Panel);
                    }
                    _ => {}
                }
            }
            Step::Skills if on("skchrgen") => {
                let Some(o) = ui.opts.clone() else { continue };
                let spent: i32 = o.skills.iter().map(|s| ui.skills[s.id] * s.cost).sum();
                let left = o.skill_points - spent;
                for (s, (points, pre)) in SKILL_TAGS.iter().enumerate() {
                    let sc = &o.skills[s];
                    if tag == format!("{pre}_PLUS_BTN") {
                        ui.focus = s;
                        if left >= sc.cost && ui.skills[s] < sc.max_rank {
                            ui.skills[s] += 1;
                        }
                    } else if tag == format!("{pre}_MINUS_BTN") {
                        ui.focus = s;
                        if ui.skills[s] > 0 {
                            ui.skills[s] -= 1;
                        }
                    } else if tag == *points {
                        ui.focus = s;
                    }
                }
                match tag {
                    "BTN_RECOMMENDED" => ui.skills = w.auto_choice(&o).skills,
                    "BTN_ACCEPT" => {
                        mark_done(&mut ui, Step::Skills);
                        go = Some(Step::Panel);
                    }
                    "BTN_BACK" => {
                        ui.restore();
                        go = Some(Step::Panel);
                    }
                    _ => {}
                }
            }
            Step::Feats | Step::Powers if on(if ui.step == Step::Feats { "ftchrgen" } else { "pwrlvlup" }) => {
                let Some(o) = ui.opts.clone() else { continue };
                let feats = ui.step == Step::Feats;
                let list = if feats { "LB_FEATS" } else { "LB_POWERS" };
                if tag == list
                    && let Some(i) = c.item
                {
                    ui.sel = Some(i);
                }
                let (select, accept, back, recommended) =
                    if feats { ("BTN_SELECT", "BTN_ACCEPT", "BTN_BACK", "BTN_RECOMMENDED") } else { ("SELECT_BTN", "ACCEPT_BTN", "BACK_BTN", "RECOMMENDED_BTN") };
                if tag == select
                    && let Some(i) = ui.sel
                {
                    if feats {
                        if let Some(f) = o.feats.get(i).map(|f| f.id as u16) {
                            if let Some(p) = ui.feats.iter().position(|&x| x == f) {
                                ui.feats.remove(p);
                            } else if (ui.feats.len() as i32) < o.feat_picks {
                                ui.feats.push(f);
                            }
                        }
                    } else if let Some(p) = o.powers.get(i).map(|p| p.id) {
                        if let Some(k) = ui.powers.iter().position(|&x| x == p) {
                            ui.powers.remove(k);
                        } else if (ui.powers.len() as i32) < o.power_picks {
                            ui.powers.push(p);
                        }
                    }
                } else if tag == recommended {
                    let auto = w.auto_choice(&o);
                    if feats {
                        ui.feats = auto.feats;
                    } else {
                        ui.powers = auto.powers;
                    }
                } else if tag == accept {
                    if feats && o.power_picks > 0 && !o.powers.is_empty() {
                        // K2 Jedi pick their first powers right after the feats.
                        let saved = ui.saved.clone();
                        go = Some(Step::Powers);
                        ui.saved = saved;
                    } else {
                        mark_done(&mut ui, Step::Feats);
                        go = Some(Step::Panel);
                    }
                } else if tag == back {
                    ui.restore();
                    go = Some(Step::Panel);
                }
            }
            Step::Name if on("name_p") => match tag {
                "BTN_RANDOM" => {
                    let g = ui.gender;
                    ui.name = w.random_name(g);
                }
                "END_BTN" if !ui.name.trim().is_empty() => {
                    mark_done(&mut ui, Step::Name);
                    go = Some(Step::Panel);
                }
                "BTN_BACK" => {
                    ui.restore();
                    go = Some(Step::Panel);
                }
                _ => {}
            },
            _ => {}
        }
    }
    if cancel {
        for e in &open_screens {
            commands.entity(e).despawn();
        }
        *ui = ChargenUi::default();
        for mut v in &mut menu {
            *v = Visibility::Inherited;
        }
        return;
    }
    if play {
        let spec = ui.spec();
        match w.check_spec(&spec) {
            Ok(()) => {
                info!("chargen: {} {:?} class {} portrait {:?} abilities {:?} skills {:?} feats {:?} powers {:?}", spec.name, spec.gender, spec.class.unwrap_or(-1), spec.portrait, spec.abilities, spec.skills, spec.feats, spec.powers);
                w.player_spec = spec;
                for e in &open_screens {
                    commands.entity(e).despawn();
                }
                *ui = ChargenUi::default();
                crate::menu::start_new_game(&mut world, &mut need, &mut fade, &mut next);
            }
            Err(e) => warn!("chargen: {e}"),
        }
        return;
    }
    if let Some(s) = go {
        // Powers continue the feats step (its snapshot covers both).
        if s == Step::Powers {
            let saved = ui.saved.take();
            ui.show = Some(s);
            ui.saved = saved;
        } else {
            ui.show = Some(s);
        }
        return;
    }
    refresh(&mut ui, w, &mut screens);
}

fn mark_done(ui: &mut ChargenUi, step: Step) {
    if let Some(i) = ui.steps().iter().position(|&s| s == step) {
        ui.done[i] = true;
    }
    if step != Step::Attributes {
        ui.saved = None;
    }
}

/// Mirrors the character into whichever screens are open.
fn refresh(ui: &mut ChargenUi, w: &mut k2_world::World, screens: &mut Query<&mut GuiScreen, With<ChargenScreen>>) {
    let portrait = ui.portrait_opt().map(|p| p.resref.clone());
    let (class_name, class_desc) = w.class_info(ui.class);
    for mut scr in screens.iter_mut() {
        let name = scr.name.clone();
        if name.starts_with("classsel") {
            for (i, (g, c)) in CLASS_BUTTONS.iter().enumerate() {
                let ports = w.pc_portraits(*g);
                let pick = ports.get(i % ports.len().max(1)).map(|p| p.resref.clone());
                scr.set_fill(&format!("BTN_SEL{}", i + 1), pick);
                let _ = c;
            }
            for i in 1..=6 {
                scr.set_visible(&format!("3D_MODEL{i}"), false);
            }
            scr.set_text("LBL_CLASS", class_name.clone());
            let summary = k2_world::World::class_summary_strref(ui.class).map(|s| tlk(w, s)).unwrap_or_default();
            scr.set_text("LBL_DESC", summary);
        } else if name.starts_with("qorcpnl") {
            // Quick / Custom explanations (dialog.tlk 241 / 242), then the class.
            scr.set_text("LB_DESC", format!("{}\n\n{}\n\n{class_name}\n{class_desc}", tlk(w, 241), tlk(w, 242)));
        } else if name.starts_with("maincg") {
            scr.set_fill("PORTRAIT_LBL", portrait.clone());
            scr.set_visible("MODEL_LBL", false);
            scr.set_text("LBL_NAME", ui.name.clone());
            scr.set_text("LBL_CLASS", class_name.clone());
            scr.set_text("LBL_LEVEL_VAL", "1");
            for (i, a) in ABILITY_TAGS.iter().enumerate() {
                scr.set_text(&format!("{a}_AB_LBL"), ui.abilities[i].to_string());
            }
            let m = |i: usize| ability_mod(i32::from(ui.abilities[i]));
            let hp = ui.opts.as_ref().map_or(0, |o| o.hp_gain);
            scr.set_text("LBL_VIT", hp.to_string());
            scr.set_text("LBL_DEF", (10 + m(1)).to_string());
            scr.set_text("NEW_FORT_LBL", format!("{:+}", m(2)));
            scr.set_text("NEW_REFL_LBL", format!("{:+}", m(1)));
            scr.set_text("NEW_WILL_LBL", format!("{:+}", m(4)));
        } else if name.starts_with("quickpnl") || name.starts_with("custpnl") {
            let n = ui.steps().len();
            for i in 0..n {
                let reachable = ui.done[..i].iter().all(|d| *d);
                scr.set_enabled(&format!("BTN_STEPNAME{}", i + 1), reachable);
                scr.set_selected(&format!("LBL_{}", i + 1), ui.done[i]);
                scr.set_text(&format!("LBL_NUM{}", i + 1), (i + 1).to_string());
            }
            scr.set_enabled(&format!("BTN_STEPNAME{}", n + 1), ui.all_done());
            scr.set_text(&format!("LBL_NUM{}", n + 1), (n + 1).to_string());
        } else if name.starts_with("portcust") {
            scr.set_fill("LBL_PORTRAIT", portrait.clone());
            scr.set_visible("LBL_HEAD", false);
        } else if name.starts_with("abchrgen") {
            let left = POINT_BUY - points_spent(&ui.abilities);
            for (i, a) in ABILITY_TAGS.iter().enumerate() {
                let v = ui.abilities[i];
                scr.set_text(&format!("{a}_POINTS_BTN"), v.to_string());
                scr.set_text(&format!("LBL_BONUS_{a}"), format!("{:+}", ability_mod(i32::from(v))));
                scr.set_visible(&format!("{a}_MINUS_BTN"), v > BASE_SCORE);
                scr.set_visible(&format!("{a}_PLUS_BTN"), v < MAX_SCORE && left >= point_cost(v));
            }
            scr.set_text("REMAINING_SELECTIONS_LBL", left.to_string());
            scr.set_text("COST_POINTS_LBL", point_cost(ui.abilities[ui.focus.min(5)]).to_string());
            scr.set_text("LB_DESC", tlk(w, ABILITY_DESC[ui.focus.min(5)]));
        } else if name.starts_with("skchrgen") {
            let Some(o) = ui.opts.as_ref() else { continue };
            let spent: i32 = o.skills.iter().map(|s| ui.skills[s.id] * s.cost).sum();
            let left = o.skill_points - spent;
            for (s, (points, pre)) in SKILL_TAGS.iter().enumerate() {
                let sc = &o.skills[s];
                scr.set_text(points, ui.skills[s].to_string());
                scr.set_visible(&format!("{pre}_MINUS_BTN"), ui.skills[s] > 0);
                scr.set_visible(&format!("{pre}_PLUS_BTN"), left >= sc.cost && ui.skills[s] < sc.max_rank);
            }
            let sc = &o.skills[ui.focus.min(7)];
            scr.set_text("REMAINING_SELECTIONS_LBL", left.to_string());
            scr.set_text("COST_POINTS_LBL", sc.cost.to_string());
            scr.set_visible("CLASSSKL_LBL", sc.class_skill);
            let desc = w.tables.get(&w.res, "skills").ok().and_then(|t| t.int(sc.id, "description")).and_then(|s| u32::try_from(s).ok()).map(|s| tlk(w, s)).unwrap_or_default();
            scr.set_text(
                "LB_DESC",
                format!("{}\n\nRank {} (max {}){}\n\n{desc}", sc.name, ui.skills[sc.id], sc.max_rank, if sc.class_skill { ", class skill" } else { "" }),
            );
        } else if name.starts_with("ftchrgen") || name.starts_with("pwrlvlup") {
            // pwrlvlup_p is the level up's power screen; its title says so.
            scr.set_text("MAIN_TITLE_LBL", tlk(w, 208));
            let Some(o) = ui.opts.as_ref() else { continue };
            let feats = name.starts_with("ftchrgen");
            let (items, picks, chosen): (Vec<_>, i32, Vec<i32>) = if feats {
                // Granted feats first, marked, then the pickable ones.
                (o.feats.clone(), o.feat_picks, ui.feats.iter().map(|&f| i32::from(f)).collect())
            } else {
                (o.powers.clone(), o.power_picks, ui.powers.clone())
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
            let granted: Vec<String> = o.granted_feats.iter().map(|f| f.name.clone()).collect();
            let desc = match sel {
                Some(c) => c.description.clone(),
                None if feats && !granted.is_empty() => format!("Granted at level 1: {}", granted.join(", ")),
                None => String::new(),
            };
            scr.set_text("LB_DESC", desc);
        } else if name.starts_with("name_p") {
            scr.set_text("NAME_BOX_EDIT", format!("{}_", ui.name));
        }
    }
}

/// Typing the name on name_p (Enter accepts).
fn name_entry(mut ui: ResMut<ChargenUi>, mut keys_in: MessageReader<KeyboardInput>, mut clicks: MessageWriter<GuiClick>) {
    if !ui.active || ui.step != Step::Name || ui.show.is_some() {
        keys_in.clear();
        return;
    }
    for k in keys_in.read() {
        if !k.state.is_pressed() {
            continue;
        }
        match &k.logical_key {
            Key::Character(s) => {
                for ch in s.chars() {
                    if (ch.is_ascii_alphanumeric() || " '-".contains(ch)) && ui.name.len() < 24 {
                        ui.name.push(ch);
                    }
                }
            }
            Key::Space if ui.name.len() < 24 => ui.name.push(' '),
            Key::Backspace => {
                ui.name.pop();
            }
            Key::Enter => {
                clicks.write(GuiClick { screen: "name_p".into(), tag: "END_BTN".into(), item: None });
            }
            _ => {}
        }
    }
}

/// `--quickstart` / `--newgame`: the Quick Character defaults without the
/// screens (male Jedi Guardian, first portrait).
pub fn quick_default(w: &mut k2_world::World) {
    if w.player_spec.class.is_some() {
        return;
    }
    let appearance_override = w.player_spec.appearance;
    let spec = w.quick_spec(0, 3, None);
    let keep = std::env::args().any(|a| a == "--player-appearance");
    w.player_spec = spec;
    if keep {
        w.player_spec.appearance = appearance_override;
    }
}
