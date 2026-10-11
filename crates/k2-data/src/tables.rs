//! Cached 2DA access with typed getters and helpers for the tables the
//! engine needs.
//!
//! Empty cells ("****" on disk, "" after rakata decodes them) read as None.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use k2_res::{ResKey, Resources};
use rakata_core::ResourceType;
use rakata_formats::twoda::TwoDa;

use crate::{DataError, Result, parse_err};

/// One loaded 2DA with a lowercase column index.
#[derive(Debug)]
pub struct Table {
    pub name: String,
    columns: HashMap<String, usize>,
    data: TwoDa,
}

impl Table {
    pub fn new(name: &str, data: TwoDa) -> Self {
        let columns = data.headers.iter().enumerate().map(|(i, h)| (h.to_ascii_lowercase(), i)).collect();
        Self { name: name.to_owned(), columns, data }
    }

    pub fn rows(&self) -> usize {
        self.data.rows.len()
    }

    /// Lowercase column names in file order.
    pub fn columns(&self) -> Vec<String> {
        self.data.headers.iter().map(|h| h.to_ascii_lowercase()).collect()
    }

    pub fn has_column(&self, column: &str) -> bool {
        self.columns.contains_key(&column.to_ascii_lowercase())
    }

    pub fn row_label(&self, row: usize) -> Option<&str> {
        self.data.rows.get(row).map(|r| r.label.as_str())
    }

    /// Cell text, None for out-of-range, unknown column, "" or "****".
    pub fn str(&self, row: usize, column: &str) -> Option<&str> {
        let col = *self.columns.get(&column.to_ascii_lowercase())?;
        let cell = self.data.rows.get(row)?.cells.get(col)?.trim();
        (!cell.is_empty() && cell != "****").then_some(cell)
    }

    /// Lowercased cell text, the usual form for model and texture resrefs.
    pub fn resref(&self, row: usize, column: &str) -> Option<String> {
        self.str(row, column).map(str::to_ascii_lowercase)
    }

    /// Integer cell. Accepts decimal and 0x-prefixed hex.
    pub fn int(&self, row: usize, column: &str) -> Option<i32> {
        let s = self.str(row, column)?;
        if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            return i64::from_str_radix(hex, 16).ok().map(|v| v as i32);
        }
        s.parse::<i32>().ok().or_else(|| s.parse::<f32>().ok().map(|f| f as i32))
    }

    pub fn float(&self, row: usize, column: &str) -> Option<f32> {
        self.str(row, column)?.parse().ok()
    }

    pub fn flag(&self, row: usize, column: &str) -> bool {
        self.int(row, column).is_some_and(|v| v != 0)
    }

    /// First row whose `column` equals `value`, case-insensitively.
    pub fn find(&self, column: &str, value: &str) -> Option<usize> {
        (0..self.rows()).find(|&r| self.str(r, column).is_some_and(|c| c.eq_ignore_ascii_case(value)))
    }
}

/// Lazily loaded, cached 2DAs. Cheap to clone the returned `Arc<Table>`.
#[derive(Default)]
pub struct Tables {
    cache: Mutex<HashMap<String, Arc<Table>>>,
}

/// How a creature appearance is assembled (appearance.2da `modeltype`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelType {
    /// "B": body model chosen by armor body variation plus a separate head.
    Body,
    /// "F": full single model from the `race` column.
    Full,
    /// "S": simple creature.
    Simple,
    /// "L": large creature.
    Large,
}

/// Resolved models for one creature appearance.
#[derive(Debug, Clone, PartialEq)]
pub struct CreatureModels {
    pub model_type: ModelType,
    pub body: String,
    /// Body texture override (verified to exist); None means the model's
    /// own textures.
    pub body_texture: Option<String>,
    pub head: Option<String>,
    pub walk_speed: f32,
    pub run_speed: f32,
    pub hit_radius: f32,
    pub height: f32,
}

/// One surfacemat.2da row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceMat {
    pub walk: bool,
    pub walk_check: bool,
    pub line_of_sight: bool,
    pub grass: bool,
}

/// True when a TPC or TGA texture by this name resolves.
pub fn texture_exists(res: &Resources, name: &str) -> bool {
    [ResourceType::Tpc, ResourceType::Tga].into_iter().any(|t| res.locate(&ResKey::new(name, t)).is_some())
}

impl Tables {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load (once) and return a table.
    pub fn get(&self, res: &Resources, name: &str) -> Result<Arc<Table>> {
        let name = name.to_ascii_lowercase();
        if let Some(t) = self.cache.lock().unwrap().get(&name) {
            return Ok(t.clone());
        }
        let key = ResKey::new(&name, ResourceType::TwoDa);
        let bytes = res.get(&key)?.ok_or_else(|| DataError::Missing(key.to_string()))?;
        let data = rakata_formats::read_twoda_from_bytes(&bytes).map_err(|e| parse_err(&key, e))?;
        let table = Arc::new(Table::new(&name, data));
        self.cache.lock().unwrap().insert(name, table.clone());
        Ok(table)
    }

    /// Creature models for an appearance.2da row.
    ///
    /// `body_variation` is the armor's body letter ('a'..'n', 'a' when
    /// unarmored) and `texture_variation` its texture number (1 when
    /// unarmored); both only matter for `ModelType::Body`.
    pub fn creature_models(&self, res: &Resources, appearance: u32, body_variation: char, texture_variation: u32) -> Result<CreatureModels> {
        let app = self.get(res, "appearance")?;
        let row = appearance as usize;
        if row >= app.rows() {
            return Err(DataError::Missing(format!("appearance.2da row {row}")));
        }
        let model_type = match app.str(row, "modeltype").map(|s| s.to_ascii_uppercase()) {
            Some(s) if s == "B" => ModelType::Body,
            Some(s) if s == "S" => ModelType::Simple,
            Some(s) if s == "L" => ModelType::Large,
            _ => ModelType::Full,
        };
        let (body, body_texture, head) = if model_type == ModelType::Body {
            // An armor letter the row has no model for falls back to letter a
            // (model and texture together).
            let mut v = body_variation.to_ascii_lowercase();
            if app.resref(row, &format!("model{v}")).is_none() {
                v = 'a';
            }
            let body = app
                .resref(row, &format!("model{v}"))
                .ok_or_else(|| DataError::Missing(format!("appearance.2da row {row} body model")))?;
            // A letter whose tex column is empty (party bodies such as
            // P_AttonBB) keeps the model's own textures; borrowing texa put
            // the plain body skin on them. Some armor TextureVar values have
            // no matching texture (n_smuggler04 + 06); fall back to variation
            // 01, then the bare name, then the model's own textures.
            let tex = app.resref(row, &format!("tex{v}")).and_then(|t| {
                [format!("{t}{texture_variation:02}"), format!("{t}01"), t].into_iter().find(|c| texture_exists(res, c))
            });
            let head = match app.int(row, "normalhead") {
                Some(h) if h >= 0 => self.get(res, "heads")?.resref(h as usize, "head"),
                _ => None,
            };
            (body, tex, head)
        } else {
            let body = app
                .resref(row, "race")
                .ok_or_else(|| DataError::Missing(format!("appearance.2da row {row} race model")))?;
            // racetex is an optional override; several rows name textures
            // that do not ship (n_gand, n_talia) and use the model's own.
            (body, app.resref(row, "racetex").filter(|t| texture_exists(res, t)), None)
        };
        Ok(CreatureModels {
            model_type,
            body,
            body_texture,
            head,
            walk_speed: app.float(row, "walkdist").unwrap_or(1.7),
            run_speed: app.float(row, "rundist").unwrap_or(5.4),
            hit_radius: app.float(row, "hitradius").unwrap_or(0.5),
            height: app.float(row, "height").unwrap_or(1.6),
        })
    }

    /// placeables.2da modelname for a UTP Appearance.
    pub fn placeable_model(&self, res: &Resources, appearance: u32) -> Result<Option<String>> {
        Ok(self.get(res, "placeables")?.resref(appearance as usize, "modelname"))
    }

    /// genericdoors.2da modelname for a UTD GenericType.
    pub fn door_model(&self, res: &Resources, generic_type: u32) -> Result<Option<String>> {
        Ok(self.get(res, "genericdoors")?.resref(generic_type as usize, "modelname"))
    }

    /// surfacemat.2da row for a walkmesh material id.
    pub fn surface(&self, res: &Resources, material: u32) -> Result<SurfaceMat> {
        let t = self.get(res, "surfacemat")?;
        let r = material as usize;
        Ok(SurfaceMat {
            walk: t.flag(r, "walk"),
            walk_check: t.flag(r, "walkcheck"),
            line_of_sight: t.flag(r, "lineofsight"),
            grass: t.flag(r, "grass"),
        })
    }

    /// All surface materials, indexed by material id.
    pub fn surfaces(&self, res: &Resources) -> Result<Vec<SurfaceMat>> {
        let n = self.get(res, "surfacemat")?.rows();
        (0..n as u32).map(|m| self.surface(res, m)).collect()
    }

    /// portraits.2da baseresref for a portrait id.
    pub fn portrait(&self, res: &Resources, id: u32) -> Result<Option<String>> {
        Ok(self.get(res, "portraits")?.resref(id as usize, "baseresref"))
    }
}

#[cfg(test)]
mod tests;
