//! Engine routines that need no game world: math, strings, dice,
//! conversions, vectors, printing. A real host calls [`Builtins::call`] first
//! and implements the world-facing routines itself.

use std::sync::Arc;

use crate::value::Value;
use crate::vm::Call;

/// Pure routines plus the RNG they need.
pub struct Builtins {
    rng: u64,
    /// Collected PrintString/PrintInteger/... output.
    pub printed: Vec<String>,
}

impl Default for Builtins {
    fn default() -> Self {
        Self::with_seed(0x2545_F491_4F6C_DD1D)
    }
}

impl Builtins {
    pub fn with_seed(seed: u64) -> Self {
        Self { rng: seed.max(1), printed: Vec::new() }
    }

    /// RNG state, for save games.
    pub fn rng_state(&self) -> u64 {
        self.rng
    }

    pub fn set_rng_state(&mut self, state: u64) {
        self.rng = state.max(1);
    }

    /// Uniform integer in 0..n (0 when n <= 0), xorshift64*.
    pub fn random(&mut self, n: i32) -> i32 {
        if n <= 0 {
            return 0;
        }
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let r = self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D);
        ((r >> 33) % n as u64) as i32
    }

    fn dice(&mut self, num: i32, sides: i32) -> i32 {
        (0..num.max(1)).map(|_| self.random(sides) + 1).sum()
    }

    /// Runs `call` if it is a world-independent routine.
    pub fn call(&mut self, call: &Call<'_>) -> Option<Value> {
        let a = call.args;
        let int = |i: usize| a.get(i).and_then(Value::as_int).unwrap_or(0);
        let flt = |i: usize| a.get(i).and_then(Value::as_float).unwrap_or(0.0);
        let st = |i: usize| a.get(i).and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
        let vec3 = |i: usize| a.get(i).and_then(Value::as_vector).unwrap_or([0.0; 3]);
        let s = |v: String| Value::String(Arc::from(v));
        Some(match call.routine.name.as_str() {
            "Random" => Value::Int(self.random(int(0))),
            "d2" => Value::Int(self.dice(int(0), 2)),
            "d3" => Value::Int(self.dice(int(0), 3)),
            "d4" => Value::Int(self.dice(int(0), 4)),
            "d6" => Value::Int(self.dice(int(0), 6)),
            "d8" => Value::Int(self.dice(int(0), 8)),
            "d10" => Value::Int(self.dice(int(0), 10)),
            "d12" => Value::Int(self.dice(int(0), 12)),
            "d20" => Value::Int(self.dice(int(0), 20)),
            "d100" => Value::Int(self.dice(int(0), 100)),
            "PrintString" => {
                self.printed.push(st(0));
                Value::Void
            }
            "PrintInteger" => {
                self.printed.push(int(0).to_string());
                Value::Void
            }
            "PrintFloat" => {
                self.printed.push(format_float(flt(0), int(1), int(2)));
                Value::Void
            }
            "PrintObject" => {
                self.printed.push(format!("{:x}", a.first().and_then(Value::as_object).unwrap_or(0)));
                Value::Void
            }
            "PrintVector" => {
                let v = vec3(0);
                self.printed.push(format!("{} {} {}", v[0], v[1], v[2]));
                Value::Void
            }
            "IntToString" => s(int(0).to_string()),
            "IntToHexString" => s(format!("0x{:08x}", int(0))),
            "FloatToString" => s(format_float(flt(0), int(1), int(2))),
            "IntToFloat" => Value::Float(int(0) as f32),
            "FloatToInt" => Value::Int(flt(0) as i32),
            "StringToInt" => Value::Int(leading_number(&st(0)).parse::<f64>().map_or(0, |v| v as i32)),
            "StringToFloat" => Value::Float(leading_number(&st(0)).parse::<f32>().unwrap_or(0.0)),
            "GetStringLength" => Value::Int(st(0).chars().count() as i32),
            "GetStringUpperCase" => s(st(0).to_uppercase()),
            "GetStringLowerCase" => s(st(0).to_lowercase()),
            "GetStringRight" => {
                let c: Vec<char> = st(0).chars().collect();
                let n = (int(1).max(0) as usize).min(c.len());
                s(c[c.len() - n..].iter().collect())
            }
            "GetStringLeft" => {
                let c: Vec<char> = st(0).chars().collect();
                let n = (int(1).max(0) as usize).min(c.len());
                s(c[..n].iter().collect())
            }
            "InsertString" => {
                let mut c: Vec<char> = st(0).chars().collect();
                let at = (int(2).max(0) as usize).min(c.len());
                let ins: Vec<char> = st(1).chars().collect();
                c.splice(at..at, ins);
                s(c.into_iter().collect())
            }
            "GetSubString" => {
                let c: Vec<char> = st(0).chars().collect();
                let start = (int(1).max(0) as usize).min(c.len());
                let end = (start + int(2).max(0) as usize).min(c.len());
                s(c[start..end].iter().collect())
            }
            "FindSubString" => {
                let h = st(0);
                Value::Int(h.find(&st(1)).map_or(-1, |b| h[..b].chars().count() as i32))
            }
            "abs" => Value::Int(int(0).wrapping_abs()),
            "fabs" => Value::Float(flt(0).abs()),
            // Angles in NWScript trig are degrees.
            "cos" => Value::Float(flt(0).to_radians().cos()),
            "sin" => Value::Float(flt(0).to_radians().sin()),
            "tan" => Value::Float(flt(0).to_radians().tan()),
            "acos" => Value::Float(flt(0).acos().to_degrees()),
            "asin" => Value::Float(flt(0).asin().to_degrees()),
            "atan" => Value::Float(flt(0).atan().to_degrees()),
            "log" => Value::Float(flt(0).ln()),
            "pow" => Value::Float(flt(0).powf(flt(1))),
            "sqrt" => Value::Float(flt(0).sqrt()),
            "RoundsToSeconds" => Value::Float(int(0) as f32 * 6.0),
            "HoursToSeconds" => Value::Float(int(0) as f32 * 120.0),
            "TurnsToSeconds" => Value::Float(int(0) as f32 * 60.0),
            "FeetToMeters" => Value::Float(flt(0) * 0.3048),
            "YardsToMeters" => Value::Float(flt(0) * 0.9144),
            "Vector" => Value::Vector([flt(0), flt(1), flt(2)]),
            "VectorMagnitude" => {
                let v = vec3(0);
                Value::Float((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt())
            }
            "VectorNormalize" => {
                let v = vec3(0);
                let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                Value::Vector(if m > 0.0 { v.map(|c| c / m) } else { v })
            }
            "AngleToVector" => {
                let r = flt(0).to_radians();
                Value::Vector([r.cos(), r.sin(), 0.0])
            }
            "VectorToAngle" => {
                let v = vec3(0);
                let deg = v[1].atan2(v[0]).to_degrees();
                Value::Float(if deg < 0.0 { deg + 360.0 } else { deg })
            }
            "ShipBuild" => Value::Int(1),
            _ => return None,
        })
    }
}

fn format_float(v: f32, width: i32, decimals: i32) -> String {
    let d = decimals.clamp(0, 9) as usize;
    let w = width.clamp(0, 18) as usize;
    format!("{v:>w$.d$}")
}

/// The numeric prefix atoi/atof would accept.
fn leading_number(s: &str) -> &str {
    let t = s.trim_start();
    let mut end = 0;
    let mut seen_dot = false;
    for (i, c) in t.char_indices() {
        let ok = c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) || (c == '.' && !seen_dot);
        if !ok {
            break;
        }
        seen_dot |= c == '.';
        end = i + c.len_utf8();
    }
    &t[..end]
}
