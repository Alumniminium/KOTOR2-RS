//! Looting the 101per corpse wakes Kreia: a_setkrespwn fades out, swaps the
//! corpse placeable for the stunt creature (KreiaStuntman, p_kreiastunt) and
//! starts 101kreia; entries 41-43 are the get-up cutscene, then a_cut_kreia
//! creates the standing Kreia who talks to the PC. Skips (passes) when no
//! install is found.

use k2_res::Install;
use k2_world::World;

const DT: f32 = 1.0 / 30.0;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

/// Degrees between `id`'s facing and the direction to `to`.
fn face_err(w: &World, id: u32, to: u32) -> f32 {
    let (a, b) = (w.get(id).unwrap(), w.get(to).unwrap());
    let d = (b.position - a.position).truncate().normalize_or_zero();
    let [x, y] = k2_data::module::dir_from_bearing(a.bearing);
    (d.x * x + d.y * y).clamp(-1.0, 1.0).acos().to_degrees()
}

#[test]
fn kreia_wakes_from_the_corpse_and_faces_the_player() {
    let Some(mut w) = world() else { return };
    w.enter_module("101per", None).unwrap();
    // Play through the opening sequence.
    for _ in 0..(90.0 / DT) as usize {
        w.tick(DT);
        if w.conversation.is_some() {
            w.advance_conversation();
        }
    }
    assert!(w.conversation.is_none() && w.player_controlled());
    let pc = w.party_leader();
    let corpse = w.iter().find(|o| o.template.eq_ignore_ascii_case("g_tresgencorp004")).map(|o| o.id).expect("loot corpse");
    let cp = w.get(corpse).unwrap().position;
    w.place(pc, cp + glam::Vec3::new(1.0, 0.0, 0.0));
    w.open_container(pc, corpse).unwrap();
    w.close_container(corpse, pc);

    let mut first: Vec<usize> = Vec::new();
    let mut second: Vec<usize> = Vec::new();
    let mut stunt_model = None;
    let mut last = (String::new(), None);
    let (mut overlap, mut worst) = (0usize, 0usize);
    let stage = w.by_tag("WP_KREIA_SPAWN1", 0).and_then(|id| w.get(id)).map(|o| o.position).expect("Kreia's stage");
    for _ in 0..(90.0 / DT) as usize {
        w.tick(DT);
        // Kreia models on the stage: the corpse placeable, the stunt (until a
        // script moves it off the stage) and the standing creature. The swap
        // from one to the next must not show two at once (the screen is
        // black for a moment around the hand-over, so a few ticks pass).
        let on_stage = w
            .iter()
            .filter(|o| !o.destroyed && ["kreia_corpse", "KreiaStuntman", "Kreia"].contains(&o.tag.as_str()) && (o.position - stage).truncate().length() < 4.0)
            .count();
        overlap = if on_stage > 1 { overlap + 1 } else { 0 };
        worst = worst.max(overlap);
        let Some(c) = &w.conversation else {
            if !second.is_empty() {
                break;
            }
            continue;
        };
        let owner_tag = w.get(c.owner).unwrap().tag.clone();
        let (owner, entry) = (c.owner, c.entry);
        if (owner_tag.clone(), entry) != last {
            last = (owner_tag.clone(), entry);
            let e = entry.expect("a line");
            if owner_tag == "KreiaStuntman" {
                stunt_model = w.get(owner).and_then(|o| o.creature()).and_then(|c| c.body_model.clone());
                first.push(e);
                // The get-up entry is staged (nobody turns); the lines after
                // it (the shot from Kreia's spot) have the PC face her.
                if e != 41 {
                    assert!(face_err(&w, pc, owner) < 15.0, "entry {e}: PC faces the stunt Kreia");
                }
            } else if owner_tag == "Kreia" {
                second.push(e);
                // Every line of the regular talk: PC and Kreia face each other.
                assert!(face_err(&w, pc, owner) < 15.0, "entry {e}: PC faces Kreia ({:.0} deg off)", face_err(&w, pc, owner));
                assert!(face_err(&w, owner, pc) < 15.0, "entry {e}: Kreia faces the PC");
            }
        }
        // Stand in for the UI timer; take the first real reply.
        if w.conversation.as_ref().is_some_and(|c| c.shown_for > 0.3) {
            match w.conversation_view() {
                Some(v) if v.replies.first().is_some_and(|r| !r.text.trim().is_empty()) => w.choose_reply(0),
                _ => w.advance_conversation(),
            }
        }
    }
    assert!(worst <= 3, "two Kreia models on the stage for {worst} ticks");
    assert_eq!(first, vec![41, 42, 43], "get-up cutscene entries");
    assert_eq!(stunt_model.as_deref().map(str::to_ascii_lowercase).as_deref(), Some("p_kreiastunt"));
    assert!(second.len() >= 8, "the standing Kreia talks: {second:?}");
}
