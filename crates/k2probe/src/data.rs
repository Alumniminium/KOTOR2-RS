//! k2-data backed subcommands: module, dlg, gff, survey-data.

use std::collections::BTreeMap;

use k2_data::module::Module;
use k2_data::{Creature, Dialog, Door, Encounter, Placeable, SoundBlueprint, Tables, Talk, Trigger, Waypoint};
use k2_res::{Install, Resources};
use rakata_core::ResourceType;

type R<T> = Result<T, Box<dyn std::error::Error>>;

pub fn gff(install: Install, rest: &[String]) -> R<()> {
    let name = rest.first().ok_or("gff needs NAME.EXT [MODULE]")?;
    let (stem, ext) = name.rsplit_once('.').ok_or("gff needs NAME.EXT")?;
    let mut res = Resources::open(install)?;
    if let Some(m) = rest.get(1) {
        res.load_module(m)?;
    }
    // K2_GFF_MAXLIST raises the per-list entry cap (default 4).
    let max_list = std::env::var("K2_GFF_MAXLIST").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    print!("{}", k2_data::dump_gff(&res, stem, ResourceType::from_extension(&ext.to_ascii_lowercase()), max_list)?);
    Ok(())
}

pub fn module(install: Install, rest: &[String]) -> R<()> {
    let name = rest.first().ok_or("module needs NAME")?;
    let talk = Talk::load(&install)?;
    let mut res = Resources::open(install)?;
    let tables = Tables::new();
    let m = Module::load(&mut res, name)?;
    let i = &m.info;
    println!("module {} \"{}\" area {} \"{}\"", m.name, talk.resolve(&i.name), m.area.resref, talk.resolve(&m.area.name));
    println!(
        "entry ({:.2}, {:.2}, {:.2}) bearing {:.3} rad, OnModLoad {:?}, area OnEnter {:?}",
        i.entry_position[0], i.entry_position[1], i.entry_position[2], i.entry_bearing,
        i.script("Mod_OnModLoad"), m.area.script("OnEnter")
    );
    println!(
        "rooms {} (lyt) / {} (are), doorhooks {}, vis rooms {}, ambient {:?}, fog {:?}",
        m.layout.rooms.len(), m.area.rooms.len(), m.layout.door_hooks.len(), m.vis.rooms.len(), m.area.dynamic_ambient, m.area.fog
    );
    let g = &m.git;
    println!(
        "git: creatures {} doors {} placeables {} waypoints {} triggers {} sounds {} stores {} encounters {} cameras {}",
        g.creatures.len(), g.doors.len(), g.placeables.len(), g.waypoints.len(), g.triggers.len(),
        g.sounds.len(), g.stores.len(), g.encounters.len(), g.cameras.len()
    );
    for c in g.creatures.iter().take(4) {
        match Creature::load(&res, &c.template).and_then(|b| Ok((b.models(&res, &tables)?, b))) {
            Ok((mdl, b)) => println!(
                "  creature {:<18} {:<20} body {} tex {:?} head {:?} conv {:?}",
                c.template, talk.resolve(&b.first_name), mdl.body, mdl.body_texture, mdl.head, b.conversation
            ),
            Err(e) => println!("  creature {} ERROR {e}", c.template),
        }
    }
    for d in g.doors.iter().take(3) {
        match Door::load(&res, &d.template).and_then(|b| Ok((b.model(&res, &tables)?, b))) {
            Ok((mdl, b)) => println!("  door {:<20} tag {:<16} model {mdl} \"{}\" linked {:?}/{}", d.template, d.tag, talk.resolve(&b.name), d.linked_to_module, d.linked_to),
            Err(e) => println!("  door {} ERROR {e}", d.template),
        }
    }
    for p in g.placeables.iter().take(3) {
        match Placeable::load(&res, &p.template).and_then(|b| Ok((b.model(&res, &tables)?, b))) {
            Ok((mdl, b)) => println!("  placeable {:<16} model {mdl} \"{}\" useable {}", p.template, talk.resolve(&b.name), b.useable),
            Err(e) => println!("  placeable {} ERROR {e}", p.template),
        }
    }
    for t in g.triggers.iter().filter(|t| t.linked_to_module.is_some()).take(3) {
        println!("  transition trigger {} -> {:?} / {:?} ({} pts)", t.template, t.linked_to_module, t.linked_to, t.geometry.len());
    }
    Ok(())
}

/// Prints a 2DA as tab-separated rows: `2da NAME [COLUMN...]`.
pub fn twoda(install: Install, rest: &[String]) -> R<()> {
    let name = rest.first().ok_or("2da needs NAME [COLUMN...]")?;
    let tables = Tables::new();
    let res = Resources::open(install)?;
    let t = tables.get(&res, name)?;
    let cols: Vec<String> = if rest.len() > 1 { rest[1..].to_vec() } else { t.columns() };
    println!("row\t{}", cols.join("\t"));
    for r in 0..t.rows() {
        let cells: Vec<&str> = cols.iter().map(|c| t.str(r, c).unwrap_or("****")).collect();
        println!("{}\t{}", t.row_label(r).unwrap_or(""), cells.join("\t"));
    }
    Ok(())
}

pub fn dlg(install: Install, rest: &[String]) -> R<()> {
    let name = rest.first().ok_or("dlg needs NAME [MODULE]")?;
    let talk = Talk::load(&install)?;
    let mut res = Resources::open(install)?;
    if let Some(m) = rest.get(1) {
        res.load_module(m)?;
    }
    let d = Dialog::load(&res, name)?;
    println!(
        "dlg {} entries {} replies {} starts {} skippable {} camera {:?} end {:?}",
        d.resref, d.entries.len(), d.replies.len(), d.starting.len(), d.skippable, d.camera_model, d.end_conversation
    );
    for p in d.check() {
        println!("  PROBLEM {p}");
    }
    let mut seen = vec![false; d.entries.len()];
    for s in &d.starting {
        println!("START -> E{}{}", s.index, cond_str(s));
        print_entry(&d, &talk, s.index, 1, &mut seen);
    }
    Ok(())
}

fn cond_str(l: &k2_data::dlg::Link) -> String {
    let mut out = String::new();
    for c in [&l.active, &l.active2].into_iter().flatten() {
        out.push_str(&format!(" [{}{}{:?}]", if c.not { "!" } else { "" }, c.resref, c.params));
    }
    out
}

fn print_entry(d: &Dialog, talk: &Talk, e: usize, depth: usize, seen: &mut [bool]) {
    let Some(node) = d.entries.get(e) else { return };
    let pad = "  ".repeat(depth);
    let who = if node.speaker.is_empty() { "OWNER" } else { &node.speaker };
    println!("{pad}E{e} {who}: {}{}", short(&talk.resolve(&node.text)), node.script.as_ref().map(|s| format!(" (script {})", s.resref)).unwrap_or_default());
    if seen[e] {
        println!("{pad}  ...(already shown)");
        return;
    }
    seen[e] = true;
    for l in &node.links {
        let Some(r) = d.replies.get(l.index) else { continue };
        println!("{pad}  R{} > {}{}", l.index, short(&talk.resolve(&r.text)), cond_str(l));
        for rl in &r.links {
            print_entry(d, talk, rl.index, depth + 2, seen);
        }
    }
}

fn short(s: &str) -> String {
    let s = s.replace(['\n', '\r'], " ");
    if s.chars().count() > 90 { format!("{}...", s.chars().take(90).collect::<String>()) } else if s.is_empty() { "[continue]".into() } else { s }
}

#[derive(Default)]
struct Fails {
    ok: usize,
    by_class: BTreeMap<String, Vec<String>>,
}

impl Fails {
    fn add<T, E: std::fmt::Display>(&mut self, what: &str, r: Result<T, E>) -> Option<T> {
        match r {
            Ok(v) => {
                self.ok += 1;
                Some(v)
            }
            Err(e) => {
                let msg = e.to_string();
                // Group by message with the resource name replaced.
                let class = msg.split(':').next_back().unwrap_or(&msg).trim().to_owned();
                let class = if class.ends_with("not found") { "not found".to_owned() } else { class };
                self.by_class.entry(class).or_default().push(format!("{what} ({msg})"));
                None
            }
        }
    }
}

pub fn survey_data(install: Install, rest: &[String]) -> R<()> {
    let talk = Talk::load(&install)?;
    println!("dialog.tlk {} strings", talk.len());
    let modules: Vec<String> = if rest.is_empty() { install.module_names() } else { rest.to_vec() };
    let mut res = Resources::open(install)?;
    let tables = Tables::new();
    let mut f: BTreeMap<&str, Fails> = BTreeMap::new();
    for name in &modules {
        let Some(m) = f.entry("module").or_default().add(name, Module::load(&mut res, name)) else { continue };
        let g = &m.git;
        for c in &g.creatures {
            let w = format!("{name}/{}.utc", c.template);
            if let Some(b) = f.entry("utc").or_default().add(&w, Creature::load(&res, &c.template)) {
                if let Some(m) = f.entry("utc models").or_default().add(&w, b.models(&res, &tables)) {
                    f.entry("mdl exists").or_default().add(&w, exists(&res, &m.body, &[ResourceType::Mdl]));
                    if let Some(h) = &m.head {
                        f.entry("mdl exists").or_default().add(&w, exists(&res, h, &[ResourceType::Mdl]));
                    }
                    if let Some(t) = &m.body_texture {
                        f.entry("tex exists").or_default().add(&w, exists(&res, t, &[ResourceType::Tpc, ResourceType::Tga]));
                    }
                }
                if let Some(conv) = &b.conversation {
                    check_dlg(&res, &mut f, name, conv);
                }
            }
        }
        for d in &g.doors {
            let w = format!("{name}/{}.utd", d.template);
            if let Some(b) = f.entry("utd").or_default().add(&w, Door::load(&res, &d.template)) {
                if let Some(m) = f.entry("utd models").or_default().add(&w, b.model(&res, &tables)) {
                    f.entry("mdl exists").or_default().add(&w, exists(&res, &m, &[ResourceType::Mdl]));
                }
                if let Some(conv) = &b.conversation {
                    check_dlg(&res, &mut f, name, conv);
                }
            }
        }
        for p in &g.placeables {
            let w = format!("{name}/{}.utp", p.template);
            if let Some(b) = f.entry("utp").or_default().add(&w, Placeable::load(&res, &p.template)) {
                if let Some(m) = f.entry("utp models").or_default().add(&w, b.model(&res, &tables)) {
                    f.entry("mdl exists").or_default().add(&w, exists(&res, &m, &[ResourceType::Mdl]));
                }
                if let Some(conv) = &b.conversation {
                    check_dlg(&res, &mut f, name, conv);
                }
            }
        }
        for t in &g.triggers {
            f.entry("utt").or_default().add(&format!("{name}/{}.utt", t.template), Trigger::load(&res, &t.template));
        }
        for w in &g.waypoints {
            f.entry("utw").or_default().add(&format!("{name}/{}.utw", w.template), Waypoint::load(&res, &w.template));
        }
        for s in &g.sounds {
            f.entry("uts").or_default().add(&format!("{name}/{}.uts", s.template), SoundBlueprint::load(&res, &s.template));
        }
        for r in &m.layout.rooms {
            f.entry("mdl exists").or_default().add(&format!("{name}/room"), exists(&res, &r.model, &[ResourceType::Mdl]));
            f.entry("wok exists").or_default().add(&format!("{name}/room"), exists(&res, &r.model, &[ResourceType::Wok]));
        }
        // Every DLG shipped in the module capsules, referenced or not.
        let dlgs: Vec<String> = res.module_keys().filter(|k| k.ty == ResourceType::Dlg.type_id()).map(|k| k.name.clone()).collect();
        for d in dlgs {
            check_dlg(&res, &mut f, name, &d);
        }
    }
    println!("{:<12} {:>7} {:>7}", "kind", "ok", "fail");
    for (k, v) in &f {
        println!("{k:<12} {:>7} {:>7}", v.ok, v.by_class.values().map(Vec::len).sum::<usize>());
    }
    for (k, v) in &f {
        for (class, who) in &v.by_class {
            let mut uniq = who.clone();
            uniq.sort();
            uniq.dedup();
            println!("\n[{k}] x{} {class}", who.len());
            println!("  e.g. {}", uniq.iter().take(if std::env::var_os("K2_VERBOSE").is_some() { usize::MAX } else { 4 }).cloned().collect::<Vec<_>>().join("; "));
        }
    }
    Ok(())
}

fn check_dlg(res: &Resources, f: &mut BTreeMap<&str, Fails>, module: &str, resref: &str) {
    let w = format!("{module}/{resref}.dlg");
    if let Some(d) = f.entry("dlg").or_default().add(&w, Dialog::load(res, resref)) {
        let problems = d.check();
        let r: Result<(), String> = if problems.is_empty() { Ok(()) } else { Err(format!("bad links: {}", problems.join(", "))) };
        f.entry("dlg links").or_default().add(&w, r);
    }
}

fn exists(res: &Resources, name: &str, types: &[ResourceType]) -> Result<(), String> {
    if types.iter().any(|t| res.locate(&k2_res::ResKey::new(name, *t)).is_some()) {
        Ok(())
    } else {
        Err(format!("{name}.{} not found", types[0].extension()))
    }
}

/// Encounter survey (`encounters [MODULE...]`): one line per GIT encounter
/// with its UTE fields, then histograms of the field values.
pub fn encounters(install: Install, rest: &[String]) -> R<()> {
    let modules: Vec<String> = if rest.is_empty() { install.module_names() } else { rest.to_vec() };
    let mut res = Resources::open(install)?;
    let mut hist: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut count = |field: &str, value: String| *hist.entry(field.to_owned()).or_default().entry(value).or_default() += 1;
    let mut total = 0;
    for name in &modules {
        let Ok(m) = Module::load(&mut res, name) else { continue };
        for e in &m.git.encounters {
            total += 1;
            let bp = match Encounter::load(&res, &e.template) {
                Ok(b) => b,
                Err(err) => {
                    println!("{name} {} ERROR {err}", e.template);
                    count("ute", "missing".into());
                    continue;
                }
            };
            let scripts: Vec<&str> = bp.scripts.iter().map(|(l, _)| l.as_str()).collect();
            let crs: Vec<f32> = bp.creatures.iter().map(|c| c.cr).collect();
            println!(
                "{name} {:<18} pos ({:.1},{:.1},{:.1}) geo {} spawns {} active {} diff {} faction {} max {} rec {} playeronly {} reset {} resettime {} respawns {} spawnopt {} creatures {} single {} guaranteed {} cr {:?} scripts {:?}",
                e.template, e.position[0], e.position[1], e.position[2], e.geometry.len(), e.spawn_points.len(),
                bp.active as u8, bp.difficulty_index, bp.faction, bp.max_creatures, bp.rec_creatures, bp.player_only as u8,
                bp.reset as u8, bp.reset_time, bp.respawns, bp.spawn_option, bp.creatures.len(),
                bp.creatures.iter().filter(|c| c.single_spawn).count(),
                bp.creatures.iter().map(|c| c.guaranteed_count).sum::<i32>(), crs, scripts
            );
            count("active", bp.active.to_string());
            count("difficulty_index", bp.difficulty_index.to_string());
            count("faction", bp.faction.to_string());
            count("max_creatures", bp.max_creatures.to_string());
            count("rec_creatures", bp.rec_creatures.to_string());
            count("player_only", bp.player_only.to_string());
            count("reset", bp.reset.to_string());
            count("reset_time", bp.reset_time.to_string());
            count("respawns", bp.respawns.to_string());
            count("spawn_option", bp.spawn_option.to_string());
            count("creature_count", bp.creatures.len().to_string());
            count("geometry_points", e.geometry.len().to_string());
            count("spawn_points", e.spawn_points.len().to_string());
            count("guaranteed_sum", bp.creatures.iter().map(|c| c.guaranteed_count).sum::<i32>().to_string());
            for c in &bp.creatures {
                count("creature_cr", format!("{:.1}", c.cr));
                count("creature_single_spawn", c.single_spawn.to_string());
                count("creature_guaranteed", c.guaranteed_count.to_string());
            }
            for (label, _) in &bp.scripts {
                count("script", label.clone());
            }
            if bp.scripts.is_empty() {
                count("script", "none".into());
            }
        }
    }
    println!("\nencounters {total}");
    for (field, values) in &hist {
        let line: Vec<String> = values.iter().map(|(v, n)| format!("{v}={n}")).collect();
        println!("{field:<22} {}", line.join(" "));
    }
    Ok(())
}
