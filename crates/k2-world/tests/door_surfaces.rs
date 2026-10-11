//! Door walk surfaces in 101per, against the real install. Ignored diagnostic:
//! run with --ignored --nocapture; it asserts nothing. For each door it reports
//! whether the DWK for the door's model (`{model}0/1/2`) loads, how many of its
//! faces are walkable, and the walk regions on the two sides of the door before
//! and after every door is opened. The walk grid takes nodes only from room faces,
//! so an open door's own walk faces should not join regions by themselves.

use glam::Vec3;
use k2_res::ResKey;
use k2_walk::Materials;
use k2_world::{ObjectKind, World};
use rakata_core::ResourceType;

mod support;

use support::{install, ring};

/// One letter per walk region seen on a ring 1.5 m round `centre`: the same
/// letter means the same region, `.` a point with no walk ground.
fn sides(w: &World, centre: Vec3) -> String {
    let mut reps: Vec<Vec3> = Vec::new();
    let mut out = String::new();
    for q in ring(centre.truncate(), 1.5) {
        let (x, y) = (q.x, q.y);
        let Some(h) = w.walk.ground(x, y, centre.z) else {
            out.push('.');
            continue;
        };
        let p = Vec3::new(x, y, h.z);
        match reps.iter().position(|&r| w.walk.same_region(r, p)) {
            Some(i) => out.push((b'A' + i as u8) as char),
            None => {
                reps.push(p);
                out.push((b'A' + (reps.len() - 1) as u8) as char);
            }
        }
    }
    out
}

#[test]
#[ignore]
fn door_walk_surfaces_in_101per() {
    let Some(install) = install() else { return };
    let mut w = World::new(install).expect("the install loads into a world");
    w.enter_module("101per", None).unwrap();
    w.drain_events();
    let surfacemat = w.res.get(&ResKey::new("surfacemat", ResourceType::TwoDa)).unwrap().expect("surfacemat.2da is in the install");
    let mats = Materials::from_2da_bytes(&surfacemat).expect("surfacemat.2da parses");
    let doors: Vec<_> = w
        .iter()
        .filter(|o| o.kind == ObjectKind::Door)
        .map(|o| (o.id, o.tag.clone(), o.position, o.door().and_then(|d| d.model.clone())))
        .collect();
    println!("{} doors in 101per", doors.len());
    for (_, tag, pos, model) in &doors {
        let Some(model) = model else {
            println!("{tag} at {pos:?}: no model");
            continue;
        };
        for i in 0..3 {
            let name = format!("{model}{i}");
            match w.res.get(&ResKey::new(&name, ResourceType::Dwk)) {
                Ok(Some(bytes)) => {
                    let bwm = rakata_formats::read_bwm_from_bytes(&bytes).expect("the DWK parses");
                    let walk: Vec<_> = bwm.faces.iter().filter(|f| mats.walkable(f.material_id)).collect();
                    let pts: Vec<Vec3> = walk
                        .iter()
                        .flat_map(|f| f.vertex_indices.map(|v| bwm.vertices[v as usize]))
                        .map(|p| Vec3::new(p.x, p.y, p.z))
                        .collect();
                    let (lo, hi) = pts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), &p| (lo.min(p), hi.max(p)));
                    println!("{tag} {name}: {} faces, {} walkable, walkable bounds {lo:?}..{hi:?}", bwm.faces.len(), walk.len());
                }
                Ok(None) => println!("{tag} {name}: not found"),
                Err(e) => println!("{tag} {name}: error {e:?}"),
            }
        }
        println!("{tag} at {pos:?} sides closed {}", sides(&w, *pos));
    }

    let pc = w.party_leader();
    for (id, ..) in &doors {
        w.set_door_open(*id, true, pc);
    }
    for _ in 0..120 {
        w.tick(1.0 / 30.0);
    }
    for (_, tag, pos, _) in &doors {
        println!("{tag} at {pos:?} sides open {}", sides(&w, *pos));
    }
    let start = w.by_tag("WP_player_start", 0).and_then(|s| w.get(s)).map(|s| s.position).expect("101per has WP_player_start");
    let exit = w.by_tag("To_102PER", 0).and_then(|s| w.get(s)).map(|s| s.position).expect("101per has To_102PER");
    println!("with every door open: entry {start:?} and exit trigger {exit:?} same walk region {}", w.walk.same_region(start, exit));
}
