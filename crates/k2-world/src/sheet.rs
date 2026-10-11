//! Data for the menu screens: character sheet (classes, saves, XP, skills,
//! feats), the quest journal (global.jrl), and item use (medpacs, stims):
//! an item's CastSpell properties run the spells.2da impact script with the
//! spell routines (GetSpellId, GetSpellTargetObject, ...) answering for it.

use std::collections::HashMap;
use std::sync::Arc;

use k2_data::{GffExt, LocString};
use k2_script::{ObjectId, Value, OBJECT_INVALID};
use rakata_core::ResourceType;

use crate::craft::SKILL_COUNT;
use crate::object::ObjectKind;
use crate::{ScriptCtx, World, WorldEvent};

/// itempropdef.2da row of the CastSpell property.
const PROPERTY_CAST_SPELL: u16 = 10;

#[derive(Default)]
pub struct SheetState {
    /// Spells being cast (innermost last), for the spell routines.
    casts: Vec<SpellCast>,
    journal: Option<Arc<JournalDb>>,
    /// Plot tag to world time of its last journal update.
    pub journal_updated: HashMap<String, f64>,
    /// Reply texts the player picked, oldest first; the UI drains them into
    /// the dialog history.
    pub chosen_replies: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct SpellCast {
    spell: i32,
    caster: ObjectId,
    target: ObjectId,
    item: ObjectId,
}

#[derive(Debug, Clone, Default)]
pub struct JournalDb {
    pub categories: Vec<JournalCategory>,
}

#[derive(Debug, Clone, Default)]
pub struct JournalCategory {
    pub tag: String,
    pub name: LocString,
    pub priority: u32,
    pub planet: i32,
    pub entries: Vec<JournalEntryDef>,
}

#[derive(Debug, Clone, Default)]
pub struct JournalEntryDef {
    pub id: u32,
    pub end: bool,
    pub text: LocString,
    pub xp_percentage: f32,
}

/// One quest as the journal screen shows it.
#[derive(Debug, Clone)]
pub struct QuestView {
    pub tag: String,
    pub name: String,
    pub text: String,
    pub state: i32,
    pub completed: bool,
    pub priority: u32,
    pub planet: i32,
    pub updated: f64,
}

#[derive(Debug, Clone, Default)]
pub struct SkillView {
    pub id: usize,
    pub name: String,
    pub description: String,
    pub rank: i32,
    pub bonus: i32,
}

#[derive(Debug, Clone, Default)]
pub struct FeatView {
    pub id: u16,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CharacterSheet {
    pub id: ObjectId,
    pub name: String,
    pub classes: Vec<(String, i32)>,
    pub level: i32,
    pub abilities: [u8; 6],
    pub defense: i32,
    pub hp: i32,
    pub max_hp: i32,
    pub force: i32,
    pub max_force: i32,
    pub xp: i32,
    pub next_level_xp: i32,
    pub fortitude: i32,
    pub reflex: i32,
    pub will: i32,
    pub good_evil: i32,
    pub attack_main: i32,
    pub attack_off: Option<i32>,
    pub damage_main: String,
    pub damage_off: Option<String>,
    pub is_droid: bool,
}

/// Ability modifier, d20 style.
pub fn ability_mod(score: u8) -> i32 {
    (i32::from(score) - 10).div_euclid(2)
}

fn damage_text(w: &crate::items::WeaponInfo, ability: i32) -> String {
    let bonus = w.damage_bonus + if w.ranged { 0 } else { ability };
    let lo = w.num_dice + bonus + w.extra_dice.iter().map(|&(n, _)| n).sum::<i32>();
    let hi = w.num_dice * w.die + bonus + w.extra_dice.iter().map(|&(n, d)| n * d).sum::<i32>();
    format!("{}-{}", lo.max(1), hi.max(1))
}

impl World {
    fn tlk(&self, strref: Option<i32>) -> String {
        match (strref, &self.talk) {
            (Some(s), Some(t)) if s >= 0 => t.get(s as u32).unwrap_or("").to_owned(),
            _ => String::new(),
        }
    }

    // ---------------------------------------------------------------
    // Character sheet
    // ---------------------------------------------------------------

    pub fn character_sheet(&mut self, id: ObjectId) -> Option<CharacterSheet> {
        let o = self.get(id)?;
        let c = o.creature()?.clone();
        let name = self.substitute(&o.name);
        let stats = self.combat_stats(id);
        let classes_t = self.tables.get(&self.res, "classes").ok();
        let mut classes = Vec::new();
        let (mut fort, mut refl, mut will) = (0, 0, 0);
        for &(class, lvl) in &c.classes {
            let row = usize::try_from(class).unwrap_or(usize::MAX);
            let cname = classes_t.as_ref().map(|t| self.tlk(t.int(row, "name"))).unwrap_or_default();
            classes.push((cname, lvl));
            let table = classes_t.as_ref().and_then(|t| t.resref(row, "savingthrowtable"));
            if let Some(st) = table.and_then(|n| self.tables.get(&self.res, &n).ok()) {
                let r = usize::try_from(lvl.max(1) - 1).unwrap_or(0);
                fort += st.int(r, "fortsave").unwrap_or(0);
                refl += st.int(r, "refsave").unwrap_or(0);
                will += st.int(r, "willsave").unwrap_or(0);
            }
        }
        let level = c.classes.iter().map(|&(_, l)| l).sum::<i32>().max(1);
        let next_level_xp = self
            .tables
            .get(&self.res, "exptable")
            .ok()
            .and_then(|t| t.int(usize::try_from(level).unwrap_or(0), "xp"))
            .unwrap_or(0);
        let abilities = stats.as_ref().map_or(c.abilities, |s| s.abilities);
        let (attack_main, attack_off, damage_main, damage_off, defense) = match &stats {
            Some(s) => (
                s.attack_bonus(false),
                s.offhand.as_ref().map(|_| s.attack_bonus(true)),
                damage_text(&s.main, s.str_mod()),
                s.offhand.as_ref().map(|w| damage_text(w, s.str_mod())),
                s.defense,
            ),
            None => (0, None, String::new(), None, 10),
        };
        Some(CharacterSheet {
            id,
            name,
            classes,
            level,
            abilities,
            defense,
            hp: c.hp,
            max_hp: c.max_hp,
            force: c.force,
            max_force: c.max_force,
            xp: c.xp,
            next_level_xp,
            fortitude: fort + ability_mod(abilities[2]),
            reflex: refl + ability_mod(abilities[1]),
            will: will + ability_mod(abilities[4]),
            good_evil: c.good_evil,
            attack_main,
            attack_off,
            damage_main,
            damage_off,
            is_droid: c.racial_type == 5,
        })
    }

    /// Skill ranks with skills.2da names and descriptions. Bonus is the key
    /// ability modifier (skills.2da keyability).
    pub fn skill_views(&mut self, id: ObjectId) -> Vec<SkillView> {
        let abilities = self.get(id).and_then(|o| o.creature()).map_or([10; 6], |c| c.abilities);
        let Ok(t) = self.tables.get(&self.res, "skills") else { return Vec::new() };
        let mut out = Vec::new();
        for s in 0..SKILL_COUNT.min(t.rows()) {
            let key = match t.str(s, "keyability").unwrap_or("").to_ascii_uppercase().as_str() {
                "STR" => 0,
                "DEX" => 1,
                "CON" => 2,
                "INT" => 3,
                "WIS" => 4,
                "CHA" => 5,
                _ => 3,
            };
            let rank = self.skill_rank(id, s);
            out.push(SkillView {
                id: s,
                name: self.tlk(t.int(s, "name")),
                description: self.tlk(t.int(s, "description")),
                rank,
                bonus: ability_mod(abilities[key]),
            });
        }
        out
    }

    /// Known feats (template FeatList plus level-ups, see progression.rs),
    /// named via feat.2da.
    pub fn feat_views(&mut self, id: ObjectId) -> Vec<FeatView> {
        let ids = self.known_feats(id);
        let Ok(t) = self.tables.get(&self.res, "feat") else { return Vec::new() };
        ids.into_iter()
            .map(|f| {
                let r = usize::from(f);
                FeatView {
                    id: f,
                    name: self.tlk(t.int(r, "name")),
                    description: self.tlk(t.int(r, "description")),
                    icon: t.resref(r, "icon"),
                }
            })
            .filter(|f| !f.name.is_empty())
            .collect()
    }

    /// Known Force powers (template KnownList0 plus level-ups), named via
    /// spells.2da.
    pub fn power_views(&mut self, id: ObjectId) -> Vec<FeatView> {
        let ids: Vec<u16> = self.known_powers(id).into_iter().filter_map(|p| u16::try_from(p).ok()).collect();
        let Ok(t) = self.tables.get(&self.res, "spells") else { return Vec::new() };
        ids.into_iter()
            .map(|f| {
                let r = usize::from(f);
                FeatView { id: f, name: self.tlk(t.int(r, "name")), description: self.tlk(t.int(r, "spelldesc")), icon: t.resref(r, "iconresref") }
            })
            .filter(|f| !f.name.is_empty())
            .collect()
    }

    /// Active effects on an object as (label, beneficial), e.g.
    /// ("Attack Increase", true). Labels come from the Effect* routine name.
    pub fn effect_views(&self, id: ObjectId) -> Vec<(String, bool)> {
        let Some(o) = self.get(id) else { return Vec::new() };
        o.effects
            .iter()
            .filter_map(|a| match self.engine.by_handle(a.handle) {
                Some(crate::engine::EngineData::Effect(e)) => Some(e.name.clone()),
                _ => None,
            })
            .filter(|n| !n.is_empty() && n != "EffectVisualEffect")
            .map(|n| {
                let bare = n.trim_start_matches("Effect");
                let bad = ["Decrease", "Damage", "Stun", "Paralyze", "Poison", "Slow", "Death", "Choke", "Horrified", "Fear", "Confused", "Sleep", "Entangle", "Blind"]
                    .iter()
                    .any(|k| bare.contains(k));
                // "AttackIncrease" -> "Attack Increase".
                let mut label = String::new();
                for (i, ch) in bare.chars().enumerate() {
                    if i > 0 && ch.is_ascii_uppercase() {
                        label.push(' ');
                    }
                    label.push(ch);
                }
                (label, !bad)
            })
            .collect()
    }

    // ---------------------------------------------------------------
    // Journal
    // ---------------------------------------------------------------

    fn journal_db(&mut self) -> Arc<JournalDb> {
        if let Some(j) = &self.sheet.journal {
            return j.clone();
        }
        let mut db = JournalDb::default();
        if let Ok(g) = k2_data::read_gff(&self.res, "global", ResourceType::Jrl) {
            for c in g.root.list("Categories") {
                db.categories.push(JournalCategory {
                    tag: c.string_or_empty("Tag").to_ascii_lowercase(),
                    name: c.locstring("Name"),
                    priority: c.uint("Priority").unwrap_or(4),
                    planet: c.int_or("PlanetID", -1),
                    entries: c
                        .list("EntryList")
                        .iter()
                        .map(|e| JournalEntryDef {
                            id: e.uint("ID").unwrap_or(0),
                            end: e.flag("End"),
                            text: e.locstring("Text"),
                            xp_percentage: e.float_or("XP_Percentage", 0.0),
                        })
                        .collect(),
                });
            }
        }
        let db = Arc::new(db);
        self.sheet.journal = Some(db.clone());
        db
    }

    /// Quests with a journal state, newest update first.
    pub fn journal_view(&mut self) -> Vec<QuestView> {
        let db = self.journal_db();
        let mut out: Vec<QuestView> = Vec::new();
        let states: Vec<(String, i32)> = self.journal.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for (tag, state) in states {
            if state <= 0 {
                continue;
            }
            let Some(cat) = db.categories.iter().find(|c| c.tag == tag) else { continue };
            let entry = cat.entries.iter().find(|e| e.id == state as u32);
            let resolve = |w: &World, s: &LocString| w.substitute(&w.talk.as_ref().map(|t| t.resolve(s)).unwrap_or_default());
            out.push(QuestView {
                tag: tag.clone(),
                name: resolve(self, &cat.name),
                text: entry.map(|e| resolve(self, &e.text)).unwrap_or_default(),
                state,
                completed: entry.is_some_and(|e| e.end),
                priority: cat.priority,
                planet: cat.planet,
                updated: self.sheet.journal_updated.get(&tag).copied().unwrap_or(0.0),
            });
        }
        out.sort_by(|a, b| b.updated.partial_cmp(&a.updated).unwrap_or(std::cmp::Ordering::Equal).then(a.name.cmp(&b.name)));
        out
    }

    // ---------------------------------------------------------------
    // Item use
    // ---------------------------------------------------------------

    /// Spells (spells.2da rows) an item casts when used.
    pub fn item_spells(&mut self, item: ObjectId) -> Vec<i32> {
        let Some(o) = self.get(item).filter(|o| o.kind == ObjectKind::Item) else { return Vec::new() };
        let template = o.template.clone();
        let Ok(g) = k2_data::read_gff(&self.res, &template, ResourceType::Uti) else { return Vec::new() };
        g.root
            .list("PropertiesList")
            .iter()
            .filter(|p| p.uint("PropertyName") == Some(u32::from(PROPERTY_CAST_SPELL)))
            .filter_map(|p| p.int("Subtype"))
            .collect()
    }

    /// Whether the inventory screen's Use button applies.
    pub fn is_usable_item(&mut self, item: ObjectId) -> bool {
        !self.item_spells(item).is_empty()
    }

    /// Uses an item (medpac, stim, ...) on `target`: runs each CastSpell
    /// property's impact script with `user` as OBJECT_SELF, then takes one
    /// unit of the stack. Returns the spells cast.
    pub fn use_item(&mut self, user: ObjectId, item: ObjectId, target: ObjectId) -> Result<Vec<i32>, String> {
        let spells = self.item_spells(item);
        if spells.is_empty() {
            return Err("item has no use".into());
        }
        let target = if target == OBJECT_INVALID { user } else { target };
        let table = self.tables.get(&self.res, "spells").map_err(|e| e.to_string())?;
        for &spell in &spells {
            let row = usize::try_from(spell).unwrap_or(usize::MAX);
            let Some(script) = table.resref(row, "impactscript") else { continue };
            if let Some(sound) = table.resref(row, "castsound") {
                self.emit(WorldEvent::PlaySound { object: user, sound });
            }
            self.sheet.casts.push(SpellCast { spell, caster: user, target, item });
            self.run_script(&script, user, ScriptCtx::default());
            self.sheet.casts.pop();
        }
        self.pump();
        let left = {
            let Some(it) = self.get_mut(item).and_then(|o| o.item_mut()) else { return Ok(spells) };
            it.stack_size -= 1;
            it.stack_size
        };
        if left <= 0 {
            self.detach_item(item);
            if let Some(o) = self.get_mut(item) {
                o.destroyed = true;
            }
        }
        Ok(spells)
    }

    /// Spell and journal routines. None = not handled here.
    pub(crate) fn sheet_call(&mut self, name: &str, _me: ObjectId, a: &[Value]) -> Option<Value> {
        if name == "AddJournalQuestEntry" {
            // Timestamp for the journal's "newest first" order; host.rs
            // still applies the entry.
            if let Some(Value::String(plot)) = a.first() {
                self.sheet.journal_updated.insert(plot.to_ascii_lowercase(), self.time);
            }
            return None;
        }
        let cast = *self.sheet.casts.last()?;
        Some(match name {
            "GetSpellId" => Value::Int(cast.spell),
            "GetSpellTargetObject" | "GetSpellTarget" => Value::Object(cast.target),
            "GetSpellCastItem" => Value::Object(cast.item),
            "GetLastSpellCaster" => Value::Object(cast.caster),
            _ => return None,
        })
    }
}
