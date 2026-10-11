//! Derived combat numbers for a creature: ability modifiers, base attack
//! bonus, Defense, and the equipped weapons and armour.
//!
//! A simplification of the KotOR d20 rules: BAB from each class's
//! attackbonustable (cls_atk_*.2da), Defense = 10 + dex mod (capped by the
//! armour's max dex) + natural AC + armour base AC + AC properties + class
//! Defense (acbonus.2da; the droid classes share it with the droid Combat
//! Logic feats, counted once). Blaster deflection (Jedi Defense, Deflect)
//! is carried as `deflection`. Other feats and force powers reach Defense
//! only through the effects in powers.rs.

use k2_data::GffExt;
use k2_script::ObjectId;
use rakata_core::ResourceType;

use crate::items::{ArmorInfo, ItemInfo, WeaponInfo};
use crate::World;

pub const SLOT_BODY: i32 = 1;
pub const SLOT_RIGHTWEAPON: i32 = 4;
pub const SLOT_LEFTWEAPON: i32 = 5;
pub const SLOT_CWEAPON_L: i32 = 14;
pub const SLOT_CWEAPON_R: i32 = 15;
pub const SLOT_CWEAPON_B: i32 = 16;
pub const SLOT_CARMOUR: i32 = 17;
/// Second weapon set (the inactive pair; `World::swap_weapon_sets`). Not an
/// INVENTORY_SLOT_* constant: the engine keeps it internally.
pub const SLOT_RIGHTWEAPON2: i32 = 18;
pub const SLOT_LEFTWEAPON2: i32 = 19;

/// feat.2da rows behind the Defense and deflection rules.
const FEAT_LOGIC_UPGRADE_COMBAT: i32 = 110;
const FEAT_LOGIC_UPGRADE_TACTICIAN: i32 = 111;
const FEAT_LOGIC_UPGRADE_BATTLE_DROID: i32 = 112;
const FEAT_JEDI_DEFENSE: i32 = 55;
const FEAT_ADVANCED_JEDI_DEFENSE: i32 = 1;
const FEAT_MASTER_JEDI_DEFENSE: i32 = 24;
const FEAT_DEFLECT: i32 = 168;

/// classes.2da rows of the droid classes. Their armorclasscolumn is SCD, the
/// same ladder as the Combat Logic feats.
const CLASS_COMBAT_DROID: i32 = 6;
const CLASS_EXPERT_DROID: i32 = 7;

fn is_droid_class(class: i32) -> bool {
    matches!(class, CLASS_COMBAT_DROID | CLASS_EXPERT_DROID)
}

/// Per-template UTC fields the world's CreatureState does not keep.
#[derive(Debug, Clone, Copy, Default)]
pub struct TemplateInfo {
    pub natural_ac: i32,
    pub lootable: bool,
    pub no_perm_death: bool,
    pub disable_fade: bool,
}

#[derive(Debug, Clone)]
pub struct CombatStats {
    pub abilities: [u8; 6],
    pub level: i32,
    pub bab: i32,
    pub defense: i32,
    pub main: WeaponInfo,
    pub offhand: Option<WeaponInfo>,
    pub armor: ArmorInfo,
    /// From EffectModifyAttacks and combat feats (powers.rs).
    pub extra_attacks: u32,
    /// Attack bonus (or penalty) from equipped non-weapon items: creature
    /// hides, gloves, implants (AttackBonus / ToHitPenalty properties).
    pub gear_attack: i32,
    /// Bonus to the opposed roll that deflects a blaster bolt (Jedi Defense,
    /// Deflect). None when the creature has no deflection feat.
    pub deflection: Option<i32>,
}

impl CombatStats {
    pub fn modifier(&self, ability: usize) -> i32 {
        (i32::from(self.abilities[ability]) - 10).div_euclid(2)
    }

    pub fn str_mod(&self) -> i32 {
        self.modifier(0)
    }

    pub fn dex_mod(&self) -> i32 {
        self.modifier(1)
    }

    /// Attack bonus with the main (or off) hand.
    pub fn attack_bonus(&self, offhand: bool) -> i32 {
        let w = if offhand { self.offhand.as_ref().unwrap_or(&self.main) } else { &self.main };
        let ability = if w.ranged { self.dex_mod() } else { self.str_mod() };
        // Two weapons: -4 main, -8 off (light off-hand rules not modelled).
        let twf = match (self.offhand.is_some(), offhand) {
            (false, _) => 0,
            (true, false) => -4,
            (true, true) => -8,
        };
        self.bab + ability + w.attack_bonus + twf + self.gear_attack
    }

    /// Attacks per 3 s round: one, plus one with an off-hand weapon, plus
    /// one at BAB 11 or more.
    pub fn attacks_per_round(&self) -> u32 {
        1 + u32::from(self.offhand.is_some()) + u32::from(self.bab >= 11) + self.extra_attacks
    }
}

impl World {
    pub(crate) fn template_info(&mut self, template: &str) -> TemplateInfo {
        if template.is_empty() {
            return TemplateInfo::default();
        }
        if let Some(t) = self.combat.templates.get(template) {
            return *t;
        }
        let info = k2_data::read_gff(&self.res, template, ResourceType::Utc)
            .map(|g| TemplateInfo {
                natural_ac: g.root.int("NaturalAC").unwrap_or(0),
                lootable: g.root.flag("Lootable"),
                no_perm_death: g.root.flag("NoPermDeath"),
                disable_fade: g.root.flag("DisableFade"),
            })
            .unwrap_or_default();
        self.combat.templates.insert(template.to_owned(), info);
        info
    }

    /// Template properties plus those of installed upgrades (craft.rs).
    pub(crate) fn item_info(&mut self, item: ObjectId) -> Option<ItemInfo> {
        let mut info = self.template_item_info(item)?;
        info.properties.extend(self.upgrade_properties(item));
        Some(info)
    }

    fn template_item_info(&mut self, item: ObjectId) -> Option<ItemInfo> {
        let template = self.get(item)?.template.clone();
        if let Some(i) = self.combat.items.get(&template) {
            return i.clone();
        }
        let info = ItemInfo::load(&self.res, &template).or_else(|| {
            // Created items without a UTI (scripts): base item only.
            self.get(item).and_then(|o| o.item()).map(|it| ItemInfo { base_item: it.base_item, properties: Vec::new() })
        });
        self.combat.items.insert(template, info.clone());
        info
    }

    /// One class's term of acbonus.2da: the column named by its
    /// armorclasscolumn, at row level - 1.
    fn class_ac_term(&self, class: i32, level: i32) -> i32 {
        let (Ok(cls), Ok(acb)) = (self.tables.get(&self.res, "classes"), self.tables.get(&self.res, "acbonus")) else {
            return 0;
        };
        let Some(col) = cls.str(class as usize, "armorclasscolumn") else { return 0 };
        acb.int((level - 1).min(50) as usize, &col.to_ascii_lowercase()).unwrap_or(0)
    }

    /// Class Defense, summed over the non-droid classes like BAB. Row
    /// convention checked against the droid Combat Logic text (feat 110: +2
    /// at levels 1-5, +4 at 6-11, +6 at 12-17), which matches the Scoundrel
    /// column of acbonus.2da exactly. Droid classes: see `droid_defense`.
    fn class_defense(&self, classes: &[(i32, i32)]) -> i32 {
        classes
            .iter()
            .filter(|&&(class, lvl)| lvl > 0 && !is_droid_class(class))
            .map(|&(class, lvl)| self.class_ac_term(class, lvl))
            .sum()
    }

    /// Droid Defense: the droid classes' SCD term and the Combat Logic feats
    /// are one bonus, so the larger applies, never the sum. Feat 111 and 112
    /// replace the feat below them (TLK 38674, 38675), so the highest owned
    /// one sets the value: 112 +6, 111 +4, else 110 at 2 + 2 * (level / 6)
    /// (TLK 38673). Pending an in-game sheet reading of HK-47 / T3-M4 Defense
    /// (this rule gives HK-47 +4 and T3-M4 +2; the old sum gave +8 and +4).
    fn droid_defense(&mut self, id: ObjectId, classes: &[(i32, i32)], level: i32) -> i32 {
        let class_term = classes
            .iter()
            .filter(|&&(class, lvl)| lvl > 0 && is_droid_class(class))
            .map(|&(class, lvl)| self.class_ac_term(class, lvl))
            .max()
            .unwrap_or(0);
        let logic = if self.has_feat(id, FEAT_LOGIC_UPGRADE_BATTLE_DROID) {
            6
        } else if self.has_feat(id, FEAT_LOGIC_UPGRADE_TACTICIAN) {
            4
        } else if self.has_feat(id, FEAT_LOGIC_UPGRADE_COMBAT) {
            2 + 2 * (level / 6)
        } else {
            0
        };
        class_term.max(logic)
    }

    fn class_bab(&self, class: i32, level: i32) -> i32 {
        let Ok(classes) = self.tables.get(&self.res, "classes") else { return level };
        let table = classes.resref(class as usize, "attackbonustable").unwrap_or_else(|| "cls_atk_1".into());
        self.tables
            .get(&self.res, &table)
            .ok()
            .and_then(|t| t.int((level - 1).max(0) as usize, "bab"))
            .unwrap_or(level)
    }

    /// Current combat numbers of a creature (None for non-creatures).
    pub fn combat_stats(&mut self, id: ObjectId) -> Option<CombatStats> {
        let o = self.get(id)?;
        let c = o.creature()?;
        let abilities = c.abilities;
        let classes = c.classes.clone();
        let equipped = c.equipped.clone();
        let template = o.template.clone();

        let mut level = 0;
        let mut bab = 0;
        for &(class, lvl) in &classes {
            level += lvl.max(0);
            bab += self.class_bab(class, lvl.max(1));
        }
        if level == 0 {
            level = 1;
            bab = 1;
        }

        let weapon_in = |w: &mut Self, slot: i32| -> Option<WeaponInfo> {
            let item = *equipped.get(&slot)?;
            w.item_info(item)?.weapon(&w.res, &w.tables)
        };
        let main = weapon_in(self, SLOT_RIGHTWEAPON)
            .or_else(|| weapon_in(self, SLOT_CWEAPON_R))
            .or_else(|| weapon_in(self, SLOT_CWEAPON_B))
            .or_else(|| weapon_in(self, SLOT_CWEAPON_L))
            .unwrap_or_else(WeaponInfo::unarmed);
        let offhand = weapon_in(self, SLOT_LEFTWEAPON).filter(|_| main.wield != 0);

        let mut armor = ArmorInfo::default();
        for slot in [SLOT_BODY, SLOT_CARMOUR] {
            if let Some(item) = equipped.get(&slot).copied()
                && let Some(info) = self.item_info(item)
            {
                let a = info.armor(&self.res, &self.tables);
                armor.base_ac += a.base_ac;
                armor.ac_bonus += a.ac_bonus;
                armor.max_dex = match (armor.max_dex, a.max_dex) {
                    (Some(x), Some(y)) => Some(x.min(y)),
                    (x, y) => x.or(y),
                };
            }
        }
        // Other equipped items (belts, implants, gloves) can carry AC too.
        for (&slot, &item) in &equipped {
            if slot == SLOT_BODY || slot == SLOT_CARMOUR || (4..=5).contains(&slot) || (14..=16).contains(&slot) || slot >= SLOT_RIGHTWEAPON2 {
                continue;
            }
            if let Some(info) = self.item_info(item) {
                armor.ac_bonus += info.armor(&self.res, &self.tables).ac_bonus;
            }
        }

        // Ability and attack properties on worn items (itempropdef 0 Ability,
        // 19 DecreaseAbilityScore, 38 AttackBonus, 41/8 attack penalties).
        // The mining droids' creature hide, for one, takes 5 off Dex and 5
        // off every attack; without it they are far too accurate.
        let mut abilities = abilities;
        let mut gear_attack = 0;
        for (&slot, &item) in &equipped {
            if (4..=5).contains(&slot) || (14..=16).contains(&slot) || slot >= SLOT_RIGHTWEAPON2 {
                continue;
            }
            let Some(info) = self.item_info(item) else { continue };
            for p in &info.properties {
                let bonus = ItemInfo::bonus(&self.res, &self.tables, p.cost_value);
                let ability = usize::from(p.subtype).min(5);
                match p.property {
                    0 => abilities[ability] = (i32::from(abilities[ability]) + bonus).clamp(1, 255) as u8,
                    19 => abilities[ability] = (i32::from(abilities[ability]) - bonus).clamp(1, 255) as u8,
                    38 => gear_attack += bonus,
                    8 | 41 => gear_attack -= bonus,
                    _ => {}
                }
            }
        }
        let natural_ac = self.template_info(&template).natural_ac;
        let dex_mod = (i32::from(abilities[1]) - 10).div_euclid(2);
        let dex = armor.max_dex.map_or(dex_mod, |m| dex_mod.min(m));
        let class_def = self.class_defense(&classes);
        let droid_def = self.droid_defense(id, &classes, level);
        let defense = 10 + dex + natural_ac + armor.base_ac + armor.ac_bonus + class_def + droid_def;
        // Blaster deflection: Jedi Defense (55, +0), Advanced (1, +3), Master
        // (24, +6), a chain where each replaces the one before (TLK 1179,
        // 1126, 1148). Any of them, with a lightsaber in hand, gives the
        // highest. Weapon Master Deflect (+1, plus 1 per two levels after
        // 1st). Stacking is not in the feat text: the better one applies
        // (guess).
        let lightsaber = main.lightsaber || offhand.as_ref().is_some_and(|w| w.lightsaber);
        let jedi = [(FEAT_MASTER_JEDI_DEFENSE, 6), (FEAT_ADVANCED_JEDI_DEFENSE, 3), (FEAT_JEDI_DEFENSE, 0)]
            .into_iter()
            .filter(|&(feat, _)| self.has_feat(id, feat))
            .map(|(_, bonus)| bonus)
            .max();
        let mut deflection = if lightsaber { jedi } else { None };
        if self.has_feat(id, FEAT_DEFLECT) {
            let deflect = 1 + (level - 1) / 2;
            deflection = Some(deflection.map_or(deflect, |d| d.max(deflect)));
        }
        let mut stats = CombatStats { abilities, level, bab, defense, main, offhand, armor, extra_attacks: 0, gear_attack, deflection };
        self.apply_power_modifiers(id, &mut stats);
        Some(stats)
    }
}
