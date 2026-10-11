//! NCS bytecode parsing.
//!
//! Layout: "NCS V1.0" magic, then a pseudo-instruction 0x42 followed by the
//! big-endian u32 file size, then instructions from offset 13. All operands
//! are big-endian. Jump offsets are relative to the start of the jumping
//! instruction.

use std::collections::HashMap;
use std::fmt;

/// First instruction offset (8 byte magic + 5 byte size marker).
pub const CODE_START: u32 = 13;

#[derive(Debug, thiserror::Error)]
pub enum NcsError {
    #[error("not an NCS V1.0 file")]
    BadMagic,
    #[error("missing 0x42 size marker")]
    BadSizeMarker,
    #[error("truncated instruction at {0:#x}")]
    Truncated(u32),
    #[error("unknown opcode {op:#04x} type {ty:#04x} at {at:#x}")]
    UnknownOpcode { op: u8, ty: u8, at: u32 },
}

/// Instruction type byte for typed opcodes.
pub mod ty {
    pub const NONE: u8 = 0x00;
    pub const DIRECT: u8 = 0x01;
    pub const INT: u8 = 0x03;
    pub const FLOAT: u8 = 0x04;
    pub const STRING: u8 = 0x05;
    pub const OBJECT: u8 = 0x06;
    /// 0x10..=0x1F: engine structure N (effect, event, location, talent, ...).
    pub const ENGINE0: u8 = 0x10;
    pub const II: u8 = 0x20;
    pub const FF: u8 = 0x21;
    pub const OO: u8 = 0x22;
    pub const SS: u8 = 0x23;
    pub const TT: u8 = 0x24;
    pub const IF: u8 = 0x25;
    pub const FI: u8 = 0x26;
    /// 0x30..=0x39: engine structure N compared with itself.
    pub const ENGINE0_ENGINE0: u8 = 0x30;
    pub const VV: u8 = 0x3A;
    pub const VF: u8 = 0x3B;
    pub const FV: u8 = 0x3C;
}

/// Arithmetic / comparison operators sharing the binary-op encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    LogAnd,
    LogOr,
    IncOr,
    ExcOr,
    BoolAnd,
    Equal,
    NEqual,
    Geq,
    Gt,
    Lt,
    Leq,
    ShLeft,
    ShRight,
    UShRight,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

impl BinOp {
    fn from_opcode(op: u8) -> Option<Self> {
        Some(match op {
            0x06 => Self::LogAnd,
            0x07 => Self::LogOr,
            0x08 => Self::IncOr,
            0x09 => Self::ExcOr,
            0x0A => Self::BoolAnd,
            0x0B => Self::Equal,
            0x0C => Self::NEqual,
            0x0D => Self::Geq,
            0x0E => Self::Gt,
            0x0F => Self::Lt,
            0x10 => Self::Leq,
            0x11 => Self::ShLeft,
            0x12 => Self::ShRight,
            0x13 => Self::UShRight,
            0x14 => Self::Add,
            0x15 => Self::Sub,
            0x16 => Self::Mul,
            0x17 => Self::Div,
            0x18 => Self::Mod,
            _ => return None,
        })
    }

    pub fn mnemonic(self) -> &'static str {
        match self {
            Self::LogAnd => "LOGAND",
            Self::LogOr => "LOGOR",
            Self::IncOr => "INCOR",
            Self::ExcOr => "EXCOR",
            Self::BoolAnd => "BOOLAND",
            Self::Equal => "EQUAL",
            Self::NEqual => "NEQUAL",
            Self::Geq => "GEQ",
            Self::Gt => "GT",
            Self::Lt => "LT",
            Self::Leq => "LEQ",
            Self::ShLeft => "SHLEFT",
            Self::ShRight => "SHRIGHT",
            Self::UShRight => "USHRIGHT",
            Self::Add => "ADD",
            Self::Sub => "SUB",
            Self::Mul => "MUL",
            Self::Div => "DIV",
            Self::Mod => "MOD",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    CpDownSp { offset: i32, size: u16 },
    RsAdd(u8),
    CpTopSp { offset: i32, size: u16 },
    ConstI(i32),
    ConstF(f32),
    ConstS(String),
    ConstO(u32),
    Action { routine: u16, argc: u8 },
    /// Binary operator with its operand type byte. `size` is only set for TT.
    Bin { op: BinOp, ty: u8, size: u16 },
    Neg(u8),
    Comp,
    MovSp(i32),
    StoreStateAll(u8),
    Jmp(i32),
    Jsr(i32),
    Jz(i32),
    Retn,
    Destruct { size: i16, keep_offset: i16, keep_size: i16 },
    NotI,
    DecISp(i32),
    IncISp(i32),
    Jnz(i32),
    CpDownBp { offset: i32, size: u16 },
    CpTopBp { offset: i32, size: u16 },
    DecIBp(i32),
    IncIBp(i32),
    SaveBp,
    RestoreBp,
    /// `skip` is the type byte: distance from this instruction to the
    /// deferred code block (always past the following JMP).
    StoreState { skip: u8, size_bp: u32, size_sp: u32 },
    Nop,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Instr {
    pub offset: u32,
    pub len: u32,
    pub op: Op,
}

impl Instr {
    /// Absolute target of a relative jump, if this is one.
    pub fn jump_target(&self) -> Option<u32> {
        match self.op {
            Op::Jmp(d) | Op::Jsr(d) | Op::Jz(d) | Op::Jnz(d) => Some((self.offset as i64 + d as i64) as u32),
            _ => None,
        }
    }
}

/// A parsed compiled script.
#[derive(Debug, Clone, PartialEq)]
pub struct Ncs {
    pub name: String,
    pub instrs: Vec<Instr>,
    index: HashMap<u32, usize>,
}

impl Ncs {
    pub fn parse(name: impl Into<String>, bytes: &[u8]) -> Result<Self, NcsError> {
        if bytes.len() < CODE_START as usize || &bytes[..8] != b"NCS V1.0" {
            return Err(NcsError::BadMagic);
        }
        if bytes[8] != 0x42 {
            return Err(NcsError::BadSizeMarker);
        }
        // The declared size is informational; trust the buffer we got.
        let mut instrs = Vec::new();
        let mut index = HashMap::new();
        let mut pos = CODE_START as usize;
        while pos < bytes.len() {
            let instr = decode(bytes, pos)?;
            pos += instr.len as usize;
            index.insert(instr.offset, instrs.len());
            instrs.push(instr);
        }
        Ok(Self { name: name.into(), instrs, index })
    }

    /// An empty script carrying only a name (save loading, see saved.rs).
    pub fn placeholder(name: impl Into<String>) -> Self {
        Self { name: name.into(), instrs: Vec::new(), index: HashMap::new() }
    }

    /// Index of the instruction starting at `offset`.
    pub fn at(&self, offset: u32) -> Option<usize> {
        self.index.get(&offset).copied()
    }
}

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
    start: u32,
}

impl Cursor<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], NcsError> {
        let s = self.b.get(self.pos..self.pos + N).ok_or(NcsError::Truncated(self.start))?;
        self.pos += N;
        Ok(s.try_into().expect("slice length"))
    }
    fn u8(&mut self) -> Result<u8, NcsError> {
        Ok(self.take::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, NcsError> {
        Ok(u16::from_be_bytes(self.take()?))
    }
    fn i16(&mut self) -> Result<i16, NcsError> {
        Ok(i16::from_be_bytes(self.take()?))
    }
    fn u32(&mut self) -> Result<u32, NcsError> {
        Ok(u32::from_be_bytes(self.take()?))
    }
    fn i32(&mut self) -> Result<i32, NcsError> {
        Ok(i32::from_be_bytes(self.take()?))
    }
}

fn decode(bytes: &[u8], pos: usize) -> Result<Instr, NcsError> {
    let start = pos as u32;
    let mut c = Cursor { b: bytes, pos, start };
    let op = c.u8()?;
    let t = c.u8()?;
    let unknown = || NcsError::UnknownOpcode { op, ty: t, at: start };
    let decoded = match op {
        0x01 => Op::CpDownSp { offset: c.i32()?, size: c.u16()? },
        0x02 => Op::RsAdd(t),
        0x03 => Op::CpTopSp { offset: c.i32()?, size: c.u16()? },
        0x04 => match t {
            ty::INT => Op::ConstI(c.i32()?),
            ty::FLOAT => Op::ConstF(f32::from_bits(c.u32()?)),
            ty::OBJECT => Op::ConstO(c.u32()?),
            ty::STRING => {
                let n = c.u16()? as usize;
                let s = c.b.get(c.pos..c.pos + n).ok_or(NcsError::Truncated(start))?;
                c.pos += n;
                // Script text is Windows-1252; map bytes 1:1 for ASCII and
                // through char::from for the rest (Latin-1 superset is close
                // enough for engine strings, which are tags and resrefs).
                Op::ConstS(s.iter().map(|&b| char::from(b)).collect())
            }
            _ => return Err(unknown()),
        },
        0x05 => Op::Action { routine: c.u16()?, argc: c.u8()? },
        0x06..=0x18 => {
            let bop = BinOp::from_opcode(op).ok_or_else(unknown)?;
            let size = if t == ty::TT { c.u16()? } else { 0 };
            Op::Bin { op: bop, ty: t, size }
        }
        0x19 => Op::Neg(t),
        0x1A => Op::Comp,
        0x1B => Op::MovSp(c.i32()?),
        0x1C => Op::StoreStateAll(t),
        0x1D => Op::Jmp(c.i32()?),
        0x1E => Op::Jsr(c.i32()?),
        0x1F => Op::Jz(c.i32()?),
        0x20 => Op::Retn,
        0x21 => Op::Destruct { size: c.i16()?, keep_offset: c.i16()?, keep_size: c.i16()? },
        0x22 => Op::NotI,
        0x23 => Op::DecISp(c.i32()?),
        0x24 => Op::IncISp(c.i32()?),
        0x25 => Op::Jnz(c.i32()?),
        0x26 => Op::CpDownBp { offset: c.i32()?, size: c.u16()? },
        0x27 => Op::CpTopBp { offset: c.i32()?, size: c.u16()? },
        0x28 => Op::DecIBp(c.i32()?),
        0x29 => Op::IncIBp(c.i32()?),
        0x2A => Op::SaveBp,
        0x2B => Op::RestoreBp,
        0x2C => Op::StoreState { skip: t, size_bp: c.u32()?, size_sp: c.u32()? },
        0x2D => Op::Nop,
        _ => return Err(unknown()),
    };
    Ok(Instr { offset: start, len: (c.pos - pos) as u32, op: decoded })
}

/// Short type suffix used by the disassembler (I, F, S, O, II, ...).
pub fn type_suffix(t: u8) -> String {
    match t {
        ty::NONE => String::new(),
        ty::DIRECT => String::new(),
        ty::INT => "I".into(),
        ty::FLOAT => "F".into(),
        ty::STRING => "S".into(),
        ty::OBJECT => "O".into(),
        0x10..=0x1F => format!("E{}", t - ty::ENGINE0),
        ty::II => "II".into(),
        ty::FF => "FF".into(),
        ty::OO => "OO".into(),
        ty::SS => "SS".into(),
        ty::TT => "TT".into(),
        ty::IF => "IF".into(),
        ty::FI => "FI".into(),
        0x30..=0x39 => format!("E{0}E{0}", t - ty::ENGINE0_ENGINE0),
        ty::VV => "VV".into(),
        ty::VF => "VF".into(),
        ty::FV => "FV".into(),
        other => format!("?{other:02x}"),
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Op::CpDownSp { offset, size } => write!(f, "CPDOWNSP {offset}, {size}"),
            Op::RsAdd(t) => write!(f, "RSADD{}", type_suffix(*t)),
            Op::CpTopSp { offset, size } => write!(f, "CPTOPSP {offset}, {size}"),
            Op::ConstI(v) => write!(f, "CONSTI {v}"),
            Op::ConstF(v) => write!(f, "CONSTF {v:?}"),
            Op::ConstS(v) => write!(f, "CONSTS {v:?}"),
            Op::ConstO(v) => write!(f, "CONSTO {v:#x}"),
            Op::Action { routine, argc } => write!(f, "ACTION {routine}, {argc}"),
            Op::Bin { op, ty: t, size } => {
                if *t == ty::TT {
                    write!(f, "{}TT {size}", op.mnemonic())
                } else {
                    write!(f, "{}{}", op.mnemonic(), type_suffix(*t))
                }
            }
            Op::Neg(t) => write!(f, "NEG{}", type_suffix(*t)),
            Op::Comp => write!(f, "COMPI"),
            Op::MovSp(v) => write!(f, "MOVSP {v}"),
            Op::StoreStateAll(t) => write!(f, "STORE_STATEALL {t}"),
            Op::Jmp(d) => write!(f, "JMP {d:+}"),
            Op::Jsr(d) => write!(f, "JSR {d:+}"),
            Op::Jz(d) => write!(f, "JZ {d:+}"),
            Op::Retn => write!(f, "RETN"),
            Op::Destruct { size, keep_offset, keep_size } => write!(f, "DESTRUCT {size}, {keep_offset}, {keep_size}"),
            Op::NotI => write!(f, "NOTI"),
            Op::DecISp(o) => write!(f, "DECISP {o}"),
            Op::IncISp(o) => write!(f, "INCISP {o}"),
            Op::Jnz(d) => write!(f, "JNZ {d:+}"),
            Op::CpDownBp { offset, size } => write!(f, "CPDOWNBP {offset}, {size}"),
            Op::CpTopBp { offset, size } => write!(f, "CPTOPBP {offset}, {size}"),
            Op::DecIBp(o) => write!(f, "DECIBP {o}"),
            Op::IncIBp(o) => write!(f, "INCIBP {o}"),
            Op::SaveBp => write!(f, "SAVEBP"),
            Op::RestoreBp => write!(f, "RESTOREBP"),
            Op::StoreState { skip, size_bp, size_sp } => write!(f, "STORE_STATE {skip}, {size_bp}, {size_sp}"),
            Op::Nop => write!(f, "NOP"),
        }
    }
}
