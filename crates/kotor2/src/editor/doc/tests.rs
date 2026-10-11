use std::io::Write;
use super::*;

#[test]
fn the_mod_capsule_follows_k2_mod_dir() {
    use std::ffi::OsStr;
    let home = Some(OsStr::new("/home/u"));
    let default = PathBuf::from("/home/u/.local/share/kotor2-rs/mods/editor.mod");
    assert_eq!(
        mod_path_in("editor", Some(OsStr::new("/run/mods:/other")), None, home),
        PathBuf::from("/run/mods/editor.mod"),
        "the first folder takes the capsule"
    );
    assert_eq!(
        mod_path_in("editor", Some(OsStr::new("")), None, home),
        default,
        "an empty K2_MOD_DIR is no folder"
    );
    assert_eq!(
        mod_path_in("editor", None, None, home),
        default,
        "without K2_MOD_DIR the default is unchanged"
    );
    assert_eq!(
        mod_path_in("editor", None, Some(OsStr::new("/data")), home),
        PathBuf::from("/data/kotor2-rs/mods/editor.mod"),
        "XDG_DATA_HOME still applies"
    );
}

#[test]
fn placeholders_are_resrefs_untitled_and_numbered_modules() {
    assert!(is_placeholder("101PER", "101per", None));
    assert!(is_placeholder("853NIH", "853nih", Some("853nih")));
    assert!(is_placeholder("904MAL", "907mal", Some("904mal")));
    assert!(is_placeholder("Untitled", "006ebo", Some("003ebo")));
    assert!(is_placeholder("module001", "106per", None));
    assert!(!is_placeholder("Taris", "101per", Some("101per")));
    assert!(!is_placeholder("Dantooine", "601dan", Some("601dan")));
    assert!(!is_placeholder("module", "106per", None));
    assert!(!is_placeholder(
        "Peragus - Administration Level",
        "101per",
        Some("101per")
    ));
}

#[test]
fn map_names_come_from_the_area_not_the_resref() {
    let dir = std::env::var("K2_INSTALL")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP map_names_come_from_the_area_not_the_resref: no install (set K2_INSTALL)");
        return;
    };
    let talk = Talk::load(&install).ok();
    let name = |m: &str| map_name(&install, talk.as_ref(), m);
    assert_eq!(name("001ebo"), "Ebon Hawk - Interior");
    assert_eq!(name("101per"), "Peragus - Administration Level");
    assert_eq!(name("904mal"), "Malachor V - Trayus Core");
    // 907mal's Mod_Name says 904MAL, but its entry area is 904mal.
    assert_eq!(name("907mal"), "Malachor V - Trayus Core");
    // Entry area "Untitled" in this module, named in module 003ebo / 204tel.
    assert_eq!(name("006ebo"), "Ebon Hawk - Interior");
    assert_eq!(name("205tel"), "Citadel Station - Residential 082 West");
    // Area and Mod_Name are both the resref: no name, the resref shows.
    assert_eq!(map_title(&install, talk.as_ref(), "853nih"), None);
    assert_eq!(name("853nih"), "853nih");
}
