//! Survey of body armor resolution for appearance rows. Usage: armorsurvey [ROW...]
//! Without rows, uses every portraits.2da forpc appearance.
use k2_data::tables::texture_exists;
use k2_data::Tables;
use k2_res::{Install, ResKey, Resources};
use rakata_core::ResourceType;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let install = Install::discover(std::env::var("K2_INSTALL").unwrap_or_else(|_| "..".into())).unwrap();
    let res = Resources::open(install).unwrap();
    let t = Tables::new();
    let mut rows: Vec<u32> = a.iter().filter_map(|s| s.parse().ok()).collect();
    if rows.is_empty() {
        let p = t.get(&res, "portraits").unwrap();
        for r in 0..p.rows() {
            if p.int(r, "forpc") == Some(1) {
                for c in ["appearance_s", "appearance_l", "appearancenumber"] {
                    if let Some(v) = p.int(r, c) {
                        rows.push(v as u32);
                    }
                }
            }
        }
        rows.sort();
        rows.dedup();
    }
    let app = t.get(&res, "appearance").unwrap();
    let mdl = |n: &str| res.locate(&ResKey::new(n, ResourceType::Mdl)).is_some();
    for row in rows {
        println!("== appearance {row} {:?} modeltype {:?} head {:?}", app.str(row as usize, "label"), app.str(row as usize, "modeltype"), app.int(row as usize, "normalhead"));
        for l in 'a'..='n' {
            let v = l;
            let m = app.resref(row as usize, &format!("model{v}"));
            let tx = app.resref(row as usize, &format!("tex{v}"));
            let mut miss = vec![];
            for tv in 1..=15u32 {
                if let Some(tx) = &tx {
                    let n = format!("{tx}{tv:02}");
                    if !texture_exists(&res, &n) {
                        miss.push(tv);
                    }
                }
            }
            println!("  {v}: model {:?} ({}) tex {:?} missing variations {:?}", m, m.as_deref().is_some_and(mdl), tx, miss);
        }
    }
}
