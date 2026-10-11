use std::io::Write;
use super::*;

/// The action bar slots an object adds (headless: 101per from the install).
fn bar_for(w: &mut k2_world::World, obj: ObjectId) -> [Vec<Entry>; SLOTS] {
    let leader = w.party_leader();
    let mut slots: [Vec<Entry>; SLOTS] = Default::default();
    object_entries(w, leader, obj, &mut slots);
    slots
}

#[test]
fn creature_entries_talk_to_friends_and_leave_enemies_to_attack() {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = k2_res::Install::discover(dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP creature_entries_talk_to_friends_and_leave_enemies_to_attack: no install (set K2_INSTALL)");
        return;
    };
    let mut w = k2_world::World::new(install).unwrap();
    w.enter_module("101per", None).unwrap();
    let leader = w.party_leader();
    let living: Vec<ObjectId> = w.iter().filter(|o| o.id != leader && o.creature().is_some_and(|c| !c.dead)).map(|o| o.id).collect();
    let (mut talks, mut enemies) = (0, 0);
    for id in living {
        let conversation = w.get(id).and_then(|o| o.conversation.clone()).is_some_and(|s| !s.is_empty());
        let slots = bar_for(&mut w, id);
        if w.is_enemy(leader, id) {
            enemies += 1;
            assert!(slots.iter().all(|s| s.is_empty()), "hostile {} gets no quick action", w.describe(id));
        } else if conversation {
            talks += 1;
            let first = slots[0].first().expect("a conversation offers Talk");
            assert_eq!((first.name.as_str(), first.icon.as_deref(), first.ty), ("Talk", Some("gui_mp_talku"), ENTRY_USE_OBJECT));
        }
    }
    assert!(talks > 0 && enemies > 0, "101per has friendly talkers ({talks}) and enemies ({enemies})");
}

#[test]
fn door_use_entry_has_the_original_use_cursor_icon() {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = k2_res::Install::discover(dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP door_use_entry_has_the_original_use_cursor_icon: no install (set K2_INSTALL)");
        return;
    };
    let mut w = k2_world::World::new(install).unwrap();
    w.enter_module("101per", None).unwrap();
    let doors: Vec<ObjectId> = w.iter().filter(|o| o.door().is_some_and(|d| !d.is_static)).map(|o| o.id).collect();
    let icons: Vec<String> = doors.into_iter().flat_map(|d| bar_for(&mut w, d)[0].clone()).filter(|e| e.ty == ENTRY_USE_OBJECT).filter_map(|e| e.icon).collect();
    assert!(!icons.is_empty(), "101per has a usable door");
    assert!(icons.iter().all(|i| i == "gui_mp_useu"), "{icons:?}");
}

/// Every medpac, stim, grenade and shield template gives the quick bar an
/// entry that shows the item's own icon, and that icon decodes (see
/// `assert_icon_decodes`). Each template runs in a fresh world: an earlier
/// item in the pack would supply the spell's icon. Then every item in the
/// party's inventory in a save folder named by K2_ICON_SAVE (opt-in: unset
/// SKIPs; set but without a timed save fails).
#[test]
fn medpac_stim_grenade_and_shield_items_show_their_icons_on_the_bar() {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = k2_res::Install::discover(dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP medpac_stim_grenade_and_shield_items_show_their_icons_on_the_bar: no install (set K2_INSTALL)");
        return;
    };
    let res = k2_res::Resources::open(install.clone()).unwrap();
    let mut stock: Vec<String> = vec!["g_i_medeqpmnt01", "g_i_medeqpmnt02", "g_i_medeqpmnt03", "g_i_medeqpmnt04", "g_i_medeqpmnt08"].into_iter().map(String::from).collect();
    stock.extend((1..=6).map(|i| format!("g_i_adrnaline{i:03}")));
    stock.extend((1..=3).map(|i| format!("g_i_cmbtshot{i:03}")));
    stock.extend(["g_w_adhsvgren001", "g_w_cryobgren001", "g_w_firegren001", "g_w_fraggren01", "g_w_iongren01", "g_w_poisngren01", "g_w_sonicgren01", "g_w_stungren01", "g_w_thermldet01"].map(String::from));
    stock.extend((1..=7).map(|i| format!("a_shield_{i:02}")));
    stock.extend((1..=6).map(|i| format!("d_shield_{i:02}")));
    for t in &stock {
        let mut w = k2_world::World::new(install.clone()).unwrap();
        w.enter_module("101per", None).unwrap();
        let leader = w.party_leader();
        let icon = w.item_def(t).and_then(|d| d.icon.clone()).unwrap_or_else(|| panic!("{t}: no icon"));
        w.give_new_item(t, leader, 1, true).unwrap_or_else(|| panic!("{t}: not a template"));
        // Grenades are offered on an enemy, so the bar is read on the leader and on one enemy.
        let ids: Vec<ObjectId> = w.iter().filter(|o| o.creature().is_some() && o.id != leader).map(|o| o.id).collect();
        let enemy = ids.into_iter().find(|&id| w.is_enemy(leader, id));
        let mut bar = w.usable_talents(leader, leader);
        if let Some(e) = enemy {
            bar.extend(w.usable_talents(leader, e));
        }
        let shown: Vec<(&str, Option<&str>)> = bar.iter().map(|v| (v.name.as_str(), v.icon.as_deref())).collect();
        assert!(bar.iter().any(|v| v.icon.as_deref() == Some(icon.as_str())), "{t}: no quick bar entry shows {icon}; the bar has {shown:?}");
        assert_icon_decodes(&res, &icon);
    }
    // Opt-in: K2_ICON_SAVE names a save folder (the one holding timed/).
    let Ok(dir) = std::env::var("K2_ICON_SAVE") else {
        let _ = writeln!(std::io::stderr(), "SKIP save inventory icons: set K2_ICON_SAVE to a save folder");
        return;
    };
    let data = k2_world::save::read_slot(std::path::Path::new(&dir), "timed").unwrap_or_else(|e| panic!("K2_ICON_SAVE={dir}: no timed save ({e:?})"));
    let mut sw = k2_world::World::load(install, &data).unwrap();
    for member in sw.party.clone() {
        for item in sw.inventory_of(member) {
            let Some(d) = sw.item_def_of(item) else {
                let what = sw.get(item).map_or("gone".to_owned(), |o| format!("{:?} tag {:?} template {:?}", o.kind, o.tag, o.template));
                panic!("K2_ICON_SAVE={dir}: inventory object {item} ({what}) has no item definition");
            };
            let icon = d.icon.clone().unwrap_or_else(|| panic!("{} in the save: no icon", d.template));
            assert_icon_decodes(&res, &icon);
        }
    }
}

/// `icon` is a TPC that decodes to its header's size, with every mip level
/// complete in the file. `decode_tpc` zero-fills a truncated base level, so
/// the payload is checked against the header here, and the decoded RGBA
/// must be exactly the base level (w*h*4 bytes, first) plus its mips.
fn assert_icon_decodes(res: &k2_res::Resources, icon: &str) {
    let bytes = res.get(&k2_res::ResKey::new(icon, rakata_core::ResourceType::Tpc)).ok().flatten().unwrap_or_else(|| panic!("{icon}: no TPC"));
    let d = crate::tpc::decode_tpc(&bytes).unwrap_or_else(|| panic!("{icon}: the TPC does not decode"));
    let (w, h) = (u32::from(u16::from_le_bytes([bytes[8], bytes[9]])), u32::from(u16::from_le_bytes([bytes[10], bytes[11]])));
    assert_eq!((d.width, d.height), (w, h), "{icon}: decoded size differs from the header");
    assert!(d.mips >= 1, "{icon}: no mip level");
    let data_size = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let base_file = match (data_size != 0, bytes[12]) {
        (false, 1) => (w * h) as usize,
        (false, 2) => (w * h * 3) as usize,
        (false, 4) => (w * h * 4) as usize,
        (true, 4) => (w.div_ceil(4) * h.div_ceil(4) * 16) as usize,
        _ => (w.div_ceil(4) * h.div_ceil(4) * 8) as usize,
    };
    assert!(bytes.len() >= 128 + base_file, "{icon}: base level truncated in the file ({} of {base_file} bytes)", bytes.len().saturating_sub(128));
    let (mut lw, mut lh, mut rgba) = (w, h, 0usize);
    for _ in 0..d.mips {
        rgba += (lw * lh * 4) as usize;
        lw = (lw / 2).max(1);
        lh = (lh / 2).max(1);
    }
    assert_eq!(d.data.len(), rgba, "{icon}: decoded RGBA is not the base level plus {} mips", d.mips - 1);
}
