//! Global variables (globalcat.2da) that persist across modules.

use std::collections::{BTreeMap, HashMap};

use k2_data::Tables;
use k2_res::Resources;

use crate::engine::Location;

/// Global numbers, booleans, strings and locations. Names are
/// case-insensitive. globalcat.2da declares them; undeclared names are still
/// stored (and listed in `undeclared`) because scripts occasionally use them.
#[derive(Debug, Clone, Default)]
pub struct Globals {
    pub numbers: BTreeMap<String, i32>,
    pub booleans: BTreeMap<String, bool>,
    pub strings: BTreeMap<String, String>,
    pub locations: BTreeMap<String, Location>,
    declared: HashMap<String, String>,
    pub undeclared: BTreeMap<String, usize>,
}

impl Globals {
    pub fn from_tables(res: &Resources, tables: &Tables) -> Self {
        let mut g = Self::default();
        if let Ok(t) = tables.get(res, "globalcat") {
            for row in 0..t.rows() {
                let (Some(name), Some(ty)) = (t.str(row, "name"), t.str(row, "type")) else { continue };
                let key = name.to_ascii_lowercase();
                match ty.to_ascii_lowercase().as_str() {
                    "number" => {
                        g.numbers.insert(key.clone(), 0);
                    }
                    "boolean" => {
                        g.booleans.insert(key.clone(), false);
                    }
                    "string" => {
                        g.strings.insert(key.clone(), String::new());
                    }
                    _ => {}
                }
                g.declared.insert(key, ty.to_ascii_lowercase());
            }
        }
        g
    }

    fn note(&mut self, key: &str) {
        if !self.declared.is_empty() && !self.declared.contains_key(key) {
            *self.undeclared.entry(key.to_owned()).or_default() += 1;
        }
    }

    pub fn number(&self, name: &str) -> i32 {
        self.numbers.get(&name.to_ascii_lowercase()).copied().unwrap_or(0)
    }

    pub fn set_number(&mut self, name: &str, v: i32) {
        let key = name.to_ascii_lowercase();
        self.note(&key);
        self.numbers.insert(key, v);
    }

    pub fn boolean(&self, name: &str) -> bool {
        self.booleans.get(&name.to_ascii_lowercase()).copied().unwrap_or(false)
    }

    pub fn set_boolean(&mut self, name: &str, v: bool) {
        let key = name.to_ascii_lowercase();
        self.note(&key);
        self.booleans.insert(key, v);
    }

    pub fn string(&self, name: &str) -> String {
        self.strings.get(&name.to_ascii_lowercase()).cloned().unwrap_or_default()
    }

    pub fn set_string(&mut self, name: &str, v: String) {
        let key = name.to_ascii_lowercase();
        self.note(&key);
        self.strings.insert(key, v);
    }

    pub fn location(&self, name: &str) -> Option<Location> {
        self.locations.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn set_location(&mut self, name: &str, v: Location) {
        let key = name.to_ascii_lowercase();
        self.note(&key);
        self.locations.insert(key, v);
    }

    pub fn declared_count(&self) -> usize {
        self.declared.len()
    }
}
