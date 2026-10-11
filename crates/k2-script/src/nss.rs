//! Engine routine table, parsed at runtime from the game's own nwscript.nss.
//!
//! The routine index used by the ACTION opcode is the declaration order of
//! function prototypes in that file. Nothing derived from the file is stored
//! in this repository.

use std::collections::HashMap;

/// Script-level variable types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VarType {
    Void,
    Int,
    Float,
    String,
    Object,
    Vector,
    /// Deferred code block (STORE_STATE). Takes no stack space.
    Action,
    /// Engine structure by index (ENGINE_STRUCTURE_N).
    Engine(u8),
}

impl VarType {
    /// Stack slots (4 bytes each) a value of this type occupies.
    pub fn slots(self) -> usize {
        match self {
            VarType::Void | VarType::Action => 0,
            VarType::Vector => 3,
            _ => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DefaultValue {
    Int(i32),
    Float(f32),
    String(String),
    ObjectSelf,
    ObjectInvalid,
    Vector([f32; 3]),
    /// A default the parser could not evaluate (kept verbatim).
    Unparsed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub ty: VarType,
    pub name: String,
    pub default: Option<DefaultValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Routine {
    pub id: u16,
    pub name: String,
    pub ret: VarType,
    pub params: Vec<Param>,
}

impl Routine {
    pub fn min_args(&self) -> usize {
        self.params.iter().take_while(|p| p.default.is_none()).count()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Int(i32),
    Float(f32),
    String(String),
}

#[derive(Debug, thiserror::Error)]
pub enum NssError {
    #[error("unknown type {0:?} in {1:?}")]
    UnknownType(String, String),
    #[error("malformed declaration {0:?}")]
    Malformed(String),
}

/// Routine table plus the constants and engine structures declared alongside.
#[derive(Debug, Clone, Default)]
pub struct Routines {
    pub routines: Vec<Routine>,
    pub engine_structures: Vec<String>,
    pub constants: HashMap<String, ConstValue>,
    by_name: HashMap<String, u16>,
}

impl Routines {
    pub fn get(&self, id: u16) -> Option<&Routine> {
        self.routines.get(id as usize)
    }

    pub fn by_name(&self, name: &str) -> Option<&Routine> {
        self.by_name.get(name).and_then(|&i| self.get(i))
    }

    pub fn len(&self) -> usize {
        self.routines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routines.is_empty()
    }

    /// Parses nwscript.nss source bytes (Windows-1252, CRLF tolerated).
    pub fn parse(bytes: &[u8]) -> Result<Self, NssError> {
        let text: String = bytes.iter().map(|&b| char::from(b)).collect();
        let text = strip_comments(&text);

        let mut out = Routines::default();
        let mut body = String::with_capacity(text.len());
        for line in text.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("#define") {
                let mut it = rest.split_whitespace();
                if let (Some(k), Some(v)) = (it.next(), it.next())
                    && let Some(n) = k.strip_prefix("ENGINE_STRUCTURE_")
                    && let Ok(n) = n.parse::<usize>()
                {
                    if out.engine_structures.len() <= n {
                        out.engine_structures.resize(n + 1, String::new());
                    }
                    out.engine_structures[n] = v.to_owned();
                }
                continue;
            }
            body.push_str(line);
            body.push('\n');
        }

        for stmt in split_top(&body, ';') {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            if let Some(open) = stmt.find('(') {
                // Function prototype: "<ret> <Name>(<params>)"
                let close = stmt.rfind(')').ok_or_else(|| NssError::Malformed(stmt.to_owned()))?;
                let head: Vec<&str> = stmt[..open].split_whitespace().collect();
                let [ret, name] = head[..] else {
                    // Not a prototype (e.g. a constant initialised by a call).
                    continue;
                };
                let ret = out.parse_type(ret, stmt)?;
                let mut params = Vec::new();
                for p in split_top(&stmt[open + 1..close], ',') {
                    let p = p.trim();
                    if p.is_empty() || p == "void" {
                        continue;
                    }
                    let (decl, default) = match p.split_once('=') {
                        Some((d, v)) => (d.trim(), Some(v.trim())),
                        None => (p, None),
                    };
                    let mut it = decl.split_whitespace();
                    let (Some(t), Some(n)) = (it.next(), it.next()) else {
                        return Err(NssError::Malformed(stmt.to_owned()));
                    };
                    let ty = out.parse_type(t, stmt)?;
                    let default = default.map(|d| out.eval_default(d));
                    params.push(Param { ty, name: n.to_owned(), default });
                }
                let id = out.routines.len() as u16;
                out.by_name.insert(name.to_owned(), id);
                out.routines.push(Routine { id, name: name.to_owned(), ret, params });
            } else if let Some((decl, value)) = stmt.split_once('=') {
                let mut it = decl.split_whitespace();
                let (Some(t), Some(n)) = (it.next(), it.next()) else { continue };
                let value = value.trim();
                let c = match t {
                    "int" => parse_int(value).map(ConstValue::Int).or_else(|| out.lookup_int(value)),
                    "float" => parse_float(value).map(ConstValue::Float),
                    "string" => parse_string(value).map(ConstValue::String),
                    _ => None,
                };
                if let Some(c) = c {
                    out.constants.insert(n.to_owned(), c);
                }
            }
        }
        Ok(out)
    }

    fn lookup_int(&self, name: &str) -> Option<ConstValue> {
        match self.constants.get(name) {
            Some(ConstValue::Int(v)) => Some(ConstValue::Int(*v)),
            _ => None,
        }
    }

    fn parse_type(&self, t: &str, ctx: &str) -> Result<VarType, NssError> {
        Ok(match t {
            "void" => VarType::Void,
            // A handful of K2 prototypes use these spellings.
            "int" | "INT" => VarType::Int,
            "float" => VarType::Float,
            "string" => VarType::String,
            "object" | "OBJECT_ID" => VarType::Object,
            "vector" => VarType::Vector,
            "action" => VarType::Action,
            other => match self.engine_structures.iter().position(|s| s == other) {
                Some(i) => VarType::Engine(i as u8),
                None => return Err(NssError::UnknownType(other.to_owned(), ctx.to_owned())),
            },
        })
    }

    fn eval_default(&self, v: &str) -> DefaultValue {
        let v = v.trim();
        if let Some(i) = parse_int(v) {
            return DefaultValue::Int(i);
        }
        if let Some(f) = parse_float(v) {
            return DefaultValue::Float(f);
        }
        if let Some(s) = parse_string(v) {
            return DefaultValue::String(s);
        }
        match v {
            "OBJECT_SELF" => return DefaultValue::ObjectSelf,
            "OBJECT_INVALID" => return DefaultValue::ObjectInvalid,
            _ => {}
        }
        if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let parts: Vec<f32> = inner.split(',').filter_map(|p| parse_float(p.trim())).collect();
            if parts.len() == 3 {
                return DefaultValue::Vector([parts[0], parts[1], parts[2]]);
            }
        }
        match self.constants.get(v) {
            Some(ConstValue::Int(i)) => DefaultValue::Int(*i),
            Some(ConstValue::Float(f)) => DefaultValue::Float(*f),
            Some(ConstValue::String(s)) => DefaultValue::String(s.clone()),
            None => DefaultValue::Unparsed(v.to_owned()),
        }
    }
}

fn parse_int(v: &str) -> Option<i32> {
    let v = v.trim();
    let (neg, body) = match v.strip_prefix('-') {
        Some(b) => (true, b.trim()),
        None => (false, v),
    };
    let n = if let Some(h) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        u32::from_str_radix(h, 16).ok()? as i32
    } else if body.chars().all(|c| c.is_ascii_digit()) && !body.is_empty() {
        body.parse::<i64>().ok()? as i32
    } else {
        return None;
    };
    Some(if neg { n.wrapping_neg() } else { n })
}

fn parse_float(v: &str) -> Option<f32> {
    let v = v.trim();
    let v = v.strip_suffix('f').unwrap_or(v);
    if !v.contains('.') {
        return None;
    }
    v.parse::<f32>().ok()
}

fn parse_string(v: &str) -> Option<String> {
    let v = v.trim();
    v.strip_prefix('"').and_then(|s| s.strip_suffix('"')).map(str::to_owned)
}

fn strip_comments(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
        } else if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else {
            if c != '\r' {
                out.push(c);
            }
            i += 1;
        }
    }
    out
}

/// Splits on `sep` outside brackets, parentheses and strings.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '(' | '[' if !in_str => depth += 1,
            ')' | ']' if !in_str => depth -= 1,
            c if c == sep && depth == 0 && !in_str => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}
