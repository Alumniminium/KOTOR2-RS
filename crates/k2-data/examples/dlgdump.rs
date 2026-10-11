//! Flat dump of every DLG node with speaker, camera, VO, scripts, anims and
//! stunts. Usage: dlgdump RESREF [MODULE]  (install from K2_INSTALL or ..)
use k2_data::{Dialog, Talk};
use k2_res::{Install, Resources};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let install = Install::discover(std::env::var("K2_INSTALL").unwrap_or_else(|_| "..".into())).unwrap();
    let talk = Talk::load(&install).unwrap();
    let mut res = Resources::open(install).unwrap();
    if let Some(m) = a.get(2) {
        res.load_module(m).unwrap();
    }
    let d = Dialog::load(&res, &a[1]).unwrap();
    println!("camera_model {:?} animated_cut {} stunts {:?}", d.camera_model, d.animated_cut, d.stunts);
    for (i, s) in d.starting.iter().enumerate() {
        println!("START{i} -> E{} active {:?}", s.index, s.active.as_ref().map(|c| &c.resref));
    }
    for (kind, list) in [("E", &d.entries), ("R", &d.replies)] {
        for (i, n) in list.iter().enumerate() {
            let text: String = talk.resolve(&n.text).chars().take(70).collect();
            println!(
                "{kind}{i} spk={:?} lst={:?} cam={} id={:?} camanim={:?} vo={:?} scr={:?} scr2={:?} anims={:?} -> {:?} | {}",
                n.speaker,
                n.listener,
                n.camera_angle,
                n.camera_id,
                n.camera_animation,
                n.vo_resref,
                n.script.as_ref().map(|s| &s.resref),
                n.script2.as_ref().map(|s| &s.resref),
                n.anims.iter().map(|x| (x.participant.as_str(), x.animation)).collect::<Vec<_>>(),
                n.links.iter().map(|l| l.index).collect::<Vec<_>>(),
                text
            );
        }
    }
}
