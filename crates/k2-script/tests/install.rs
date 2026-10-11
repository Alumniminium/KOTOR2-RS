//! Tests against the real install. They skip (pass with a note) when no
//! install is found at K2_INSTALL or `../..` (the crate dir is two levels
//! below the game folder's kotor2-rs checkout).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use k2_res::{Install, ResKey, Resources};
use k2_script::{load_routines, DefaultValue, Ncs, StubHost, Vm};
use rakata_core::ResourceType;

fn resources() -> Option<Resources> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").to_owned());
    let install = Install::discover(&dir).ok()?;
    Resources::open(install).ok()
}

/// Every NCS in chitin, then every module's capsules, as (origin, key).
fn all_scripts(res: &mut Resources) -> Vec<(String, ResKey, Vec<u8>)> {
    let ncs = ResourceType::Ncs.type_id();
    let mut out = Vec::new();
    let mut keys: Vec<ResKey> = res.chitin_keys().filter(|k| k.ty == ncs).cloned().collect();
    keys.sort();
    for k in keys {
        if let Ok(Some(b)) = res.get(&k) {
            out.push(("chitin".to_owned(), k, b));
        }
    }
    for m in res.install.module_names() {
        if res.load_module(&m).is_err() {
            continue;
        }
        let mut keys: Vec<ResKey> = res.module_keys().filter(|k| k.ty == ncs).cloned().collect();
        keys.sort();
        keys.dedup();
        for k in keys {
            if let Ok(Some(b)) = res.get(&k) {
                out.push((m.clone(), k, b));
            }
        }
    }
    out
}

#[test]
fn routine_table() {
    let Some(res) = resources() else {
        eprintln!("no install found, skipping");
        return;
    };
    let r = load_routines(&res).expect("nwscript.nss");
    assert_eq!(r.len(), 877, "K2 declares 877 engine routines");
    assert_eq!(r.engine_structures, ["effect", "event", "location", "talent"]);
    let unparsed: Vec<String> = r
        .routines
        .iter()
        .flat_map(|rt| rt.params.iter().map(move |p| (rt, p)))
        .filter_map(|(rt, p)| match &p.default {
            Some(DefaultValue::Unparsed(s)) => Some(format!("{}.{} = {s}", rt.name, p.name)),
            _ => None,
        })
        .collect();
    assert!(unparsed.is_empty(), "unparsed defaults: {unparsed:?}");
    assert_eq!(r.get(0).map(|x| x.name.as_str()), Some("Random"));
    assert_eq!(r.by_name("DelayCommand").map(|x| x.params.len()), Some(2));
}

#[test]
fn disassemble_and_run_everything() {
    let Some(mut res) = resources() else {
        eprintln!("no install found, skipping");
        return;
    };
    let routines = load_routines(&res).expect("nwscript.nss");
    let scripts = all_scripts(&mut res);
    let vm = Vm::new(&routines);

    let mut parsed = 0usize;
    let mut parse_fail: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut ran = 0usize;
    let mut run_fail: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut resumed = 0usize;
    let mut seen = BTreeSet::new();
    let mut conditionals = 0usize;
    let mut junk = Vec::new();

    for (origin, key, bytes) in &scripts {
        let label = format!("{origin}/{}", key.name);
        let ncs = match Ncs::parse(&key.name, bytes) {
            Ok(n) => Arc::new(n),
            Err(e) => {
                parse_fail.entry(class(&e.to_string())).or_default().push(label);
                continue;
            }
        };
        parsed += 1;
        // Identical bytes in several capsules only need one run.
        if !seen.insert(bytes.clone()) {
            continue;
        }
        if key.name.starts_with("k_con_") {
            conditionals += 1;
        }
        let mut host = StubHost::default();
        ran += 1;
        match vm.run(&ncs, &mut host, 0x100) {
            Err(e) => {
                run_fail.entry(class(&e.kind.to_string())).or_default().push(format!("{label}@{:#x}", e.offset));
                continue;
            }
            Ok(o) => {
                if key.name.starts_with("k_con_") && !matches!(o.ret, Some(k2_script::Value::Int(_))) {
                    run_fail.entry("conditional without int return".into()).or_default().push(label.clone());
                }
                if o.leftover > 0 {
                    junk.push(format!("{label}:{}", o.leftover));
                }
            }
        }
        // Deferred blocks (DelayCommand / AssignCommand / ActionDoCommand).
        let mut rounds = 0;
        while !host.deferred.is_empty() && rounds < 4 {
            rounds += 1;
            for state in std::mem::take(&mut host.deferred) {
                resumed += 1;
                if let Err(e) = vm.resume(&state, &mut host) {
                    run_fail
                        .entry(format!("resume: {}", class(&e.kind.to_string())))
                        .or_default()
                        .push(format!("{label}@{:#x}", e.offset));
                }
            }
        }
    }

    println!(
        "scripts {} parsed {} unique-run {} (k_con_* {}) resumed-blocks {}",
        scripts.len(),
        parsed,
        ran,
        conditionals,
        resumed
    );
    println!("scripts leaving compiler junk on the stack: {} {:?}", junk.len(), &junk[..junk.len().min(5)]);
    for (c, w) in &parse_fail {
        println!("parse x{} {c}: {:?}", w.len(), &w[..w.len().min(5)]);
    }
    for (c, w) in &run_fail {
        println!("run x{} {c}: {:?}", w.len(), &w[..w.len().min(5)]);
    }
    assert!(parse_fail.is_empty(), "parse failures");
    assert!(run_fail.is_empty(), "run failures");
}

/// Error message with numbers stripped, for grouping.
fn class(msg: &str) -> String {
    let mut out = String::new();
    let mut last_digit = false;
    for c in msg.chars() {
        if c.is_ascii_hexdigit() && (last_digit || c.is_ascii_digit()) {
            if !last_digit {
                out.push('N');
            }
            last_digit = true;
        } else {
            last_digit = false;
            out.push(c);
        }
    }
    out.chars().take(160).collect()
}
