//! Typed, forgiving accessors over rakata's GFF tree.
//!
//! The engine is lenient about field types (a BYTE where an INT is expected
//! reads fine), so numeric getters coerce across all integer widths.

use std::fmt::Write as _;

use rakata_formats::gff::{GffLocalizedString, GffStruct, GffValue};

/// A CExoLocString: TLK reference and/or embedded substrings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocString {
    /// TLK index, None when the field carries no reference.
    pub strref: Option<u32>,
    /// Embedded substrings as (string_id, text); string_id = language * 2 + gender.
    pub substrings: Vec<(u32, String)>,
}

impl LocString {
    pub fn from_gff(l: &GffLocalizedString) -> Self {
        Self {
            strref: l.string_ref.index(),
            substrings: l.substrings.iter().map(|s| (s.string_id, s.text.clone())).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.strref.is_none() && self.substrings.iter().all(|(_, t)| t.is_empty())
    }
}

/// Extension getters on a GFF struct.
pub trait GffExt {
    fn value(&self, label: &str) -> Option<&GffValue>;

    fn has(&self, label: &str) -> bool {
        self.value(label).is_some()
    }

    fn i64_(&self, label: &str) -> Option<i64> {
        Some(match self.value(label)? {
            GffValue::UInt8(v) => i64::from(*v),
            GffValue::Int8(v) => i64::from(*v),
            GffValue::UInt16(v) => i64::from(*v),
            GffValue::Int16(v) => i64::from(*v),
            GffValue::UInt32(v) => i64::from(*v),
            GffValue::Int32(v) => i64::from(*v),
            GffValue::UInt64(v) => *v as i64,
            GffValue::Int64(v) => *v,
            GffValue::Single(v) => *v as i64,
            GffValue::Double(v) => *v as i64,
            _ => return None,
        })
    }

    fn int(&self, label: &str) -> Option<i32> {
        self.i64_(label).map(|v| v as i32)
    }

    fn uint(&self, label: &str) -> Option<u32> {
        self.i64_(label).map(|v| v as u32)
    }

    fn int_or(&self, label: &str, default: i32) -> i32 {
        self.int(label).unwrap_or(default)
    }

    fn flag(&self, label: &str) -> bool {
        self.i64_(label).is_some_and(|v| v != 0)
    }

    fn float(&self, label: &str) -> Option<f32> {
        Some(match self.value(label)? {
            GffValue::Single(v) => *v,
            GffValue::Double(v) => *v as f32,
            _ => self.i64_(label)? as f32,
        })
    }

    fn float_or(&self, label: &str, default: f32) -> f32 {
        self.float(label).unwrap_or(default)
    }

    /// CExoString or CResRef as text. ResRefs come back lowercase.
    fn string(&self, label: &str) -> Option<String> {
        match self.value(label)? {
            GffValue::String(s) => Some(s.clone()),
            GffValue::ResRef(r) => Some(r.to_string().to_ascii_lowercase()),
            _ => None,
        }
    }

    fn string_or_empty(&self, label: &str) -> String {
        self.string(label).unwrap_or_default()
    }

    /// Non-empty resref, lowercase.
    fn resref(&self, label: &str) -> Option<String> {
        self.string(label).map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty())
    }

    fn locstring(&self, label: &str) -> LocString {
        match self.value(label) {
            Some(GffValue::LocalizedString(l)) => LocString::from_gff(l),
            _ => LocString::default(),
        }
    }

    fn list(&self, label: &str) -> &[GffStruct] {
        match self.value(label) {
            Some(GffValue::List(l)) => l,
            _ => &[],
        }
    }

    fn child(&self, label: &str) -> Option<&GffStruct> {
        match self.value(label)? {
            GffValue::Struct(s) => Some(s),
            _ => None,
        }
    }

    fn vec3(&self, label: &str) -> Option<[f32; 3]> {
        match self.value(label)? {
            GffValue::Vector3(v) => Some(*v),
            _ => None,
        }
    }

    /// Quaternion as stored: scalar first [w, x, y, z].
    fn quat(&self, label: &str) -> Option<[f32; 4]> {
        match self.value(label)? {
            GffValue::Vector4(v) => Some(*v),
            _ => None,
        }
    }

    fn binary(&self, label: &str) -> Option<&[u8]> {
        match self.value(label)? {
            GffValue::Binary(b) => Some(b),
            _ => None,
        }
    }

    /// Three floats stored as label suffixed X/Y/Z (e.g. "XPosition"
    /// style is handled by the caller; this is "<prefix>X").
    fn xyz(&self, x: &str, y: &str, z: &str) -> [f32; 3] {
        [self.float_or(x, 0.0), self.float_or(y, 0.0), self.float_or(z, 0.0)]
    }
}

impl GffExt for GffStruct {
    fn value(&self, label: &str) -> Option<&GffValue> {
        self.field(label)
    }
}

/// Packed BGR colour (DWORD 0x00BBGGRR) to linear-ish [r, g, b] in 0..1.
pub fn color_from_u32(v: u32) -> [f32; 3] {
    let r = (v & 0xFF) as f32 / 255.0;
    let g = ((v >> 8) & 0xFF) as f32 / 255.0;
    let b = ((v >> 16) & 0xFF) as f32 / 255.0;
    [r, g, b]
}

/// Human-readable dump of a GFF tree, for k2probe.
pub fn dump(s: &GffStruct, depth: usize, max_list: usize) -> String {
    let mut out = String::new();
    dump_into(&mut out, s, depth, max_list);
    out
}

fn dump_into(out: &mut String, s: &GffStruct, depth: usize, max_list: usize) {
    let pad = "  ".repeat(depth);
    for f in &s.fields {
        let label = f.label.as_str();
        match &f.value {
            GffValue::Struct(c) => {
                let _ = writeln!(out, "{pad}{label}: struct id={}", c.struct_id);
                dump_into(out, c, depth + 1, max_list);
            }
            GffValue::List(l) => {
                let _ = writeln!(out, "{pad}{label}: list[{}]", l.len());
                for (i, c) in l.iter().take(max_list).enumerate() {
                    let _ = writeln!(out, "{pad}  [{i}] id={}", c.struct_id);
                    dump_into(out, c, depth + 2, max_list);
                }
            }
            GffValue::LocalizedString(l) => {
                let _ = writeln!(out, "{pad}{label}: locstr strref={:?} subs={:?}", l.string_ref.index(), l.substrings.iter().map(|s| (s.string_id, &s.text)).collect::<Vec<_>>());
            }
            GffValue::Binary(b) => {
                let _ = writeln!(out, "{pad}{label}: binary[{}]", b.len());
            }
            GffValue::ResRef(r) => {
                let _ = writeln!(out, "{pad}{label}: resref {r:?}", r = r.to_string());
            }
            other => {
                let _ = writeln!(out, "{pad}{label}: {other:?}");
            }
        }
    }
}
