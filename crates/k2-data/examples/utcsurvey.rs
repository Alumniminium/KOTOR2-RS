//! Survey of creature body variation fields over all module GIT creatures.
//! Prints one line per distinct (template, appearance, body var, tex var, armor).
use k2_data::{Creature, Item, Module, Tables};
use k2_res::{Install, Resources};
use std::collections::BTreeSet;

fn main() {
    let install = Install::discover(std::env::var("K2_INSTALL").unwrap_or_else(|_| "..".into())).unwrap();
    let modules = install.module_names();
    let mut res = Resources::open(install).unwrap();
    let t = Tables::new();
    let app = t.get(&res, "appearance").unwrap();
    let base = t.get(&res, "baseitems").unwrap();
    let mut seen = BTreeSet::new();
    for name in &modules {
        let Ok(m) = Module::load(&mut res, name) else { continue };
        for c in &m.git.creatures {
            let Ok(b) = Creature::load(&res, &c.template) else { continue };
            if !seen.insert(c.template.clone()) {
                continue;
            }
            let mt = app.str(b.appearance as usize, "modeltype").unwrap_or("").to_string();
            let armor = b.equipment.iter().find(|(s, _)| *s == 2).map(|(_, r)| r.clone());
            let (item_desc, item_bv) = match armor.as_deref().and_then(|a| Item::load(&res, a).ok()) {
                Some(i) => (format!("{} base {} bv {} tv {}", armor.clone().unwrap(), i.base_item, i.body_variation, i.texture_variation), base.str(i.base_item as usize, "bodyvar").unwrap_or("").to_string()),
                None => (String::new(), String::new()),
            };
            let mdls = b.models(&res, &t).ok();
            println!(
                "{name} {} app {} type {mt} utc_bv {} utc_tv {} armor [{item_desc}] bodyvar {item_bv} -> {:?} {:?} head {:?}",
                c.template, b.appearance, b.body_variation, b.texture_variation,
                mdls.as_ref().map(|m| m.body.clone()), mdls.as_ref().and_then(|m| m.body_texture.clone()), mdls.as_ref().and_then(|m| m.head.clone())
            );
        }
    }
}
