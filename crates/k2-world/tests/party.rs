//! Party selection and leader switching against the real install. Skips
//! (passes) when no install is found at K2_INSTALL or ../..

use k2_res::Install;
use k2_world::{AvailableNpc, World};

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

fn slot(tpl: &str) -> AvailableNpc {
    AvailableNpc { template: tpl.into(), selectable: true, ..Default::default() }
}

#[test]
fn selection_roster_and_leader_switching() {
    let Some(mut w) = world() else { return };
    w.enter_module("103per", None).unwrap();
    let pc = w.player;
    w.available_npcs.insert(6, slot("p_kreia"));
    w.available_npcs.insert(1, slot("p_baodur"));
    w.available_npcs.insert(0, slot("p_atton"));

    let roster = w.party_roster();
    assert_eq!(roster.iter().map(|r| r.npc).collect::<Vec<_>>(), vec![0, 1, 6]);
    assert!(roster.iter().all(|r| !r.name.is_empty() && !r.in_party), "{roster:?}");

    // Three is one too many.
    assert!(w.apply_party_selection(&[0, 1, 6]).is_err());
    w.apply_party_selection(&[6, 1]).unwrap();
    assert_eq!(w.party.len(), 3);
    let kreia = w.available_npcs[&6].object.unwrap();
    assert!(w.party.contains(&kreia));

    // Kreia takes a hit, then is left behind and brought back: same state.
    w.get_mut(kreia).unwrap().creature_mut().unwrap().hp = 5;
    w.apply_party_selection(&[1, 0]).unwrap();
    assert!(!w.party.contains(&kreia) && w.get(kreia).is_none());
    assert!(w.available_npcs[&6].stored.is_some());
    w.apply_party_selection(&[6]).unwrap();
    assert_eq!(w.available_npcs[&6].object, Some(kreia));
    assert_eq!(w.get(kreia).unwrap().creature().unwrap().hp, 5, "stored state lost");
    assert_eq!(w.party.len(), 2);

    // Tab cycles control and comes back round.
    assert_eq!(w.party_leader(), pc);
    let next = w.cycle_party_leader().unwrap();
    assert_eq!(next, kreia);
    assert_eq!(w.party_leader(), kreia);
    assert_eq!(w.cycle_party_leader(), Some(pc));
    // Direct pick (HUD portrait).
    assert!(w.set_party_leader(kreia));
    assert_eq!(w.party_leader(), kreia);
    assert!(w.set_party_leader(pc));

    // Unset AI style: party Jedi support, others aggressive, strangers 0.
    assert_eq!(w.default_npc_ai_style(kreia), 13);
    assert_eq!(w.default_npc_ai_style(pc), 0);
}
