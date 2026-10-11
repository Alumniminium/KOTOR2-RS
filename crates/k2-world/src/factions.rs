//! Faction reputation: repute.2da plus runtime adjustments.
//!
//! repute.2da is a square table: row = the faction holding the opinion,
//! column = the faction it is about (column 1 = faction 1, hostile_1; row 0 =
//! the Player faction, which has no column, so opinions about faction 0 are
//! read from row 0). Values are 0..100: <= 10 is hostile, >= 90 friendly,
//! anything else neutral (the NWN/KotOR thresholds).

use std::collections::HashMap;

use k2_data::Tables;
use k2_res::Resources;
use serde::{Deserialize, Serialize};

pub const HOSTILE_MAX: i32 = 10;
pub const FRIENDLY_MIN: i32 = 90;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Factions {
    /// table[a][b]: how faction a feels about faction b.
    table: Vec<Vec<i32>>,
    /// AdjustReputation / SetFactionReputation overrides, keyed (a, b).
    overrides: HashMap<(i32, i32), i32>,
}

impl Factions {
    pub fn load(res: &Resources, tables: &Tables) -> Self {
        let Ok(t) = tables.get(res, "repute") else { return Self::default() };
        let n = t.rows();
        let mut table = vec![vec![50; n]; n];
        // Column names follow the `label` column: Hostile_1 = row 1, ...
        let columns: Vec<Option<String>> = (0..n).map(|r| t.str(r, "label").map(str::to_ascii_lowercase)).collect();
        for (a, row) in table.iter_mut().enumerate() {
            for (b, cell) in row.iter_mut().enumerate() {
                let col = columns.get(b).cloned().flatten();
                let v = col.and_then(|c| t.int(a, &c));
                *cell = v.unwrap_or(50);
            }
        }
        // Opinions about faction 0 (Player) come from row 0 (no column).
        for a in 0..n {
            if let Some(v) = table.first().and_then(|r| r.get(a)).copied() {
                table[a][0] = v;
            }
        }
        table[0][0] = 100;
        Self { table, overrides: HashMap::new() }
    }

    pub fn reputation(&self, a: i32, b: i32) -> i32 {
        if let Some(v) = self.overrides.get(&(a, b)) {
            return *v;
        }
        if a == b {
            return 100;
        }
        let get = |x: i32, y: i32| self.table.get(x as usize).and_then(|r| r.get(y as usize)).copied();
        if a < 0 || b < 0 {
            return 50;
        }
        get(a, b).unwrap_or(50)
    }

    pub fn adjust(&mut self, a: i32, b: i32, delta: i32) {
        let v = (self.reputation(a, b) + delta).clamp(0, 100);
        self.overrides.insert((a, b), v);
    }

    pub fn set(&mut self, a: i32, b: i32, value: i32) {
        self.overrides.insert((a, b), value.clamp(0, 100));
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}
