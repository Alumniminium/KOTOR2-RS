//! Character generation: the choices K2's New Game screens offer (class and
//! gender, portrait, attribute point buy, skills, feats, Force powers, name)
//! and turning them into the PC.
//!
//! Rules, as the original's chargen screens apply them:
//! - Classes: Jedi Consular, Sentinel, Guardian; the classsel_p buttons are
//!   male Consular/Sentinel/Guardian then female Guardian/Sentinel/Consular.
//! - Body: the portraits.2da row gives three appearances; Consular takes
//!   appearance_s, Guardian appearance_l, Sentinel appearancenumber.
//! - Attributes: 30 points over a base of 8 each, max 18; raising a score
//!   costs its current modifier clamped to 1..=3.
//! - Level 1 itself goes through the progression level up (hit die, Force
//!   die, skill points x2 as the class descriptions state, granted and picked
//!   feats, powers).

use rakata_formats::{Ltr, LTR_CHARACTER_COUNT};

use crate::craft::SKILL_COUNT;
use crate::object::{Detail, Object, ObjectKind};
use crate::progression::{LevelUpChoice, LevelUpOptions, ability_mod};
use crate::{PlayerSpec, World};

pub const POINT_BUY: i32 = 30;
pub const BASE_SCORE: u8 = 8;
pub const MAX_SCORE: u8 = 18;

/// classsel_p BTN_SEL1..=6 as (gender, class): male Consular, Sentinel,
/// Guardian, then female Guardian, Sentinel, Consular.
pub const CLASS_BUTTONS: [(i32, i32); 6] = [(0, 4), (0, 5), (0, 3), (1, 3), (1, 5), (1, 4)];

/// A portraits.2da row the player may pick.
#[derive(Debug, Clone, PartialEq)]
pub struct PortraitOption {
    pub row: u32,
    pub resref: String,
    pub gender: i32,
    /// appearance_s, appearancenumber, appearance_l.
    pub appearance: [u32; 3],
}

/// Cost of raising a score by one.
pub fn point_cost(score: u8) -> i32 {
    ability_mod(i32::from(score)).clamp(1, 3)
}

/// Points spent on `abilities` from the base of 8.
pub fn points_spent(abilities: &[u8; 6]) -> i32 {
    abilities.iter().map(|&a| (BASE_SCORE..a.max(BASE_SCORE)).map(point_cost).sum::<i32>()).sum()
}

impl World {
    fn tlk_str(&self, strref: Option<i32>) -> String {
        match (strref, &self.talk) {
            (Some(s), Some(t)) if s >= 0 => t.get(s as u32).unwrap_or("").to_owned(),
            _ => String::new(),
        }
    }

    /// The class screen's one-line summaries (Consular, Sentinel, Guardian).
pub fn class_summary_strref(class: i32) -> Option<u32> {
    match class {
        4 => Some(48031),
        5 => Some(48032),
        3 => Some(48033),
        _ => None,
    }
}

/// Class name and description (classes.2da).
    pub fn class_info(&self, class: i32) -> (String, String) {
        let Ok(t) = self.tables.get(&self.res, "classes") else { return (String::new(), String::new()) };
        let row = usize::try_from(class).unwrap_or(0);
        (self.tlk_str(t.int(row, "name")), self.tlk_str(t.int(row, "description")))
    }

    /// The class's suggested scores (classes.2da str..cha).
    pub fn class_default_abilities(&self, class: i32) -> [u8; 6] {
        let mut out = [12u8; 6];
        if let Ok(t) = self.tables.get(&self.res, "classes") {
            let row = usize::try_from(class).unwrap_or(0);
            for (i, col) in ["str", "dex", "con", "int", "wis", "cha"].iter().enumerate() {
                if let Some(v) = t.int(row, col) {
                    out[i] = v.clamp(i32::from(BASE_SCORE), i32::from(MAX_SCORE)) as u8;
                }
            }
        }
        out
    }

    /// Player portraits for a gender (portraits.2da forpc = 1).
    pub fn pc_portraits(&self, gender: i32) -> Vec<PortraitOption> {
        let Ok(t) = self.tables.get(&self.res, "portraits") else { return Vec::new() };
        (0..t.rows())
            .filter(|&r| t.int(r, "forpc") == Some(1) && t.int(r, "sex") == Some(gender))
            .filter_map(|r| {
                let med = u32::try_from(t.int(r, "appearancenumber")?).ok()?;
                let s = t.int(r, "appearance_s").and_then(|v| u32::try_from(v).ok()).unwrap_or(med);
                let l = t.int(r, "appearance_l").and_then(|v| u32::try_from(v).ok()).unwrap_or(med);
                Some(PortraitOption {
                    row: r as u32,
                    resref: t.str(r, "baseresref")?.to_ascii_lowercase(),
                    gender,
                    appearance: [s, med, l],
                })
            })
            .collect()
    }

    /// Appearance row for a portrait and class.
    pub fn appearance_for(p: &PortraitOption, class: i32) -> u32 {
        match class {
            4 => p.appearance[0],
            3 => p.appearance[2],
            _ => p.appearance[1],
        }
    }

    /// A random name from the game's letter tables (humanm/humanf + humanl).
    pub fn random_name(&mut self, gender: i32) -> String {
        let first = if gender == 1 { "humanf" } else { "humanm" };
        let mut name = self.ltr_name(first).unwrap_or_default();
        if let Some(last) = self.ltr_name("humanl") {
            name = format!("{name} {last}");
        }
        name.trim().to_owned()
    }

    fn ltr_name(&mut self, resref: &str) -> Option<String> {
        let bytes = self.res.get(&k2_res::ResKey::new(resref, rakata_core::ResourceType::Ltr)).ok().flatten()?;
        let ltr = rakata_formats::read_ltr_from_bytes(&bytes).ok()?;
        // Names should differ between presses: seed from the clock, not the
        // world's (saved, deterministic) combat RNG.
        let mut x = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0x9E37_79B9, |d| d.as_nanos() as u64) | 1;
        let mut rng = move || {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            ((x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32) / (1u64 << 24) as f32
        };
        for _ in 0..50 {
            if let Some(n) = generate_name(&ltr, &mut rng).filter(|n| (3..=10).contains(&n.len())) {
                let mut c = n.chars();
                return c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str());
            }
        }
        None
    }

    /// The PC creature a spec describes, at level 0 when it carries a class
    /// (character generation applies level 1 on top).
    pub(crate) fn pc_object(&self, id: k2_script::ObjectId, spec: &PlayerSpec) -> Object {
        let mut o = Object::new(id, ObjectKind::Creature);
        o.name = spec.name.clone();
        let mut c = crate::object::CreatureState {
            appearance: spec.appearance,
            gender: spec.gender,
            hp: 30,
            max_hp: 30,
            force: 10,
            max_force: 10,
            is_pc: true,
            faction: 2,
            good_evil: 50,
            abilities: spec.abilities.unwrap_or([14; 6]),
            // Without character generation (tests, --quickstart before the
            // menu existed): a level 1 Jedi Guardian.
            classes: vec![(3, 1)],
            walk_speed: 1.75,
            run_speed: 5.4,
            radius: 0.3,
            perception_range: 20.0,
            ..Default::default()
        };
        if let Some(class) = spec.class {
            c.classes = vec![(class, 0)];
            c.hp = 0;
            c.max_hp = 0;
            c.force = 0;
            c.max_force = 0;
        }
        self.fill_appearance(&mut c, 'A', 1);
        o.detail = Detail::Creature(Box::new(c));
        o
    }

    /// What level 1 offers for a spec (skills, feats, powers, picks).
    pub fn chargen_options(&mut self, spec: &PlayerSpec) -> Option<LevelUpOptions> {
        let mut spec = spec.clone();
        spec.class.get_or_insert(3);
        let id = self.alloc_id();
        let o = self.pc_object(id, &spec);
        self.objects.insert(id, o);
        self.craft.skills.insert(id, [0; SKILL_COUNT]);
        let opts = self.level_up_options(id);
        self.objects.remove(&id);
        self.craft.skills.remove(&id);
        self.progression.known.remove(&id);
        opts
    }

    /// Quick Character: class defaults and the recommended picks.
    pub fn quick_spec(&mut self, gender: i32, class: i32, portrait: Option<&PortraitOption>) -> PlayerSpec {
        let portraits;
        let p = match portrait {
            Some(p) => Some(p.clone()),
            None => {
                portraits = self.pc_portraits(gender);
                portraits.first().cloned()
            }
        };
        let mut spec = PlayerSpec {
            appearance: p.as_ref().map_or(self.player_spec.appearance, |p| Self::appearance_for(p, class)),
            gender,
            name: "Exile".into(),
            class: Some(class),
            portrait: p.as_ref().map(|p| p.row),
            abilities: Some(self.class_default_abilities(class)),
            ..Default::default()
        };
        if let Some(o) = self.chargen_options(&spec) {
            let auto = self.auto_choice(&o);
            spec.skills = Some(auto.skills);
            spec.feats = auto.feats;
            spec.powers = auto.powers;
        }
        spec
    }

    /// Validates a spec's level 1 picks; Err names the first problem.
    pub fn check_spec(&mut self, spec: &PlayerSpec) -> Result<(), String> {
        let abilities = spec.abilities.unwrap_or([BASE_SCORE; 6]);
        if abilities.iter().any(|&a| !(BASE_SCORE..=MAX_SCORE).contains(&a)) {
            return Err("attribute out of range".into());
        }
        if points_spent(&abilities) > POINT_BUY {
            return Err("too many attribute points".into());
        }
        if spec.name.trim().is_empty() {
            return Err("no name".into());
        }
        let o = self.chargen_options(spec).ok_or("no class")?;
        let skills = spec.skills.unwrap_or([0; SKILL_COUNT]);
        let spent: i32 = o.skills.iter().map(|s| skills[s.id] * s.cost).sum();
        if spent > o.skill_points {
            return Err("too many skill points".into());
        }
        if spec.feats.len() as i32 > o.feat_picks || spec.powers.len() as i32 > o.power_picks {
            return Err("too many picks".into());
        }
        Ok(())
    }

    /// The PC from a spec: level 0, then level 1 with the spec's picks.
    pub(crate) fn spawn_pc_from_spec(&mut self) -> k2_script::ObjectId {
        let id = self.alloc_id();
        let spec = self.player_spec.clone();
        let o = self.pc_object(id, &spec);
        self.objects.insert(id, o);
        if spec.class.is_some() {
            self.craft.skills.insert(id, [0; SKILL_COUNT]);
            let choice = LevelUpChoice { attribute: None, skills: spec.skills.unwrap_or([0; SKILL_COUNT]), feats: spec.feats.clone(), powers: spec.powers.clone() };
            if let Err(e) = self.apply_level_up_unchecked(id, &choice) {
                self.warnings.push(format!("chargen: {e}; using the recommended picks"));
                if let Some(opts) = self.level_up_options(id) {
                    let auto = self.auto_choice(&opts);
                    let _ = self.apply_level_up_unchecked(id, &auto);
                }
            }
            // The prologue reads this (a_bet3m4 sets it from GetGender too).
            self.globals.set_boolean("000_PLAYER_GENDER", spec.gender == 0);
        }
        id
    }
}

/// Markov name from an LTR table set. `rng` returns uniform [0, 1).
/// Probabilities are cumulative per context (NWN/KotOR LTR convention).
fn generate_name(ltr: &Ltr, rng: &mut dyn FnMut() -> f32) -> Option<String> {
    const LETTERS: &[u8; LTR_CHARACTER_COUNT] = b"abcdefghijklmnopqrstuvwxyz'-";
    let pick = |table: &[f32; LTR_CHARACTER_COUNT], r: f32| table.iter().position(|&p| p > r);
    let mut out: Vec<usize> = Vec::new();
    out.push(pick(&ltr.singles.start, rng())?);
    out.push(pick(&ltr.doubles[out[0]].start, rng())?);
    out.push(pick(&ltr.triples[out[0]][out[1]].start, rng())?);
    for _ in 0..8 {
        let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
        let block = &ltr.triples[a][b];
        if out.len() >= 4
            && let Some(e) = pick(&block.end, rng())
        {
            out.push(e);
            break;
        }
        out.push(pick(&block.middle, rng())?);
    }
    Some(out.iter().map(|&i| LETTERS[i] as char).collect())
}
