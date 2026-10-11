//! k2probe: inspect a KotOR2 install through k2-res and measure how much of it
//! the K1-targeted rakata parsers can read.
//!
//! Usage:
//!   k2probe [--install DIR] info
//!   k2probe [--install DIR] modules
//!   k2probe [--install DIR] ls [MODULE]
//!   k2probe [--install DIR] cat NAME.EXT [MODULE] > out
//!   k2probe [--install DIR] survey [MODULE...]
//!   k2probe [--install DIR] gff NAME.EXT [MODULE]
//!   k2probe [--install DIR] module NAME
//!   k2probe [--install DIR] dlg NAME [MODULE]
//!   k2probe [--install DIR] survey-data [MODULE...]
//!   k2probe [--install DIR] lights MODULE

mod data;

use std::collections::BTreeMap;
use std::io::Write;
use std::process::ExitCode;

use k2_res::{Install, ResKey, Resources};
use rakata_core::ResourceType;

const DEFAULT_INSTALL: &str = "..";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut install_dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| DEFAULT_INSTALL.to_owned());
    if args.first().is_some_and(|a| a == "--install") && args.len() >= 2 {
        install_dir = args[1].clone();
        args.drain(..2);
    }
    match run(&install_dir, &args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(install_dir: &str, args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let install = Install::discover(install_dir)?;
    let cmd = args.first().map(String::as_str).unwrap_or("info");
    let rest = args.get(1..).unwrap_or(&[]);
    match cmd {
        "info" => {
            let res = Resources::open(install.clone())?;
            println!("root          {}", install.root.display());
            println!("bifs          {}", res.bif_count());
            println!("chitin        {}", res.chitin_keys().count());
            println!("override      {}", res.override_keys().count());
            println!("texturepacks  {}", res.texpack_keys().count());
            println!("modules       {}", install.module_names().len());
        }
        "modules" => {
            for m in install.module_names() {
                println!("{m}");
            }
        }
        "ls" => {
            let mut res = Resources::open(install)?;
            let mut keys: Vec<ResKey> = match rest.first() {
                Some(m) => {
                    res.load_module(m)?;
                    res.module_keys().cloned().collect()
                }
                None => res.chitin_keys().cloned().collect(),
            };
            keys.sort();
            for k in keys {
                println!("{k}");
            }
        }
        "cat" => {
            let name = rest.first().ok_or("cat needs NAME.EXT")?;
            let (stem, ext) = name.rsplit_once('.').ok_or("cat needs NAME.EXT")?;
            let mut res = Resources::open(install)?;
            if let Some(m) = rest.get(1) {
                res.load_module(m)?;
            }
            let key = ResKey::new(stem, ResourceType::from_extension(&ext.to_ascii_lowercase()));
            let bytes = res.get(&key)?.ok_or_else(|| format!("{key} not found"))?;
            std::io::stdout().write_all(&bytes)?;
        }
        "survey" => survey(install, rest)?,
        "gff" => data::gff(install, rest)?,
        "module" => data::module(install, rest)?,
        "dlg" => data::dlg(install, rest)?,
        "2da" => data::twoda(install, rest)?,
        "survey-data" => data::survey_data(install, rest)?,
        "encounters" => data::encounters(install, rest)?,
        "lights" => lights(install, rest)?,
        other => return Err(format!("unknown command {other}").into()),
    }
    Ok(())
}

#[derive(Default)]
struct Tally {
    ok: usize,
    fail: usize,
    errors: BTreeMap<String, Vec<String>>,
}

impl Tally {
    fn record(&mut self, what: &ResKey, r: Result<(), String>) {
        match r {
            Ok(()) => self.ok += 1,
            Err(e) => {
                self.fail += 1;
                // Group by message with digits stripped so one bug shows once.
                let class: String = e.chars().filter(|c| !c.is_ascii_digit()).take(140).collect();
                self.errors.entry(class).or_default().push(what.to_string());
            }
        }
    }
}

fn survey(install: Install, modules: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut res = Resources::open(install.clone())?;
    let mut tallies: BTreeMap<&'static str, Tally> = BTreeMap::new();

    let global: Vec<ResKey> = res.chitin_keys().chain(res.texpack_keys()).cloned().collect();
    probe_keys(&res, &global, &mut tallies);

    let modules: Vec<String> = if modules.is_empty() { install.module_names() } else { modules.to_vec() };
    for m in &modules {
        res.load_module(m)?;
        let keys: Vec<ResKey> = res.module_keys().cloned().collect();
        probe_keys(&res, &keys, &mut tallies);
    }

    println!("{:<6} {:>7} {:>7}", "format", "ok", "fail");
    for (fmt, t) in &tallies {
        println!("{fmt:<6} {:>7} {:>7}", t.ok, t.fail);
    }
    for (fmt, t) in &tallies {
        for (class, who) in &t.errors {
            println!("\n[{fmt}] x{} {class}", who.len());
            println!("  e.g. {}", who.iter().take(4).cloned().collect::<Vec<_>>().join(", "));
        }
    }
    Ok(())
}

fn probe_keys(res: &Resources, keys: &[ResKey], tallies: &mut BTreeMap<&'static str, Tally>) {
    for key in keys {
        let Some(ty) = rakata_core::ResourceTypeCode::from_raw_id(key.ty).known_type() else { continue };
        let fmt = match ty {
            ResourceType::Mdl => "mdl",
            ResourceType::Tpc => "tpc",
            ResourceType::Wok | ResourceType::Dwk | ResourceType::Pwk => "bwm",
            ResourceType::TwoDa => "2da",
            ResourceType::Lyt => "lyt",
            ResourceType::Vis => "vis",
            ResourceType::Tlk => "tlk",
            _ => "gff",
        };
        let bytes = match res.get(key) {
            Ok(Some(b)) => b,
            Ok(None) => continue,
            Err(e) => {
                tallies.entry("read").or_default().record(key, Err(e.to_string()));
                continue;
            }
        };
        let r = match fmt {
            "mdl" => {
                let mdx = res.get(&ResKey { name: key.name.clone(), ty: ResourceType::Mdx.type_id() }).ok().flatten();
                k2_mdl::read_mdl_from_bytes(&bytes, mdx.as_deref())
                    .map_err(|e| e.to_string())
                    .and_then(|m| check_mdl(&m))
            }
            "tpc" => rakata_formats::read_tpc_from_bytes(&bytes).map(drop).map_err(|e| e.to_string()),
            "bwm" => rakata_formats::read_bwm_from_bytes(&bytes).map(drop).map_err(|e| e.to_string()),
            "2da" => rakata_formats::read_twoda_from_bytes(&bytes).map(drop).map_err(|e| e.to_string()),
            "lyt" => rakata_formats::read_lyt_from_bytes(&bytes).map(drop).map_err(|e| e.to_string()),
            "vis" => rakata_formats::read_vis_from_bytes(&bytes).map(drop).map_err(|e| e.to_string()),
            "tlk" => rakata_formats::read_tlk_from_bytes(&bytes).map(drop).map_err(|e| e.to_string()),
            _ => {
                // Only things that look like GFF V3.2 count as GFF.
                if bytes.get(4..8) != Some(b"V3.2") {
                    continue;
                }
                rakata_formats::read_gff_from_bytes(&bytes).map(drop).map_err(|e| e.to_string())
            }
        };
        tallies.entry(fmt).or_default().record(key, r);
    }
}

/// Rejects models that parsed but carry implausible geometry, so a silent
/// layout misread can't count as ok.
fn check_mdl(mdl: &k2_mdl::Mdl) -> Result<(), String> {
    if !mdl.is_tsl() {
        return Err(format!("not a K2 model (fn_ptr1 {:#x})", mdl.geometry_fn_ptr1));
    }
    check_node(&mdl.root_node)
}

fn check_node(node: &k2_mdl::MdlNode) -> Result<(), String> {
    if let Some(mesh) = node.node_data.mesh() {
        let n = mesh.positions.len();
        if n != usize::from(mesh.vertex_count) {
            return Err(format!("sanity: mesh vertex count {} but {} positions read", mesh.vertex_count, n));
        }
        if let Some(p) = mesh.positions.iter().flatten().find(|v| !v.is_finite() || v.abs() > 1e5) {
            return Err(format!("sanity: vertex coordinate out of range ({p})"));
        }
        for f in &mesh.faces {
            if let Some(&i) = f.vertex_indices.iter().find(|&&i| usize::from(i) >= n) {
                return Err(format!("sanity: face index {i} >= vertex count {n}"));
            }
        }
    }
    node.children.iter().try_for_each(check_node)
}

/// Lists the light nodes of every room model of a module: position, colour,
/// radius, multiplier and the shadow / ambientonly / affectdynamic flags.
fn lights(install: Install, rest: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use k2_mdl::{MdlController, MdlControllerType, MdlNode, MdlNodeData};
    let name = rest.first().ok_or("lights needs MODULE")?;
    let mut res = Resources::open(install)?;
    let m = k2_data::Module::load(&mut res, name)?;
    println!("area sun_shadows {} shadow_opacity {} night {} ambient {:?} diffuse {:?} dyn_ambient {:?}", m.area.sun_shadows, m.area.shadow_opacity, m.area.is_night, m.area.ambient, m.area.diffuse, m.area.dynamic_ambient);
    fn ctl(cs: &[MdlController], t: MdlControllerType) -> Option<Vec<f32>> {
        cs.iter().find(|c| c.controller_type == t).and_then(|c| c.keys.first()).map(|k| k.values.clone())
    }
    fn walk(n: &MdlNode, off: [f32; 3], out: &mut Vec<String>) {
        let p = [off[0] + n.position[0], off[1] + n.position[1], off[2] + n.position[2]];
        if let MdlNodeData::Light(l) = &n.node_data {
            out.push(format!(
                "  {:<22} pos ({:7.2},{:7.2},{:7.2}) color {:?} radius {:?} mult {:?} shadow {} ambientonly {} affectdynamic {} prio {} ndyn {} flare {} fading {}",
                n.name, p[0], p[1], p[2], ctl(&n.controllers, MdlControllerType::COLOR), ctl(&n.controllers, MdlControllerType::RADIUS),
                ctl(&n.controllers, MdlControllerType::MULTIPLIER), l.shadow, l.ambientonly, l.affectdynamic, l.priority, l.num_dynamic_types, l.generateflare, l.fading_light
            ));
        }
        for c in &n.children {
            walk(c, p, out);
        }
    }
    for room in &m.layout.rooms {
        let Some(bytes) = res.get(&ResKey::new(&room.model, ResourceType::Mdl))? else { continue };
        let mdx = res.get(&ResKey::new(&room.model, ResourceType::Mdx))?;
        let mdl = k2_mdl::read_mdl_from_bytes(&bytes, mdx.as_deref())?;
        let mut out = Vec::new();
        walk(&mdl.root_node, room.position, &mut out);
        println!("room {} at {:?}: {} lights", room.model, room.position, out.len());
        for l in out {
            println!("{l}");
        }
    }
    Ok(())
}
