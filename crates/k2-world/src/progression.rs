//! Experience, levels and level up.
//!
//! - XP: GiveXPToCreature, SetXP, GivePlotXP (plot.2da), journal XP
//!   (global.jrl entry XP_Percentage of the category's plot.2da row) and
//!   kill XP (xptable.2da: row = killer level, column = victim
//!   ChallengeRating). Level thresholds come from exptable.2da.
//! - The PC's XP is shared: party members and spawned available NPCs get
//!   the same amount, as in K2 where companions keep pace with the Exile.
//! - Level up follows the class tables: hit die + Con (max roll per level,
//!   as KotOR does), force die + Wis for Force users, skill points
//!   (skillpointbase + Int, minimum 1), an attribute point every 4 levels,
//!   feats from featgain.2da (<cls>_reg + <cls>_bon) and Force powers from
//!   classpowergain.2da. Granted feats (feat.2da <cls>_granted and, for the
//!   PC, <cls>_pc_granted) arrive automatically.
//! - Non-PC party members level automatically (K2 offers the same).
//!
//! State is plain data so saves can serialize it.

use std::collections::{BTreeSet, HashMap};

use k2_data::GffExt;
use k2_script::{ObjectId, Value, OBJECT_INVALID};
use rakata_core::ResourceType;
use serde::{Deserialize, Serialize};

use crate::craft::SKILL_COUNT;
use crate::{World, WorldEvent};

/// global.jrl quest tag -> (plot.2da row, entry id -> XP percentage).
type JrlPlots = HashMap<String, (i32, HashMap<i32, f32>)>;

/// Feats and Force powers a creature knows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Known {
    pub feats: Vec<u16>,
    pub powers: Vec<i32>,
}

/// Progression state. `known` is filled lazily from templates and then
/// owned here (level ups and GrantFeat/GrantSpell change it).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProgressionState {
    pub known: HashMap<ObjectId, Known>,
    /// Creatures told about an available level up (so the event fires once
    /// per threshold).
    pub announced: HashMap<ObjectId, i32>,
    /// (plot tag, entry id) pairs that already paid journal XP.
    pub journal_paid: BTreeSet<(String, i32)>,
    /// When set, the PC levels up automatically too (headless runs).
    pub auto_level_pc: bool,
    /// Template caches (not game state).
    #[serde(skip)]
    template_known: HashMap<String, Known>,
    #[serde(skip)]
    challenge: HashMap<String, f32>,
    #[serde(skip)]
    jrl_plots: Option<JrlPlots>,
}

/// Abbreviation used by featgain.2da / feat.2da / skills.2da columns.
fn class_abbr(w: &World, class: i32) -> String {
    let row = usize::try_from(class).unwrap_or(usize::MAX);
    w.tables
        .get(&w.res, "classes")
        .ok()
        .and_then(|t| t.str(row, "featgain").map(str::to_ascii_lowercase))
        .unwrap_or_else(|| "sol".into())
}

/// spells.2da column holding the level at which `class` may learn a power.
pub(crate) fn spell_class_column(class: i32) -> Option<&'static str> {
    Some(match class {
        3 => "guardian",
        4 => "consular",
        5 => "sentinel",
        11 => "weapmstr",
        12 => "jedimaster",
        13 => "watchman",
        14 => "marauder",
        15 => "sithlord",
        16 => "assassin",
        _ => return None,
    })
}

pub fn ability_mod(score: i32) -> i32 {
    (score - 10).div_euclid(2)
}

fn ability_index(name: &str) -> usize {
    match name.to_ascii_uppercase().as_str() {
        "STR" => 0,
        "DEX" => 1,
        "CON" => 2,
        "INT" => 3,
        "WIS" => 4,
        "CHA" => 5,
        _ => 0,
    }
}

/// A feat or power the level up may pick.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub id: i32,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    /// Lower = recommended earlier (feat.2da <cls>_recom, spells
    /// light_recom/dark_recom); i32::MAX when not recommended.
    pub recommended: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkillChoice {
    pub id: usize,
    pub name: String,
    pub rank: i32,
    pub class_skill: bool,
    /// Highest rank allowed after this level up.
    pub max_rank: i32,
    /// Points per rank (1 class skill, 2 cross-class).
    pub cost: i32,
    pub recommended: i32,
}

/// What the next level up offers.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelUpOptions {
    pub creature: ObjectId,
    pub class: i32,
    pub class_name: String,
    /// Level in `class` after the level up.
    pub class_level: i32,
    /// Total character level after the level up.
    pub new_level: i32,
    pub hp_gain: i32,
    pub force_gain: i32,
    pub skill_points: i32,
    pub attribute_points: i32,
    pub feat_picks: i32,
    pub power_picks: i32,
    /// Granted automatically at this level.
    pub granted_feats: Vec<Choice>,
    pub feats: Vec<Choice>,
    pub powers: Vec<Choice>,
    pub skills: Vec<SkillChoice>,
    /// Primary ability (classes.2da primaryabil), the auto choice.
    pub primary_ability: usize,
}

/// The player's picks. `skills[i]` = ranks bought in skill i.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LevelUpChoice {
    pub attribute: Option<usize>,
    pub skills: [i32; SKILL_COUNT],
    pub feats: Vec<u16>,
    pub powers: Vec<i32>,
}

impl World {
    fn tlk_text(&self, strref: Option<i32>) -> String {
        match (strref, &self.talk) {
            (Some(s), Some(t)) if s >= 0 => t.get(s as u32).unwrap_or("").to_owned(),
            _ => String::new(),
        }
    }

    // ---------------------------------------------------------------
    // Known feats and powers
    // ---------------------------------------------------------------

    fn template_known(&mut self, template: &str) -> Known {
        if let Some(k) = self.progression.template_known.get(template) {
            return k.clone();
        }
        let mut k = Known::default();
        if !template.is_empty()
            && let Ok(g) = k2_data::read_gff(&self.res, template, ResourceType::Utc)
        {
            k.feats = g.root.list("FeatList").iter().filter_map(|f| f.uint("Feat").map(|x| x as u16)).collect();
            for c in g.root.list("ClassList") {
                for s in c.list("KnownList0") {
                    if let Some(sp) = s.int("Spell")
                        && !k.powers.contains(&sp)
                    {
                        k.powers.push(sp);
                    }
                }
            }
        }
        self.progression.template_known.insert(template.to_owned(), k.clone());
        k
    }

    /// Makes sure `id` has an entry in `progression.known`; the PC without a
    /// template starts with its class's level-1 grants and powers.
    fn ensure_known(&mut self, id: ObjectId) {
        if self.progression.known.contains_key(&id) {
            return;
        }
        let Some(o) = self.get(id) else { return };
        let Some(c) = o.creature() else { return };
        let template = o.template.clone();
        let is_pc = c.is_pc;
        let classes = c.classes.clone();
        let mut k = self.template_known(&template);
        self.ensure_force_pool(id);
        if template.is_empty() && is_pc {
            // No character generation yet: what a fresh level 1 gets.
            for &(class, level) in &classes {
                for l in 1..=level.max(1) {
                    for f in self.granted_feats(class, l, true) {
                        if !k.feats.contains(&(f.id as u16)) {
                            k.feats.push(f.id as u16);
                        }
                    }
                }
            }
            self.progression.known.insert(id, k.clone());
            let class = classes.last().map_or(3, |c| c.0);
            let picks = self.power_picks(class, 1);
            let good = c_good(self, id);
            let mut avail = self.available_powers(id, class, 1);
            sort_recommended(&mut avail, good);
            for p in avail.into_iter().take(picks as usize) {
                k.powers.push(p.id);
            }
        }
        self.progression.known.insert(id, k);
    }

    /// Force users whose template gives no Force points get the class pool:
    /// sum of forcedie x level plus Wis modifier x level (KotOR's rule).
    fn ensure_force_pool(&mut self, id: ObjectId) {
        let Some(c) = self.get(id).and_then(|o| o.creature()) else { return };
        if c.max_force > 0 {
            return;
        }
        let classes = c.classes.clone();
        let wis = ability_mod(i32::from(c.abilities[4]));
        let Ok(t) = self.tables.get(&self.res, "classes") else { return };
        let mut pool = 0;
        let mut force_levels = 0;
        for &(class, lvl) in &classes {
            let die = t.int(usize::try_from(class).unwrap_or(usize::MAX), "forcedie").unwrap_or(0);
            if die > 0 {
                pool += die * lvl.max(0);
                force_levels += lvl.max(0);
            }
        }
        if pool == 0 {
            return;
        }
        let pool = (pool + wis * force_levels).max(1);
        if let Some(c) = self.get_mut(id).and_then(|o| o.creature_mut()) {
            c.max_force = pool;
            c.force = pool;
        }
    }

    pub fn known_feats(&mut self, id: ObjectId) -> Vec<u16> {
        self.ensure_known(id);
        self.progression.known.get(&id).map(|k| k.feats.clone()).unwrap_or_default()
    }

    pub fn known_powers(&mut self, id: ObjectId) -> Vec<i32> {
        self.ensure_known(id);
        self.progression.known.get(&id).map(|k| k.powers.clone()).unwrap_or_default()
    }

    pub fn has_feat(&mut self, id: ObjectId, feat: i32) -> bool {
        feat >= 0 && self.known_feats(id).contains(&(feat as u16))
    }

    pub fn has_power(&mut self, id: ObjectId, spell: i32) -> bool {
        self.known_powers(id).contains(&spell)
    }

    pub fn grant_feat(&mut self, id: ObjectId, feat: u16) {
        self.ensure_known(id);
        if let Some(k) = self.progression.known.get_mut(&id)
            && !k.feats.contains(&feat)
        {
            k.feats.push(feat);
        }
    }

    pub fn grant_power(&mut self, id: ObjectId, spell: i32) {
        self.ensure_known(id);
        if let Some(k) = self.progression.known.get_mut(&id)
            && !k.powers.contains(&spell)
        {
            k.powers.push(spell);
        }
    }

    // ---------------------------------------------------------------
    // XP
    // ---------------------------------------------------------------

    pub fn level_of(&self, id: ObjectId) -> i32 {
        self.get(id).and_then(|o| o.creature()).map_or(1, |c| c.classes.iter().map(|&(_, l)| l.max(0)).sum::<i32>().max(1))
    }

    /// XP needed to reach `level` (exptable.2da).
    pub fn xp_for_level(&self, level: i32) -> i32 {
        let row = usize::try_from(level - 1).unwrap_or(0);
        self.tables.get(&self.res, "exptable").ok().and_then(|t| t.int(row, "xp")).unwrap_or(i32::MAX)
    }

    /// Whether `id` has enough XP for another level.
    pub fn level_up_available(&self, id: ObjectId) -> bool {
        let Some(c) = self.get(id).and_then(|o| o.creature()) else { return false };
        let level = self.level_of(id);
        level < 50 && c.xp >= self.xp_for_level(level + 1)
    }

    /// Party members and spawned available NPCs share PC XP.
    fn xp_recipients(&self, id: ObjectId) -> Vec<ObjectId> {
        let mut out = vec![id];
        let is_pc = id == self.player || self.party.first() == Some(&id) || self.party.contains(&id);
        if is_pc {
            for &p in &self.party {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
            if self.is_valid(self.player) && !out.contains(&self.player) {
                out.push(self.player);
            }
            for n in self.available_npcs.values() {
                if let Some(o) = n.object
                    && self.is_valid(o)
                    && !out.contains(&o)
                {
                    out.push(o);
                }
            }
        }
        out
    }

    /// GiveXPToCreature semantics plus sharing, events and auto level up.
    pub fn give_xp(&mut self, id: ObjectId, amount: i32) {
        if amount == 0 {
            return;
        }
        for r in self.xp_recipients(id) {
            let Some(c) = self.get_mut(r).and_then(|o| o.creature_mut()) else { continue };
            c.xp = (c.xp + amount).max(0);
            self.emit(WorldEvent::XpGained { creature: r, amount });
            self.check_level_up(r);
        }
    }

    pub fn set_xp(&mut self, id: ObjectId, xp: i32) {
        if let Some(c) = self.get_mut(id).and_then(|o| o.creature_mut()) {
            c.xp = xp.max(0);
        }
        self.check_level_up(id);
    }

    /// Announces (once per level) or applies an available level up.
    fn check_level_up(&mut self, id: ObjectId) {
        let auto = self.get(id).and_then(|o| o.creature()).is_some_and(|c| !c.is_pc) || self.progression.auto_level_pc;
        let mut guard = 0;
        while self.level_up_available(id) && guard < 50 {
            guard += 1;
            let level = self.level_of(id);
            if auto {
                let Some(opts) = self.level_up_options(id) else { break };
                let choice = self.auto_choice(&opts);
                if self.apply_level_up(id, &choice).is_err() {
                    break;
                }
            } else {
                if self.progression.announced.get(&id).copied() != Some(level + 1) {
                    self.progression.announced.insert(id, level + 1);
                    self.emit(WorldEvent::LevelUpAvailable { creature: id, level: level + 1 });
                }
                break;
            }
        }
    }

    /// GivePlotXP(plot, percentage): plot.2da xp * percentage / 100.
    pub fn give_plot_xp(&mut self, plot: &str, percentage: i32) {
        let Ok(t) = self.tables.get(&self.res, "plot") else { return };
        let row = (0..t.rows()).find(|&r| t.row_label(r).is_some_and(|l| l.eq_ignore_ascii_case(plot)) || t.str(r, "label").is_some_and(|l| l.eq_ignore_ascii_case(plot)));
        let Some(row) = row else { return };
        let xp = t.int(row, "xp").unwrap_or(0) * percentage / 100;
        let pc = self.party_leader();
        self.give_xp(pc, xp);
    }

    fn jrl_plots(&mut self) -> &JrlPlots {
        if self.progression.jrl_plots.is_none() {
            let mut m = HashMap::new();
            if let Ok(g) = k2_data::read_gff(&self.res, "global", ResourceType::Jrl) {
                for c in g.root.list("Categories") {
                    let tag = c.string_or_empty("Tag").to_ascii_lowercase();
                    let plot = c.int("PlotIndex").unwrap_or(-1);
                    let entries = c.list("EntryList").iter().map(|e| (e.int("ID").unwrap_or(0), e.float_or("XP_Percentage", 0.0))).collect();
                    m.insert(tag, (plot, entries));
                }
            }
            self.progression.jrl_plots = Some(m);
        }
        self.progression.jrl_plots.as_ref().expect("filled above")
    }

    /// Journal XP for reaching `entry` of quest `plot` (once).
    pub(crate) fn journal_xp(&mut self, plot: &str, entry: i32) {
        let key = (plot.to_ascii_lowercase(), entry);
        if self.progression.journal_paid.contains(&key) {
            return;
        }
        let Some((plot_index, pct)) = self.jrl_plots().get(&key.0).and_then(|(p, e)| e.get(&entry).map(|x| (*p, *x))) else { return };
        if plot_index < 0 || pct <= 0.0 {
            return;
        }
        let base = self.tables.get(&self.res, "plot").ok().and_then(|t| t.int(plot_index as usize, "xp")).unwrap_or(0);
        self.progression.journal_paid.insert(key);
        let xp = (base as f32 * pct / 100.0).round() as i32;
        let pc = self.party_leader();
        self.give_xp(pc, xp);
    }

    fn challenge_rating(&mut self, id: ObjectId) -> f32 {
        let Some(o) = self.get(id) else { return 0.0 };
        let template = o.template.clone();
        if let Some(&cr) = self.progression.challenge.get(&template) {
            return cr;
        }
        let cr = k2_data::read_gff(&self.res, &template, ResourceType::Utc)
            .ok()
            .and_then(|g| g.root.float("ChallengeRating"))
            .unwrap_or_else(|| self.level_of(id) as f32);
        self.progression.challenge.insert(template, cr);
        cr
    }

    /// Kill XP when a party member kills a hostile: xptable.2da[party
    /// level][victim CR].
    pub(crate) fn award_kill_xp(&mut self, victim: ObjectId, killer: ObjectId) {
        if killer == OBJECT_INVALID || !self.party.contains(&killer) || self.party.contains(&victim) {
            return;
        }
        if !self.is_enemy(victim, killer) && !self.is_enemy(killer, victim) {
            return;
        }
        let cr = self.challenge_rating(victim).round().max(0.0) as i32;
        let leader = self.party_leader();
        let level = self.level_of(leader);
        let Ok(t) = self.tables.get(&self.res, "xptable") else { return };
        let row = usize::try_from(level - 1).unwrap_or(0).min(t.rows().saturating_sub(1));
        let xp = t.int(row, &cr.min(50).to_string()).unwrap_or(0);
        if xp > 0 {
            self.give_xp(leader, xp);
        }
    }

    // ---------------------------------------------------------------
    // Level up
    // ---------------------------------------------------------------

    fn granted_feats(&self, class: i32, class_level: i32, is_pc: bool) -> Vec<Choice> {
        let abbr = class_abbr(self, class);
        let Ok(t) = self.tables.get(&self.res, "feat") else { return Vec::new() };
        let mut out = Vec::new();
        for r in 0..t.rows() {
            let granted = t.int(r, &format!("{abbr}_granted")).unwrap_or(-1) == class_level
                || (is_pc && t.int(r, &format!("{abbr}_pc_granted")).unwrap_or(-1) == class_level);
            if granted {
                out.push(self.feat_choice(&t, r, &abbr));
            }
        }
        out
    }

    fn feat_choice(&self, t: &k2_data::tables::Table, r: usize, abbr: &str) -> Choice {
        Choice {
            id: r as i32,
            name: self.tlk_text(t.int(r, "name")),
            description: self.tlk_text(t.int(r, "description")),
            icon: t.resref(r, "icon"),
            recommended: t.int(r, &format!("{abbr}_recom")).unwrap_or(i32::MAX),
        }
    }

    fn power_picks(&self, class: i32, class_level: i32) -> i32 {
        let Ok(classes) = self.tables.get(&self.res, "classes") else { return 0 };
        let row = usize::try_from(class).unwrap_or(usize::MAX);
        let Some(col) = classes.str(row, "spellgaintable").map(str::to_ascii_lowercase) else { return 0 };
        self.tables
            .get(&self.res, "classpowergain")
            .ok()
            .and_then(|t| t.int(usize::try_from(class_level - 1).unwrap_or(0), &col))
            .unwrap_or(0)
    }

    fn available_feats(&mut self, id: ObjectId, class: i32, new_level: i32) -> Vec<Choice> {
        let abbr = class_abbr(self, class);
        let known = self.known_feats(id);
        let Ok(t) = self.tables.get(&self.res, "feat") else { return Vec::new() };
        let mut out = Vec::new();
        for r in 0..t.rows() {
            let list = t.int(r, &format!("{abbr}_list")).unwrap_or(4);
            if !(list == 1 || list == 2) || known.contains(&(r as u16)) {
                continue;
            }
            if t.str(r, "label").is_some_and(|l| l.starts_with("XXX")) {
                continue;
            }
            if t.int(r, "mincharlevel").is_some_and(|m| m > new_level) {
                continue;
            }
            let pre_ok = ["prereqfeat1", "prereqfeat2"].iter().all(|c| t.int(r, c).is_none_or(|p| known.contains(&(p as u16))));
            let ors: Vec<i32> = (0..5).filter_map(|i| t.int(r, &format!("orreqfeat{i}"))).collect();
            let or_ok = ors.is_empty() || ors.iter().any(|p| known.contains(&(*p as u16)));
            if !pre_ok || !or_ok {
                continue;
            }
            let c = self.feat_choice(&t, r, &abbr);
            if !c.name.is_empty() {
                out.push(c);
            }
        }
        out
    }

    fn available_powers(&mut self, id: ObjectId, class: i32, class_level: i32) -> Vec<Choice> {
        let Some(col) = spell_class_column(class) else { return Vec::new() };
        let known = self.known_powers(id);
        let Ok(t) = self.tables.get(&self.res, "spells") else { return Vec::new() };
        let good = c_good(self, id);
        let mut out = Vec::new();
        for r in 0..t.rows() {
            if t.int(r, "usertype") != Some(1) || known.contains(&(r as i32)) {
                continue;
            }
            let Some(min) = t.int(r, col) else { continue };
            if min > class_level {
                continue;
            }
            let pre_ok = t.str(r, "prerequisites").is_none_or(|p| p.split('_').filter_map(|x| x.parse::<i32>().ok()).all(|p| known.contains(&p)));
            if !pre_ok {
                continue;
            }
            let name = self.tlk_text(t.int(r, "name"));
            if name.is_empty() {
                continue;
            }
            let recom = if good { t.int(r, "light_recom") } else { t.int(r, "dark_recom") };
            out.push(Choice {
                id: r as i32,
                name,
                description: self.tlk_text(t.int(r, "spelldesc")),
                icon: t.resref(r, "iconresref"),
                recommended: recom.unwrap_or(i32::MAX),
            });
        }
        out
    }

    /// What the next level up of `id` offers (None if not a creature).
    pub fn level_up_options(&mut self, id: ObjectId) -> Option<LevelUpOptions> {
        let c = self.get(id)?.creature()?.clone();
        let (class, class_level) = c.classes.last().copied().unwrap_or((0, 0));
        let class_level = class_level + 1;
        let new_level = c.classes.iter().map(|&(_, l)| l.max(0)).sum::<i32>() + 1;
        let classes = self.tables.get(&self.res, "classes").ok()?;
        let row = usize::try_from(class).unwrap_or(0);
        let abbr = class_abbr(self, class);
        let hitdie = classes.int(row, "hitdie").unwrap_or(8);
        let forcedie = classes.int(row, "forcedie").unwrap_or(0);
        let skill_base = classes.int(row, "skillpointbase").unwrap_or(1);
        let primary = ability_index(classes.str(row, "primaryabil").unwrap_or("STR"));
        let con = ability_mod(i32::from(c.abilities[2]));
        let wis = ability_mod(i32::from(c.abilities[4]));
        let int = ability_mod(i32::from(c.abilities[3]));
        let feat_picks = self
            .tables
            .get(&self.res, "featgain")
            .ok()
            .map(|t| {
                let r = usize::try_from(class_level - 1).unwrap_or(0);
                t.int(r, &format!("{abbr}_reg")).unwrap_or(0) + t.int(r, &format!("{abbr}_bon")).unwrap_or(0)
            })
            .unwrap_or(0);
        let power_picks = self.power_picks(class, class_level);
        let granted: Vec<Choice> = self
            .granted_feats(class, class_level, c.is_pc)
            .into_iter()
            .filter(|f| !self.progression.known.get(&id).is_some_and(|k| k.feats.contains(&(f.id as u16))))
            .collect();
        let feats = self.available_feats(id, class, new_level);
        let powers = self.available_powers(id, class, class_level);
        let skills_t = self.tables.get(&self.res, "skills").ok();
        let mut skills = Vec::new();
        for s in 0..SKILL_COUNT {
            let class_skill = skills_t.as_ref().is_some_and(|t| t.flag(s, &format!("{abbr}_class")));
            let rank = self.skill_rank(id, s);
            let max_rank = if class_skill { new_level + 3 } else { (new_level + 3) / 2 };
            skills.push(SkillChoice {
                id: s,
                name: skills_t.as_ref().map(|t| self.tlk_text(t.int(s, "name"))).unwrap_or_default(),
                rank,
                class_skill,
                max_rank,
                cost: if class_skill { 1 } else { 2 },
                recommended: skills_t.as_ref().and_then(|t| t.int(s, &format!("{abbr}_reco"))).unwrap_or(i32::MAX),
            });
        }
        Some(LevelUpOptions {
            creature: id,
            class,
            class_name: self.tlk_text(classes.int(row, "name")),
            class_level,
            new_level,
            hp_gain: (hitdie + con).max(1),
            force_gain: if forcedie > 0 { (forcedie + wis).max(1) } else { 0 },
            // First level (character generation) doubles: K2's own class
            // descriptions read "Skill Points (*2 at 1st level)".
            skill_points: (skill_base + int).max(1) * if new_level == 1 { 2 } else { 1 },
            attribute_points: i32::from(new_level % 4 == 0),
            feat_picks,
            power_picks,
            granted_feats: granted,
            feats,
            powers,
            skills,
            primary_ability: primary,
        })
    }

    /// Recommended picks (party auto level up, headless tests).
    pub fn auto_choice(&mut self, o: &LevelUpOptions) -> LevelUpChoice {
        let mut choice = LevelUpChoice { attribute: (o.attribute_points > 0).then_some(o.primary_ability), ..Default::default() };
        let mut feats = o.feats.clone();
        feats.sort_by_key(|f| (f.recommended, f.id));
        choice.feats = feats.iter().take(o.feat_picks.max(0) as usize).map(|f| f.id as u16).collect();
        let good = c_good(self, o.creature);
        let mut powers = o.powers.clone();
        sort_recommended(&mut powers, good);
        choice.powers = powers.iter().take(o.power_picks.max(0) as usize).map(|p| p.id).collect();
        let mut skills: Vec<&SkillChoice> = o.skills.iter().filter(|s| s.class_skill).collect();
        skills.sort_by_key(|s| (s.recommended, s.id));
        let mut points = o.skill_points;
        'outer: while points > 0 {
            let mut spent = false;
            for s in &skills {
                if points < s.cost {
                    continue;
                }
                if s.rank + choice.skills[s.id] < s.max_rank {
                    choice.skills[s.id] += 1;
                    points -= s.cost;
                    spent = true;
                    if points <= 0 {
                        break 'outer;
                    }
                }
            }
            if !spent {
                break;
            }
        }
        choice
    }

    /// Applies one level up. Validates the picks against the options.
    pub fn apply_level_up(&mut self, id: ObjectId, choice: &LevelUpChoice) -> Result<(), String> {
        if !self.level_up_available(id) {
            return Err("not enough experience".into());
        }
        self.apply_level_up_unchecked(id, choice)
    }

    /// [`Self::apply_level_up`] without the XP check: character generation
    /// takes a level 0 creature to level 1.
    pub(crate) fn apply_level_up_unchecked(&mut self, id: ObjectId, choice: &LevelUpChoice) -> Result<(), String> {
        // The level-up gain would raise a dead creature's HP (dead stays set) or
        // hand a knocked-out one HP without get_up. Refuse both, before anything
        // is spent.
        if self.get(id).and_then(|o| o.creature()).is_some_and(|c| c.dead || c.down) {
            return Err("the dead and the knocked out cannot level up".into());
        }
        let o = self.level_up_options(id).ok_or("not a creature")?;
        if choice.feats.len() as i32 > o.feat_picks || choice.powers.len() as i32 > o.power_picks {
            return Err("too many picks".into());
        }
        if choice.feats.iter().any(|f| !o.feats.iter().any(|c| c.id == i32::from(*f))) {
            return Err("feat not available".into());
        }
        if choice.powers.iter().any(|p| !o.powers.iter().any(|c| c.id == *p)) {
            return Err("power not available".into());
        }
        let mut spent = 0;
        for s in &o.skills {
            let add = choice.skills[s.id];
            if add < 0 || (add > 0 && s.rank + add > s.max_rank) {
                return Err(format!("skill {} over its maximum", s.name));
            }
            spent += add * s.cost;
        }
        if spent > o.skill_points {
            return Err("too many skill points".into());
        }
        if choice.attribute.is_some() && o.attribute_points == 0 {
            return Err("no attribute point this level".into());
        }
        // Skills before the class level changes (rank lookups use the template).
        for s in &o.skills {
            if choice.skills[s.id] > 0 {
                self.set_skill_rank(id, s.id, s.rank + choice.skills[s.id]);
            }
        }
        self.ensure_known(id);
        {
            let c = self.get_mut(id).and_then(|x| x.creature_mut()).ok_or("not a creature")?;
            if let Some(last) = c.classes.last_mut() {
                last.1 += 1;
            } else {
                c.classes.push((o.class, 1));
            }
            if let Some(a) = choice.attribute {
                c.abilities[a.min(5)] = c.abilities[a.min(5)].saturating_add(1);
            }
            c.max_hp += o.hp_gain;
            c.hp += o.hp_gain;
            c.max_force += o.force_gain;
            c.force += o.force_gain;
        }
        if let Some(k) = self.progression.known.get_mut(&id) {
            for f in o.granted_feats.iter().map(|f| f.id as u16).chain(choice.feats.iter().copied()) {
                if !k.feats.contains(&f) {
                    k.feats.push(f);
                }
            }
            for &p in &choice.powers {
                if !k.powers.contains(&p) {
                    k.powers.push(p);
                }
            }
        }
        self.progression.announced.remove(&id);
        self.emit(WorldEvent::LevelledUp { creature: id, level: o.new_level, class: o.class });
        Ok(())
    }

    /// Engine routines for XP and levels. None = not handled here.
    pub(crate) fn progression_call(&mut self, name: &str, me: ObjectId, a: &[Value]) -> Option<Value> {
        let int = |n: usize| a.get(n).and_then(Value::as_int).unwrap_or(0);
        let obj = |n: usize| a.get(n).and_then(Value::as_object).unwrap_or(OBJECT_INVALID);
        let obj_or_me = |n: usize| match a.get(n).and_then(Value::as_object) {
            Some(o) if o != OBJECT_INVALID => o,
            _ => me,
        };
        Some(match name {
            "GiveXPToCreature" => {
                self.give_xp(obj(0), int(1));
                Value::Void
            }
            "SetXP" => {
                self.set_xp(obj(0), int(1));
                Value::Void
            }
            "GetXP" => Value::Int(self.get(obj(0)).and_then(|o| o.creature()).map_or(0, |c| c.xp)),
            "GivePlotXP" => {
                let plot = a.first().and_then(Value::as_str).unwrap_or("").to_owned();
                self.give_plot_xp(&plot, int(1));
                Value::Void
            }
            "GetJournalQuestExperience" => {
                let plot = a.first().and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
                let pi = self.jrl_plots().get(&plot).map_or(-1, |(p, _)| *p);
                let xp = if pi >= 0 { self.tables.get(&self.res, "plot").ok().and_then(|t| t.int(pi as usize, "xp")).unwrap_or(0) } else { 0 };
                Value::Int(xp)
            }
            "AddJournalQuestEntry" => {
                // Journal XP; the journal itself is applied by host.rs.
                if let Some(plot) = a.first().and_then(Value::as_str) {
                    let plot = plot.to_owned();
                    self.journal_xp(&plot, int(1));
                }
                return None;
            }
            "ShowLevelUpGUI" => {
                self.emit(WorldEvent::ShowGui("levelup".into()));
                Value::Int(1)
            }
            "GetPCLevellingUp" => Value::Object(self.party_leader()),
            "AddMultiClass" => {
                let id = obj_or_me(1);
                let class = int(0);
                if let Some(c) = self.get_mut(id).and_then(|o| o.creature_mut())
                    && !c.classes.iter().any(|&(k, _)| k == class)
                    && c.classes.len() < 2
                {
                    c.classes.push((class, 0));
                }
                Value::Void
            }
            "GetHasFeat" | "GetFeatAcquired" => {
                let id = obj_or_me(1);
                Value::Int(i32::from(self.has_feat(id, int(0))))
            }
            "GrantFeat" => {
                self.grant_feat(obj(1), int(0).max(0) as u16);
                Value::Void
            }
            "GrantSpell" => {
                self.grant_power(obj(1), int(0));
                Value::Void
            }
            _ => return None,
        })
    }
}

fn c_good(w: &World, id: ObjectId) -> bool {
    w.get(id).and_then(|o| o.creature()).is_none_or(|c| c.good_evil >= 50)
}

fn sort_recommended(v: &mut [Choice], _good: bool) {
    v.sort_by_key(|c| (c.recommended, c.id));
}
