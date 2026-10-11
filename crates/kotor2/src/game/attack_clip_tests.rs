use std::io::Write;
use super::*;

/// What the client plays for a ranged attack: the attack's clip list
/// (k2_world attack_clip_list, as AttackStarted carries it) through
/// AnimSet::resolve, the chain the Bevy side uses on the real models.
#[test]
fn the_client_plays_a_blaster_clip_for_ranged_attacks_on_area_bodies() {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = k2_res::Install::discover(dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP the_client_plays_a_blaster_clip_for_ranged_attacks_on_area_bodies: no install (set K2_INSTALL)");
        return;
    };
    let res = k2_res::Resources::open(install).unwrap();
    let mut cache = AssetCache::default();
    let mut images = Assets::<Image>::default();
    let mut meshes = Assets::<Mesh>::default();
    let mut materials = Assets::<KMaterial>::default();
    let mut loader = Loader { res: &res, cache: &mut cache, images: &mut images, meshes: &mut meshes, materials: &mut materials };
    // Iziz cantina (503ond) bodies and the Peragus party.
    let bodies = [
        "p_mandalorebb", "p_hk47", "pmbhm", "n_commf", "n_commm", "n_bith", "n_duros", "n_ondcomm1", "n_twilekf", "n_rodian",
        "n_quarren", "n_swoopgang", "n_sullustan", "n_walrusman",
    ];
    for body in bodies {
        let set = AnimSet::resolve(&mut loader, body).unwrap_or_else(|| panic!("{body} loads"));
        for wield in [4, 5, 6] {
            for k in [1, 2] {
                let names = k2_world::combat::attack_clip_list(wield, true, false, k, false);
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                let chosen = first_clip(&set, &names).unwrap_or_else(|| panic!("{body} wield {wield}: no clip in {names:?}"));
                let clip = set.clip(&chosen).unwrap();
                let melee = clip.events.iter().any(|(_, e)| e.starts_with("swing") || e == "hit" || e == "clash");
                assert!(!melee, "{body} wield {wield} attack {k} plays {chosen}, a melee clip: {:?}", clip.events);
                assert!(!chosen.starts_with('g') && !chosen.starts_with('m') && !chosen.starts_with("throw"), "{body} plays {chosen}");
            }
        }
    }
}
