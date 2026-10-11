//! Routine usage scan: `rscan PATTERN...` counts ACTION calls whose routine
//! name contains any pattern, across chitin and every module, and lists the
//! scripts that use them.
use std::collections::{BTreeMap, BTreeSet};

use k2_res::{Install, Resources};
use k2_script::{Ncs, Op};
use rakata_core::ResourceType;

fn main() {
    let pats: Vec<String> = std::env::args().skip(1).map(|p| p.to_ascii_lowercase()).collect();
    let install = Install::discover(std::env::var("K2_INSTALL").unwrap_or_else(|_| "..".into())).unwrap();
    let mut res = Resources::open(install.clone()).unwrap();
    let routines = k2_script::load_routines(&res).unwrap();
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    let mut users: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut scan = |res: &Resources, keys: Vec<k2_res::ResKey>, module: &str| {
        for k in keys {
            if k.ty != ResourceType::Ncs.type_id() || !seen.insert((module.to_owned(), k.name.clone())) {
                continue;
            }
            let Ok(Some(b)) = res.get(&k) else { continue };
            let Ok(ncs) = Ncs::parse(&k.name, &b) else { continue };
            for i in &ncs.instrs {
                if let Op::Action { routine, .. } = i.op {
                    let name = routines.get(routine).map_or("?".to_owned(), |r| r.name.clone());
                    let l = name.to_ascii_lowercase();
                    if pats.iter().any(|p| l.contains(p.as_str())) {
                        *count.entry(name.clone()).or_default() += 1;
                        users.entry(name).or_default().insert(format!("{module}:{}", k.name));
                    }
                }
            }
        }
    };
    let ck: Vec<_> = res.chitin_keys().cloned().collect();
    scan(&res, ck, "chitin");
    for m in install.module_names() {
        if res.load_module(&m).is_err() {
            continue;
        }
        let mk: Vec<_> = res.module_keys().cloned().collect();
        scan(&res, mk, &m);
    }
    let mut v: Vec<_> = count.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    for (n, c) in v {
        let u = &users[&n];
        println!("{c:5} {n}  [{}] {}", u.len(), u.iter().take(8).cloned().collect::<Vec<_>>().join(" "));
    }
}
