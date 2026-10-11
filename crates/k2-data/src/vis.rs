//! Lenient VIS (room visibility) reader.
//!
//! Format: an unindented "room count" line starts a room; the indented lines
//! after it name the rooms it can see. Shipped data does not always agree
//! with the declared count (710kor.vis lists fewer children than it
//! declares), so children are taken by indentation and the count is ignored.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Visibility {
    /// Room name (lowercase) to the rooms visible from it.
    pub rooms: BTreeMap<String, BTreeSet<String>>,
}

impl Visibility {
    pub fn parse(bytes: &[u8]) -> Self {
        let text: String = bytes.iter().map(|&b| b as char).collect();
        let mut vis = Self::default();
        let mut current: Option<String> = None;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let indented = line.starts_with(|c: char| c.is_whitespace());
            let mut tokens = trimmed.split_whitespace();
            let name = tokens.next().unwrap_or_default().to_ascii_lowercase();
            if indented {
                if let Some(parent) = &current {
                    vis.rooms.entry(parent.clone()).or_default().insert(name);
                }
            } else {
                // "room V3.28" style version lines carry no room.
                if tokens.next().is_some_and(|t| t.starts_with(['V', 'v'])) {
                    current = None;
                    continue;
                }
                vis.rooms.entry(name.clone()).or_default();
                current = Some(name);
            }
        }
        vis
    }

    /// Rooms visible from `room`, always including itself.
    pub fn visible_from(&self, room: &str) -> BTreeSet<String> {
        let room = room.to_ascii_lowercase();
        let mut out = self.rooms.get(&room).cloned().unwrap_or_default();
        out.insert(room);
        out
    }
}

#[cfg(test)]
mod tests;
