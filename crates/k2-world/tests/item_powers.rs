//! Item powers on the quick bar (medpacs, stims, grenades, shields) show the
//! icon of the item that casts them: spells.2da has no icon for these rows.
//! Skips when no install is found.

use std::io::Write;
use k2_res::Install;
use k2_world::World;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(&dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP item powers test: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    Some(World::new(install).unwrap_or_else(|e| panic!("{dir}: the install does not load into a world: {e:?}")))
}

/// The quick bar's entries for each item template, once it is in the leader's
/// pack: (template, [(name, icon)]) for the entries whose icon is the item's.
fn bar_icons(w: &mut World, template: &str, icon: &str) -> Vec<String> {
    let leader = w.party_leader();
    w.give_new_item(template, leader, 1, true).unwrap_or_else(|| panic!("{template} is not a template"));
    let talents = w.usable_talents(leader, leader);
    talents.into_iter().filter(|t| t.icon.as_deref() == Some(icon)).map(|t| t.name).collect()
}

#[test]
fn a_medpac_on_the_quick_bar_shows_the_medpac_icon() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    let names = bar_icons(&mut w, "g_i_medeqpmnt01", "ii_medeqpmnt_002");
    assert!(names.iter().any(|n| n.starts_with("Medpac")), "no medpac entry with the medpac icon: {names:?}");
}

#[test]
fn stims_on_the_quick_bar_show_their_item_icons() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    assert!(!bar_icons(&mut w, "g_i_adrnaline001", "ii_adrnaline_001").is_empty(), "adrenaline shot has no entry with its icon");
    assert!(!bar_icons(&mut w, "g_i_cmbtshot001", "ii_cmbtshot_001").is_empty(), "combat shot has no entry with its icon");
}
