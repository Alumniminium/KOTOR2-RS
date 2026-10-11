//! Pazaak screens: wager (pazaakwager), side deck (pazaaksetup) and the
//! table (pazaakgame), driving k2_world::pazaak. Opened by PlayPazaak
//! (WorldEvent::ShowGui("pazaak")); the result goes back through
//! World::pazaak_finish, which runs the end script.
//!
//! The screens count as an open menu (the world pauses; Esc forfeits).
//!
//! Test hooks: `K2_MG_SCRIPT=name[:param]` runs a script as the leader 90
//! frames in; `K2_PAZAAK_AUTO=1` lets the AI play the player's side too.

use bevy::prelude::*;
use k2_world::pazaak::{Card, Pazaak, Phase};

use crate::game::{AppState, FrameEvents, GameSet, WorldRes};
use crate::gui::menus::{MenuScreen, OpenMenu};
use crate::gui::{GuiClick, GuiScreen, GuiSet, Placement, open_screen};
use crate::model::GameRes;
use crate::uisfx::UiSfx;

pub struct PazaakPlugin;

impl Plugin for PazaakPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PazaakUi>()
            .add_systems(Update, (open_on_request, env_script).in_set(GameSet::Ui))
            .add_systems(Update, (clicks, drive, draw).chain().in_set(GuiSet::Logic).run_if(in_state(AppState::Game)));
    }
}

const MENU: &str = "pazaak";
/// Seconds between opponent actions (real time: the world is paused).
const AI_DELAY: f32 = 0.9;

/// Opponent pacing; auto (test) runs go faster.
fn ai_delay() -> f32 {
    if std::env::var_os("K2_PAZAAK_AUTO").is_some() { 0.2 } else { AI_DELAY }
}
const RESULT_HOLD: f32 = 2.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    Idle,
    Wager,
    Setup,
    Game,
}

#[derive(Resource, Default)]
pub struct PazaakUi {
    stage: Stage,
    screen: Option<Entity>,
    wager: i32,
    max_wager: i32,
    /// Cards available to build the side deck from, with counts.
    pool: Vec<(Card, i32)>,
    chosen: Vec<Card>,
    game: Option<Pazaak>,
    /// Per hand slot: index into Card::choices.
    choice: [usize; 4],
    ai_wait: f32,
    /// Seconds left showing the final result.
    done_wait: Option<(bool, f32)>,
    opponent_name: String,
    player_name: String,
}

fn open(commands: &mut Commands, ui: &mut PazaakUi, game: &GameRes, base: &str) {
    if let Some(e) = ui.screen.take() {
        commands.entity(e).try_despawn();
    }
    if let Some(e) = open_screen(commands, &game.0, base, Placement::Center, 150) {
        commands.entity(e).insert((MenuScreen, BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7))));
        ui.screen = Some(e);
    } else {
        warn!("pazaak: no layout {base}");
    }
}

#[allow(clippy::too_many_arguments)]
fn open_on_request(
    mut commands: Commands,
    events: Res<FrameEvents>,
    game: Res<GameRes>,
    mut world: ResMut<WorldRes>,
    mut ui: ResMut<PazaakUi>,
    mut menu: ResMut<OpenMenu>,
) {
    if !events.0.iter().any(|e| matches!(e, k2_world::WorldEvent::ShowGui(g) if g == "pazaak")) {
        return;
    }
    let w = &mut world.0;
    let Some(req) = w.pazaak.clone() else { return };
    ui.max_wager = if req.tutorial { 0 } else { req.max_wager.min(w.credits()).max(0) };
    ui.wager = ui.max_wager.min(10.max(ui.max_wager / 4));
    ui.opponent_name = w.get(req.opponent).map(|o| w.substitute(&o.name)).filter(|n| !n.is_empty()).unwrap_or_else(|| "Opponent".into());
    ui.player_name = w.get(w.player).map(|o| w.substitute(&o.name)).filter(|n| !n.is_empty()).unwrap_or_else(|| "Player".into());
    // Pool: owned cards, topped up with the starter cards so a fresh
    // character can still build ten.
    let mut pool = w.pazaak_owned_cards();
    let owned: i32 = pool.iter().map(|(_, n)| n).sum();
    if owned < 10 {
        for c in w.pazaak_opponent_deck(0) {
            match pool.iter_mut().find(|(k, _)| *k == c) {
                Some(e) => e.1 += 1,
                None => pool.push((c, 1)),
            }
        }
    }
    ui.pool = pool;
    ui.chosen = w.pazaak_default_side_deck();
    ui.game = None;
    ui.done_wait = None;
    ui.choice = [0; 4];
    menu.0 = Some(MENU.into());
    if ui.max_wager > 0 {
        ui.stage = Stage::Wager;
        open(&mut commands, &mut ui, &game, "pazaakwager");
    } else {
        ui.stage = Stage::Setup;
        open(&mut commands, &mut ui, &game, "pazaaksetup");
    }
}

/// `K2_MG_SCRIPT=name[:param]`: runs a script as the leader once.
fn env_script(mut world: ResMut<WorldRes>, mut frames: Local<u32>) {
    *frames += 1;
    if *frames != 90 {
        return;
    }
    let Ok(spec) = std::env::var("K2_MG_SCRIPT") else { return };
    let (name, param) = spec.split_once(':').map_or((spec.as_str(), 0), |(n, p)| (n, p.parse().unwrap_or(0)));
    let w = &mut world.0;
    let leader = w.party_leader();
    w.run_script(name, leader, k2_world::ScriptCtx { params: [param, 0, 0, 0, 0], last_used_by: leader, ..Default::default() });
    w.pump();
}

/// Setup grid slot (BTN_AVAILrc) for a card.
fn avail_tag(c: Card) -> String {
    let (r, col) = match c {
        Card::Plus(n) => (0, n - 1),
        Card::Minus(n) => (1, n - 1),
        Card::PlusMinus(n) => (2, n - 1),
        Card::OneOrTwo => (3, 0),
        Card::Double => (3, 1),
        Card::TieBreaker => (3, 2),
        Card::Flip24 => (3, 3),
        Card::Flip36 => (3, 4),
    };
    format!("{r}{col}")
}

fn card_at(rc: &str) -> Option<Card> {
    let b = rc.as_bytes();
    let (r, c) = ((b.first()? - b'0') as i8, (b.get(1)? - b'0') as i8);
    Some(match r {
        0 => Card::Plus(c + 1),
        1 => Card::Minus(c + 1),
        2 => Card::PlusMinus(c + 1),
        _ => match c {
            0 => Card::OneOrTwo,
            1 => Card::Double,
            2 => Card::TieBreaker,
            3 => Card::Flip24,
            4 => Card::Flip36,
            _ => return None,
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn clicks(
    mut commands: Commands,
    mut clicks: MessageReader<GuiClick>,
    game: Res<GameRes>,
    mut world: ResMut<WorldRes>,
    mut ui: ResMut<PazaakUi>,
    mut menu: ResMut<OpenMenu>,
    mut sfx: MessageWriter<UiSfx>,
) {
    if ui.stage == Stage::Idle {
        clicks.clear();
        return;
    }
    // Esc (or another menu) closed us: forfeit.
    if menu.0.as_deref() != Some(MENU) {
        finish(&mut commands, &mut ui, &mut world, &mut menu, false);
        return;
    }
    for c in clicks.read() {
        let tag = c.tag.as_str();
        match ui.stage {
            Stage::Wager => match tag {
                "BTN_LESS" => ui.wager = (ui.wager - 10.max(ui.max_wager / 20)).max(1.min(ui.max_wager)),
                "BTN_MORE" => ui.wager = (ui.wager + 10.max(ui.max_wager / 20)).min(ui.max_wager),
                "BTN_WAGER" => {
                    ui.stage = Stage::Setup;
                    open(&mut commands, &mut ui, &game, "pazaaksetup");
                }
                "BTN_QUIT" => finish(&mut commands, &mut ui, &mut world, &mut menu, false),
                _ => {}
            },
            Stage::Setup => {
                if let Some(rc) = tag.strip_prefix("BTN_AVAIL")
                    && let Some(card) = card_at(rc)
                {
                    let have = ui.pool.iter().find(|(k, _)| *k == card).map_or(0, |e| e.1);
                    let used = ui.chosen.iter().filter(|k| **k == card).count() as i32;
                    if ui.chosen.len() < 10 && used < have {
                        ui.chosen.push(card);
                    } else {
                        sfx.write(UiSfx("Error_Default"));
                    }
                } else if let Some(i) = tag.strip_prefix("BTN_CHOSEN").and_then(|n| n.parse::<usize>().ok()) {
                    if i < ui.chosen.len() {
                        ui.chosen.remove(i);
                    }
                } else if tag == "BTN_CLEARCARDS" {
                    ui.chosen.clear();
                } else if tag == "BTN_ATEXT" {
                    let total: i32 = ui.pool.iter().map(|(_, n)| n).sum();
                    if ui.chosen.len() == 10 || ui.chosen.len() as i32 == total {
                        let deck = ui.chosen.clone();
                        ui.game = world.0.pazaak_start(deck);
                        ui.stage = Stage::Game;
                        ui.ai_wait = ai_delay();
                        open(&mut commands, &mut ui, &game, "pazaakgame");
                    } else {
                        sfx.write(UiSfx("Error_Default"));
                    }
                }
            }
            Stage::Game => {
                let Some(g) = ui.game.as_mut() else { continue };
                match tag {
                    "BTN_FORFEITGAME" => {
                        finish(&mut commands, &mut ui, &mut world, &mut menu, false);
                        return;
                    }
                    "BTN_XTEXT" => match g.phase {
                        Phase::Turn if g.turn == 0 => {
                            let _ = g.end_turn(0);
                            ui.ai_wait = ai_delay();
                        }
                        Phase::SetOver { .. } => {
                            g.next_set();
                            ui.ai_wait = ai_delay();
                        }
                        _ => {}
                    },
                    "BTN_YTEXT" => {
                        if g.phase == Phase::Turn && g.turn == 0 {
                            let _ = g.stand(0);
                            ui.ai_wait = ai_delay();
                        }
                    }
                    _ => {
                        if let Some(i) = tag.strip_prefix("BTN_PLRSIDE").and_then(|n| n.parse::<usize>().ok()) {
                            let choice = ui.choice.get(i).copied().unwrap_or(0);
                            let g = ui.game.as_mut().unwrap();
                            if g.play(0, i, choice).is_err() {
                                sfx.write(UiSfx("Error_Default"));
                            }
                        } else if let Some(i) = tag.strip_prefix("BTN_FLIP").and_then(|n| n.parse::<usize>().ok()) {
                            // Flip the sign of a +/- card in hand.
                            if let Some(Some(card)) = ui.game.as_ref().and_then(|g| g.sides[0].hand.get(i).copied()) {
                                let n = card.choices().len();
                                if n > 1 && i < 4 {
                                    ui.choice[i] = match card {
                                        Card::OneOrTwo => (ui.choice[i] + 2) % 4,
                                        _ => (ui.choice[i] + 1) % n,
                                    };
                                }
                            }
                        } else if let Some(i) = tag.strip_prefix("BTN_CHANGE").and_then(|n| n.parse::<usize>().ok())
                            && i < 4
                            && matches!(ui.game.as_ref().and_then(|g| g.sides[0].hand.get(i).copied()), Some(Some(Card::OneOrTwo)))
                        {
                            // Value change: 1 <-> 2 keeping the sign.
                            ui.choice[i] ^= 1;
                        }
                    }
                }
            }
            Stage::Idle => {}
        }
    }
}

fn finish(commands: &mut Commands, ui: &mut PazaakUi, world: &mut WorldRes, menu: &mut OpenMenu, won: bool) {
    if let Some(e) = ui.screen.take() {
        commands.entity(e).try_despawn();
    }
    let wager = if ui.game.is_some() { ui.wager } else { 0 };
    ui.stage = Stage::Idle;
    ui.game = None;
    ui.done_wait = None;
    if menu.0.as_deref() == Some(MENU) {
        menu.0 = None;
    }
    let before = world.0.credits();
    world.0.pazaak_finish(won, wager);
    info!(
        "pazaak: {} (wager {wager}), credits {before} -> {}, GetLastPazaakResult {}",
        if won { "won" } else { "lost" },
        world.0.credits(),
        world.0.last_pazaak
    );
}

/// Opponent turns, the auto player, and the end-of-match hold.
fn drive(mut commands: Commands, time: Res<Time<Real>>, game: Res<GameRes>, mut world: ResMut<WorldRes>, mut ui: ResMut<PazaakUi>, mut menu: ResMut<OpenMenu>) {
    let dt = time.delta_secs();
    let auto = std::env::var_os("K2_PAZAAK_AUTO").is_some();
    // Auto mode clicks through the wager and deck screens after a moment.
    if auto && matches!(ui.stage, Stage::Wager | Stage::Setup) {
        ui.ai_wait -= dt;
        if ui.ai_wait <= -2.0 {
            ui.ai_wait = ai_delay();
            if ui.stage == Stage::Wager {
                ui.stage = Stage::Setup;
                open(&mut commands, &mut ui, &game, "pazaaksetup");
            } else {
                let deck = ui.chosen.clone();
                ui.game = world.0.pazaak_start(deck);
                ui.stage = Stage::Game;
                open(&mut commands, &mut ui, &game, "pazaakgame");
            }
        }
        return;
    }
    if ui.stage != Stage::Game {
        return;
    }
    if let Some((won, left)) = ui.done_wait {
        let left = left - dt;
        if left <= 0.0 {
            finish(&mut commands, &mut ui, &mut world, &mut menu, won);
        } else {
            ui.done_wait = Some((won, left));
        }
        return;
    }
    ui.ai_wait -= dt;
    let wait_over = ui.ai_wait <= 0.0;
    let Some(g) = ui.game.as_mut() else { return };
    match g.phase {
        Phase::MatchOver { winner } => ui.done_wait = Some((winner == 0, RESULT_HOLD)),
        Phase::SetOver { .. } if auto && wait_over => {
            g.next_set();
            ui.ai_wait = ai_delay();
        }
        Phase::Turn if wait_over && (g.turn == 1 || auto) => {
            let p = g.turn;
            g.ai_turn(p);
            ui.ai_wait = ai_delay();
        }
        _ => {}
    }
}

fn card_fill(c: Card, choice: usize) -> &'static str {
    match c {
        Card::Plus(_) => "pcards_pos_p",
        Card::Minus(_) => "pcards_neg_p",
        Card::PlusMinus(_) => {
            if choice % 2 == 0 {
                "pcards_dblpos_p"
            } else {
                "pcards_dblneg_p"
            }
        }
        _ => "pcards_gold_p",
    }
}

fn hand_label(c: Card, choice: usize) -> String {
    match c {
        Card::PlusMinus(n) => format!("{}{n}", if choice % 2 == 0 { "+" } else { "-" }),
        Card::OneOrTwo | Card::TieBreaker => {
            let v = c.choices().get(choice).copied().unwrap_or(1);
            format!("{v:+}{}", if c == Card::TieBreaker { "T" } else { "" })
        }
        other => other.label(),
    }
}

fn draw(ui: Res<PazaakUi>, mut screens: Query<&mut GuiScreen>) {
    let Some(mut scr) = ui.screen.and_then(|e| screens.get_mut(e).ok()) else { return };
    match ui.stage {
        Stage::Wager => {
            scr.set_text("LBL_TITLE", "Pazaak Wager");
            scr.set_text("LBL_MAXIMUM", format!("Maximum: {}", ui.max_wager));
            scr.set_text("LBL_WAGERVAL", ui.wager.to_string());
            scr.set_text("BTN_WAGER", "Wager");
            scr.set_text("BTN_QUIT", "Quit");
        }
        Stage::Setup => {
            scr.set_text("LBL_TITLE", "Choose Your Side Deck");
            scr.set_text("LBL_HELP", "Pick 10 cards. Four are dealt at random into your hand for the match.");
            scr.set_text("BTN_ATEXT", "Play");
            scr.set_text("BTN_CLEARCARDS", "Clear");
            for r in 0..4 {
                for c in 0..6 {
                    let rc = format!("{r}{c}");
                    let Some(card) = card_at(&rc) else {
                        scr.set_visible(&format!("BTN_AVAIL{rc}"), false);
                        scr.set_visible(&format!("LBL_AVAIL{rc}"), false);
                        scr.set_visible(&format!("LBL_AVAILNUM{rc}"), false);
                        continue;
                    };
                    let have = ui.pool.iter().find(|(k, _)| *k == card).map_or(0, |e| e.1);
                    let left = have - ui.chosen.iter().filter(|k| **k == card).count() as i32;
                    scr.set_text(&format!("LBL_AVAIL{rc}"), card.label());
                    scr.set_text(&format!("LBL_AVAILNUM{rc}"), if have > 0 { left.to_string() } else { String::new() });
                    scr.set_enabled(&format!("BTN_AVAIL{rc}"), left > 0);
                    scr.set_fill(&format!("BTN_AVAIL{rc}"), Some(card_fill(card, 0).into()));
                    debug_assert_eq!(avail_tag(card), rc);
                }
            }
            for i in 0..10 {
                let (b, l) = (format!("BTN_CHOSEN{i}"), format!("LBL_CHOSEN{i}"));
                match ui.chosen.get(i) {
                    Some(c) => {
                        scr.set_fill(&b, Some(card_fill(*c, 0).into()));
                        scr.set_text(&l, c.label());
                    }
                    None => {
                        scr.set_fill(&b, Some("pcards_back_p".into()));
                        scr.set_text(&l, "");
                    }
                }
            }
        }
        Stage::Game => {
            let Some(g) = ui.game.as_ref() else { return };
            scr.set_text("LBL_PLRNAME", ui.player_name.clone());
            scr.set_text("LBL_NPCNAME", ui.opponent_name.clone());
            for (p, who) in [(0usize, "PLR"), (1, "NPC")] {
                let s = &g.sides[p];
                scr.set_text(&format!("LBL_{who}TOTAL"), s.total().to_string());
                for i in 0..9 {
                    let (b, l) = (format!("BTN_{who}{i}"), format!("LBL_{who}{i}"));
                    match s.table.get(i) {
                        Some(tc) => {
                            scr.set_visible(&b, true);
                            let fill = match tc.side {
                                None => "pcards_generic_p",
                                Some(Card::Double | Card::TieBreaker | Card::Flip24 | Card::Flip36 | Card::OneOrTwo) => "pcards_gold_p",
                                Some(_) if tc.value < 0 => "pcards_neg_p",
                                Some(_) => "pcards_pos_p",
                            };
                            scr.set_fill(&b, Some(fill.into()));
                            let text = match tc.side {
                                None => tc.value.to_string(),
                                Some(Card::Flip24 | Card::Flip36) => tc.side.unwrap().label(),
                                Some(_) => format!("{:+}", tc.value),
                            };
                            scr.set_text(&l, text);
                        }
                        None => {
                            scr.set_visible(&b, false);
                            scr.set_text(&l, "");
                        }
                    }
                }
                for k in 0..3 {
                    scr.set_fill(&format!("LBL_{who}SCORE{k}"), Some(if s.sets_won as usize > k { "pz_rounds_on" } else { "pz_rounds_off" }.into()));
                }
                let lit = g.phase == Phase::Turn && g.turn == p;
                scr.set_fill(&format!("LBL_{who}TURN"), Some(if lit { "pz_playerliteon" } else { "pz_playerliteoff" }.into()));
                for i in 0..4 {
                    let (b, l) = (format!("BTN_{who}SIDE{i}"), format!("LBL_{who}SIDE{i}"));
                    match s.hand.get(i).copied().flatten() {
                        Some(c) if p == 0 => {
                            scr.set_visible(&b, true);
                            scr.set_fill(&b, Some(card_fill(c, ui.choice[i]).into()));
                            scr.set_text(&l, hand_label(c, ui.choice[i]));
                            scr.set_visible(&format!("BTN_FLIP{i}"), c.choices().len() > 1);
                            scr.set_visible(&format!("BTN_CHANGE{i}"), c == Card::OneOrTwo);
                        }
                        Some(_) => {
                            scr.set_visible(&b, true);
                            scr.set_text(&l, "");
                        }
                        None => {
                            scr.set_visible(&b, false);
                            scr.set_text(&l, "");
                            if p == 0 {
                                scr.set_visible(&format!("BTN_FLIP{i}"), false);
                                scr.set_visible(&format!("BTN_CHANGE{i}"), false);
                            }
                        }
                    }
                }
            }
            scr.set_text("LBL_FLIPLEGEND", "Flip sign");
            scr.set_text("LBL_CHANGELEGEND", "Change 1/2");
            scr.set_text("BTN_FORFEITGAME", "Forfeit");
            let (x, y) = match (g.phase, ui.done_wait) {
                (_, Some((won, _))) => ("".to_string(), if won { "You win the match!".to_string() } else { "You lose the match.".to_string() }),
                (Phase::SetOver { winner }, _) => (
                    "Next Set".into(),
                    match winner {
                        Some(0) => "You win the set".into(),
                        Some(_) => format!("{} wins the set", ui.opponent_name),
                        None => "Set tied".into(),
                    },
                ),
                (Phase::Turn, _) if g.turn == 0 => ("End Turn".into(), "Stand".into()),
                _ => ("".into(), format!("{}'s turn", ui.opponent_name)),
            };
            scr.set_text("BTN_XTEXT", x);
            scr.set_text("BTN_YTEXT", y);
        }
        Stage::Idle => {}
    }
}
