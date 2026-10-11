//! Pazaak: the card game, its opponent AI, and the PlayPazaak /
//! GetLastPazaakResult glue.
//!
//! Rules (K2): a set is played to 20. Each turn the active player draws a
//! main-deck card (1-10) onto their table, may play one side card from their
//! hand of four, then ends the turn or stands. Over 20 at the end of a turn
//! busts; nine table cards without busting win the set outright. When both
//! stand the higher total wins; a tie is no-one's set unless exactly one
//! player played a Tie Breaker. First to three sets wins. The hand of four is
//! drawn at random from the ten-card side deck once per match and spent cards
//! are gone for the rest of the match.
//!
//! Side cards (pazaakdecks.2da codes / g_i_pazcard_NNN items):
//! `+N`/`-N`, `*N` (+/-N, the player picks the sign), `VV` (+/-1 or 2),
//! `$$` (Double: the last table card's value again), `TT` (Tie Breaker:
//! +/-1 and wins ties), `F1` (flip the sign of every 2 and 4 on your
//! table), `F2` (same for 3 and 6).

use k2_script::{ObjectId, OBJECT_INVALID};
use serde::{Deserialize, Serialize};

use crate::World;

pub const TARGET: i32 = 20;
pub const MAX_TABLE: usize = 9;
pub const SETS_TO_WIN: u8 = 3;
pub const HAND: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Card {
    Plus(i8),
    Minus(i8),
    PlusMinus(i8),
    /// +/-1 or +/-2 (K2 "Value Change").
    OneOrTwo,
    Double,
    TieBreaker,
    Flip24,
    Flip36,
}

impl Card {
    /// pazaakdecks.2da cell.
    pub fn from_code(code: &str) -> Option<Card> {
        let c = code.trim();
        let n = |s: &str| s.parse::<i8>().ok().filter(|v| (1..=6).contains(v));
        Some(match c {
            "$$" => Card::Double,
            "TT" => Card::TieBreaker,
            "F1" => Card::Flip24,
            "F2" => Card::Flip36,
            "VV" => Card::OneOrTwo,
            _ => match c.as_bytes().first()? {
                b'+' => Card::Plus(n(&c[1..])?),
                b'-' => Card::Minus(n(&c[1..])?),
                b'*' => Card::PlusMinus(n(&c[1..])?),
                _ => return None,
            },
        })
    }

    /// g_i_pazcard_NNN item number (1-23).
    pub fn from_item_number(n: u32) -> Option<Card> {
        Some(match n {
            1..=6 => Card::Plus(n as i8),
            7..=12 => Card::Minus((n - 6) as i8),
            13..=18 => Card::PlusMinus((n - 12) as i8),
            19 => Card::OneOrTwo,
            20 => Card::Double,
            21 => Card::TieBreaker,
            22 => Card::Flip24,
            23 => Card::Flip36,
            _ => return None,
        })
    }

    /// Item number for the g_i_pazcard_NNN template.
    pub fn item_number(self) -> u32 {
        match self {
            Card::Plus(n) => n as u32,
            Card::Minus(n) => 6 + n as u32,
            Card::PlusMinus(n) => 12 + n as u32,
            Card::OneOrTwo => 19,
            Card::Double => 20,
            Card::TieBreaker => 21,
            Card::Flip24 => 22,
            Card::Flip36 => 23,
        }
    }

    pub fn label(self) -> String {
        match self {
            Card::Plus(n) => format!("+{n}"),
            Card::Minus(n) => format!("-{n}"),
            Card::PlusMinus(n) => format!("+/-{n}"),
            Card::OneOrTwo => "+/-1/2".into(),
            Card::Double => "D".into(),
            Card::TieBreaker => "+/-1T".into(),
            Card::Flip24 => "2&4".into(),
            Card::Flip36 => "3&6".into(),
        }
    }

    /// Values this card can add when played (the player picks one). Flip
    /// and Double resolve against the table instead.
    pub fn choices(self) -> Vec<i32> {
        match self {
            Card::Plus(n) => vec![n as i32],
            Card::Minus(n) => vec![-(n as i32)],
            Card::PlusMinus(n) => vec![n as i32, -(n as i32)],
            Card::OneOrTwo => vec![1, 2, -1, -2],
            Card::TieBreaker => vec![1, -1],
            Card::Double | Card::Flip24 | Card::Flip36 => vec![0],
        }
    }

    /// Texture family for the GUI (pcards_*_p).
    pub fn color(self) -> &'static str {
        match self {
            Card::Plus(_) => "pos",
            Card::Minus(_) => "neg",
            Card::PlusMinus(_) => "dblpos",
            _ => "gold",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableCard {
    pub value: i32,
    /// None for main-deck cards.
    pub side: Option<Card>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Side {
    pub table: Vec<TableCard>,
    /// Four slots for the match; None once spent.
    pub hand: Vec<Option<Card>>,
    pub stood: bool,
    pub sets_won: u8,
    pub tie_breaker: bool,
}

impl Side {
    pub fn total(&self) -> i32 {
        self.table.iter().map(|c| c.value).sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    /// `turn` is acting: it has drawn and may play a card, end or stand.
    Turn,
    /// Set finished; `winner` None = tie. Call `next_set`.
    SetOver { winner: Option<usize> },
    MatchOver { winner: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayError {
    NotYourTurn,
    AlreadyPlayed,
    NoCard,
    BadChoice,
    Over,
}

/// A match between player 0 (the PC) and player 1 (the opponent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pazaak {
    pub sides: [Side; 2],
    deck: Vec<i32>,
    pub turn: usize,
    pub set: u32,
    pub played_this_turn: bool,
    pub phase: Phase,
    starter: usize,
    rng: u64,
    pub log: Vec<String>,
}

impl Pazaak {
    /// `side_decks` are each player's ten side-deck cards (fewer is
    /// allowed; the hand takes up to four).
    pub fn new(side_decks: [Vec<Card>; 2], seed: u64) -> Self {
        let mut g = Pazaak {
            sides: [Side::default(), Side::default()],
            deck: Vec::new(),
            turn: 0,
            set: 0,
            played_this_turn: false,
            phase: Phase::Turn,
            starter: 0,
            rng: seed | 1,
            log: Vec::new(),
        };
        for (p, deck) in side_decks.into_iter().enumerate() {
            let mut d = deck;
            g.shuffle(&mut d);
            g.sides[p].hand = d.into_iter().take(HAND).map(Some).collect();
        }
        g.start_set();
        g
    }

    fn next_rand(&mut self) -> u64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next_rand() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }

    fn start_set(&mut self) {
        self.set += 1;
        let mut deck: Vec<i32> = (1..=10).flat_map(|v| [v; 4]).collect();
        self.shuffle(&mut deck);
        self.deck = deck;
        for s in &mut self.sides {
            s.table.clear();
            s.stood = false;
            s.tie_breaker = false;
        }
        self.turn = self.starter;
        self.phase = Phase::Turn;
        self.begin_turn();
    }

    /// After `SetOver`: deal the next set. The loser of a decided set
    /// starts the next one; ties alternate.
    pub fn next_set(&mut self) {
        if let Phase::SetOver { winner } = self.phase {
            self.starter = match winner {
                Some(w) => 1 - w,
                None => 1 - self.starter,
            };
            self.start_set();
        }
    }

    fn draw(&mut self) -> i32 {
        if self.deck.is_empty() {
            let mut deck: Vec<i32> = (1..=10).flat_map(|v| [v; 4]).collect();
            self.shuffle(&mut deck);
            self.deck = deck;
        }
        self.deck.pop().unwrap_or(1)
    }

    fn begin_turn(&mut self) {
        self.played_this_turn = false;
        let p = self.turn;
        let v = self.draw();
        self.sides[p].table.push(TableCard { value: v, side: None });
        self.log.push(format!("set {} p{p} draws {v} -> {}", self.set, self.sides[p].total()));
        // Nine cards without busting wins the set at once.
        if self.sides[p].table.len() >= MAX_TABLE && self.sides[p].total() <= TARGET {
            self.finish_set(Some(p));
        }
    }

    /// Plays hand slot `slot` with `choice` (an index into `Card::choices`).
    pub fn play(&mut self, p: usize, slot: usize, choice: usize) -> Result<(), PlayError> {
        if self.phase != Phase::Turn {
            return Err(PlayError::Over);
        }
        if p != self.turn {
            return Err(PlayError::NotYourTurn);
        }
        if self.played_this_turn {
            return Err(PlayError::AlreadyPlayed);
        }
        let card = self.sides[p].hand.get(slot).copied().flatten().ok_or(PlayError::NoCard)?;
        let side = &mut self.sides[p];
        match card {
            Card::Double => {
                let last = side.table.last().map_or(0, |c| c.value);
                side.table.push(TableCard { value: last, side: Some(card) });
            }
            Card::Flip24 | Card::Flip36 => {
                let (a, b) = if card == Card::Flip24 { (2, 4) } else { (3, 6) };
                for c in &mut side.table {
                    if c.value.abs() == a || c.value.abs() == b {
                        c.value = -c.value;
                    }
                }
                side.table.push(TableCard { value: 0, side: Some(card) });
            }
            _ => {
                let v = *card.choices().get(choice).ok_or(PlayError::BadChoice)?;
                side.table.push(TableCard { value: v, side: Some(card) });
                if card == Card::TieBreaker {
                    side.tie_breaker = true;
                }
            }
        }
        side.hand[slot] = None;
        self.played_this_turn = true;
        self.log.push(format!("p{p} plays {} -> {}", card.label(), self.sides[p].total()));
        if self.sides[p].table.len() >= MAX_TABLE && self.sides[p].total() <= TARGET {
            self.finish_set(Some(p));
        }
        Ok(())
    }

    /// Ends the turn without standing.
    pub fn end_turn(&mut self, p: usize) -> Result<(), PlayError> {
        self.close_turn(p, false)
    }

    pub fn stand(&mut self, p: usize) -> Result<(), PlayError> {
        self.close_turn(p, true)
    }

    fn close_turn(&mut self, p: usize, stand: bool) -> Result<(), PlayError> {
        if self.phase != Phase::Turn {
            return Err(PlayError::Over);
        }
        if p != self.turn {
            return Err(PlayError::NotYourTurn);
        }
        if self.sides[p].total() > TARGET {
            self.log.push(format!("p{p} busts with {}", self.sides[p].total()));
            self.finish_set(Some(1 - p));
            return Ok(());
        }
        if stand {
            self.sides[p].stood = true;
            self.log.push(format!("p{p} stands on {}", self.sides[p].total()));
        }
        let o = 1 - p;
        if self.sides[0].stood && self.sides[1].stood {
            let (a, b) = (self.sides[0].total(), self.sides[1].total());
            let winner = match a.cmp(&b) {
                std::cmp::Ordering::Greater => Some(0),
                std::cmp::Ordering::Less => Some(1),
                std::cmp::Ordering::Equal => match (self.sides[0].tie_breaker, self.sides[1].tie_breaker) {
                    (true, false) => Some(0),
                    (false, true) => Some(1),
                    _ => None,
                },
            };
            self.finish_set(winner);
            return Ok(());
        }
        // The opponent keeps playing if it has not stood; otherwise we go on.
        self.turn = if self.sides[o].stood { p } else { o };
        self.begin_turn();
        Ok(())
    }

    fn finish_set(&mut self, winner: Option<usize>) {
        if let Some(w) = winner {
            self.sides[w].sets_won += 1;
            self.log.push(format!("set {} to p{w} ({} - {})", self.set, self.sides[0].total(), self.sides[1].total()));
            if self.sides[w].sets_won >= SETS_TO_WIN {
                self.phase = Phase::MatchOver { winner: w };
                self.log.push(format!("match to p{w}"));
                return;
            }
        } else {
            self.log.push(format!("set {} tied at {}", self.set, self.sides[0].total()));
        }
        self.phase = Phase::SetOver { winner };
    }

    /// Opponent AI: one decision for player `p` (play at most one card,
    /// then end or stand). Returns false when it isn't `p`'s turn.
    pub fn ai_turn(&mut self, p: usize) -> bool {
        if self.phase != Phase::Turn || self.turn != p {
            return false;
        }
        let o = 1 - p;
        let total = self.sides[p].total();
        let opp = &self.sides[o];
        let opp_total = opp.total();
        let opp_stood = opp.stood;

        // Best single card: lands closest to 20 without going over,
        // preferring exactly 20 and beating a standing opponent.
        let mut best: Option<(usize, usize, i32)> = None;
        if !self.played_this_turn {
            for (slot, c) in self.sides[p].hand.iter().enumerate() {
                let Some(c) = c else { continue };
                for (i, v) in c.choices().into_iter().enumerate() {
                    let nt = match c {
                        Card::Double => total + self.sides[p].table.last().map_or(0, |t| t.value),
                        Card::Flip24 | Card::Flip36 => {
                            let (a, b) = if *c == Card::Flip24 { (2, 4) } else { (3, 6) };
                            self.sides[p].table.iter().map(|t| if t.value.abs() == a || t.value.abs() == b { -t.value } else { t.value }).sum()
                        }
                        _ => total + v,
                    };
                    if nt > TARGET {
                        continue;
                    }
                    if best.is_none_or(|(_, _, b)| nt > b) {
                        best = Some((slot, i, nt));
                    }
                }
            }
        }
        let wants_card = |nt: i32| -> bool {
            if total > TARGET {
                return true; // anything that saves the set
            }
            if opp_stood && total < opp_total {
                return nt >= opp_total;
            }
            // Spend cards only for a strong finish.
            nt >= 19 && nt > total
        };
        if let Some((slot, i, nt)) = best
            && wants_card(nt)
        {
            let _ = self.play(p, slot, i);
            if self.phase != Phase::Turn {
                return true;
            }
        }
        let total = self.sides[p].total();
        let stand = total > TARGET // bust: close the turn, the set is lost
            || total >= 18
            || (opp_stood && total > opp_total)
            || (opp_stood && total == opp_total && total >= 17);
        if stand {
            let _ = self.stand(p);
        } else {
            let _ = self.end_turn(p);
        }
        true
    }
}

/// A pending PlayPazaak call, shown by the game UI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PazaakRequest {
    pub opponent_deck: i32,
    pub end_script: String,
    pub max_wager: i32,
    pub tutorial: bool,
    pub opponent: ObjectId,
    /// OBJECT_SELF of the PlayPazaak call; the end script runs as it.
    pub owner: ObjectId,
}

impl World {
    /// Opponent side deck: pazaakdecks.2da row.
    pub fn pazaak_opponent_deck(&self, row: i32) -> Vec<Card> {
        let Ok(t) = self.tables.get(&self.res, "pazaakdecks") else { return Vec::new() };
        let row = row.max(0) as usize;
        (0..10).filter_map(|i| t.str(row, &format!("card{i}")).and_then(Card::from_code)).collect()
    }

    /// Pazaak cards the party owns (g_i_pazcard_NNN items, with stacks).
    pub fn pazaak_owned_cards(&mut self) -> Vec<(Card, i32)> {
        let mut out: Vec<(Card, i32)> = Vec::new();
        for id in self.party_inventory() {
            let Some(v) = self.item_view(id) else { continue };
            let t = v.template.to_ascii_lowercase();
            let Some(n) = t.strip_prefix("g_i_pazcard_").and_then(|n| n.parse::<u32>().ok()) else { continue };
            let Some(c) = Card::from_item_number(n) else { continue };
            match out.iter_mut().find(|(k, _)| *k == c) {
                Some(e) => e.1 += v.stack.max(1),
                None => out.push((c, v.stack.max(1))),
            }
        }
        out
    }

    /// The player's default side deck: ten cards taken from owned cards,
    /// strongest first; topped up from the K2 starter row (pazaakdecks
    /// row 0, +1..+5 twice) when the party owns fewer than ten.
    pub fn pazaak_default_side_deck(&mut self) -> Vec<Card> {
        let mut owned: Vec<Card> = Vec::new();
        for (c, n) in self.pazaak_owned_cards() {
            for _ in 0..n {
                owned.push(c);
            }
        }
        owned.sort_by_key(|c| std::cmp::Reverse(card_rank(*c)));
        owned.truncate(10);
        if owned.len() < 10 {
            for c in self.pazaak_opponent_deck(0) {
                if owned.len() >= 10 {
                    break;
                }
                owned.push(c);
            }
        }
        owned
    }

    /// Starts the match for the pending request with the chosen side deck.
    pub fn pazaak_start(&mut self, player_side: Vec<Card>) -> Option<Pazaak> {
        let req = self.pazaak.clone()?;
        let opp = self.pazaak_opponent_deck(req.opponent_deck);
        let seed = (self.time * 1000.0) as u64 ^ 0x9E37_79B9_7F4A_7C15;
        Some(Pazaak::new([player_side, opp], seed))
    }

    /// Ends the pending PlayPazaak: sets GetLastPazaakResult, moves the
    /// wager (only when max_wager > 0) and runs the end script as the
    /// caller. `forfeit` counts as a loss.
    pub fn pazaak_finish(&mut self, won: bool, wager: i32) {
        let Some(req) = self.pazaak.take() else { return };
        self.last_pazaak = i32::from(won);
        if req.max_wager > 0 && !req.tutorial {
            let w = wager.clamp(0, req.max_wager).min(self.credits());
            self.add_credits(if won { w } else { -w });
        }
        if !req.end_script.is_empty() {
            let owner = if self.is_valid(req.owner) { req.owner } else { self.module_id };
            self.run_script(&req.end_script.clone(), owner, crate::ScriptCtx::default());
            self.pump();
        }
    }

    pub(crate) fn pazaak_call(&mut self, name: &str, me: ObjectId, a: &[k2_script::Value]) -> Option<k2_script::Value> {
        use k2_script::Value;
        match name {
            "PlayPazaak" => {
                let int = |i: usize| match a.get(i) {
                    Some(Value::Int(v)) => *v,
                    _ => 0,
                };
                let end_script = match a.get(1) {
                    Some(Value::String(s)) => s.to_string(),
                    _ => String::new(),
                };
                let opponent = match a.get(4) {
                    Some(Value::Object(o)) => *o,
                    _ => OBJECT_INVALID,
                };
                self.pazaak = Some(PazaakRequest {
                    opponent_deck: int(0),
                    end_script,
                    max_wager: int(2),
                    tutorial: int(3) != 0,
                    opponent,
                    owner: me,
                });
                self.emit(crate::WorldEvent::ShowGui("pazaak".into()));
                Some(Value::Void)
            }
            "GetLastPazaakResult" => Some(Value::Int(self.last_pazaak)),
            _ => None,
        }
    }
}

/// Rough usefulness for picking a default side deck.
fn card_rank(c: Card) -> i32 {
    match c {
        Card::PlusMinus(n) => 20 + n as i32,
        Card::TieBreaker => 19,
        Card::OneOrTwo => 18,
        Card::Minus(n) => 10 + n as i32,
        Card::Plus(n) => 5 + n as i32,
        Card::Double => 4,
        Card::Flip24 | Card::Flip36 => 3,
    }
}

#[cfg(test)]
mod tests;
