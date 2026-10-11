//! The NCS virtual machine.
//!
//! The stack is a Vec of 4-byte slots. Byte offsets in instructions are
//! negative and relative to SP (or BP): -4 is the top slot. BP is a slot
//! count: SAVEBP sets it to the current stack length and then pushes the old
//! BP as a slot. The torlack spec says SP does not move; shipped K2 scripts
//! show it does (#globals cleans up with RESTOREBP then MOVSP of exactly the
//! globals, and CPDOWNSP offsets across the call count the extra slot).

use std::sync::Arc;

use crate::ncs::{ty, BinOp, Instr, Ncs, Op, CODE_START};
use crate::nss::{DefaultValue, Routine, Routines, VarType};
use crate::value::{EngineValue, ObjectId, ScriptState, Value, OBJECT_INVALID};

/// Default instruction budget per run.
pub const DEFAULT_BUDGET: u64 = 2_000_000;

/// One engine call as seen by the host.
pub struct Call<'a> {
    pub caller: ObjectId,
    pub script: &'a Arc<Ncs>,
    pub routine: &'a Routine,
    /// One value per declared parameter, defaults filled in. Vectors arrive
    /// as `Value::Vector`, action parameters as `Value::Action`.
    pub args: &'a [Value],
}

/// The engine side of script execution.
pub trait ScriptHost {
    /// Runs engine routine `call.routine`. Return a value fitting
    /// `call.routine.ret` (Value::Void for void routines); a mismatching
    /// value is replaced with the type's default.
    fn call(&mut self, call: Call<'_>) -> Value;
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum VmErrorKind {
    #[error("stack underflow")]
    StackUnderflow,
    #[error("bad stack offset {0}")]
    BadOffset(i32),
    #[error("type mismatch: {0}")]
    TypeMismatch(String),
    #[error("jump to {0:#x}, which is not an instruction start")]
    BadJump(u32),
    #[error("unknown engine routine {0}")]
    UnknownRoutine(u16),
    #[error("routine {0} called with {1} args, declares {2}")]
    BadArgCount(String, u8, usize),
    #[error("action argument with no STORE_STATE before it")]
    NoStoredState,
    #[error("division by zero")]
    DivByZero,
    #[error("instruction budget exhausted")]
    Budget,
    #[error("unsupported instruction {0}")]
    Unsupported(String),
    #[error("ran off the end of the script")]
    FellOff,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{script}@{offset:#x}: {kind}")]
pub struct VmError {
    pub script: String,
    pub offset: u32,
    pub kind: VmErrorKind,
}

/// Result of a completed run.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The single slot left on the stack, if any (StartingConditional's int).
    pub ret: Option<Value>,
    /// Instructions executed.
    pub steps: u64,
    /// Slots left on the stack besides the return value (compiler junk).
    pub leftover: usize,
}

impl Outcome {
    /// Truth value for conditional scripts: a non-zero int return.
    pub fn truthy(&self) -> bool {
        matches!(self.ret, Some(Value::Int(v)) if v != 0)
    }
}

pub struct Vm<'r> {
    routines: &'r Routines,
    pub budget: u64,
}

type R<T> = Result<T, VmErrorKind>;

impl<'r> Vm<'r> {
    pub fn new(routines: &'r Routines) -> Self {
        Self { routines, budget: DEFAULT_BUDGET }
    }

    /// Runs a script from its entry point with `caller` as OBJECT_SELF.
    pub fn run(&self, script: &Arc<Ncs>, host: &mut dyn ScriptHost, caller: ObjectId) -> Result<Outcome, VmError> {
        let mut ex = Exec::new(self, script, caller);
        let start = script.at(CODE_START).ok_or_else(|| ex.err(VmErrorKind::FellOff))?;
        ex.pc = start;
        let steps = ex.execute(host)?;
        // A script with a return value starts with RSADDx; JSR (the slot the
        // entry stub reserves). Anything above slot 0 at exit is junk left by
        // the original compiler, e.g. `break` out of a switch case that
        // declared locals skips the case's MOVSP. The engine tolerates it.
        let returns = matches!(
            (script.instrs.first().map(|i| &i.op), script.instrs.get(1).map(|i| &i.op)),
            (Some(Op::RsAdd(_)), Some(Op::Jsr(_)))
        );
        let leftover = ex.stack.len().saturating_sub(returns as usize);
        let ret = if returns { ex.stack.first().cloned() } else { None };
        Ok(Outcome { ret, steps, leftover })
    }

    /// Runs a conditional script (dialog Active, k_con_*): true when it
    /// returns a non-zero int.
    pub fn run_conditional(&self, script: &Arc<Ncs>, host: &mut dyn ScriptHost, caller: ObjectId) -> Result<bool, VmError> {
        self.run(script, host, caller).map(|o| o.truthy())
    }

    /// Runs a deferred block captured by STORE_STATE.
    pub fn resume(&self, state: &ScriptState, host: &mut dyn ScriptHost) -> Result<Outcome, VmError> {
        let mut ex = Exec::new(self, &state.script, state.caller);
        ex.stack.extend(state.globals.iter().cloned());
        ex.bp = ex.stack.len();
        ex.stack.extend(state.locals.iter().cloned());
        ex.pc = state.script.at(state.offset).ok_or_else(|| ex.err(VmErrorKind::BadJump(state.offset)))?;
        let steps = ex.execute(host)?;
        // Deferred blocks leave their captured frame behind; nothing to check.
        Ok(Outcome { ret: None, steps, leftover: 0 })
    }
}

struct Exec<'a, 'r> {
    vm: &'a Vm<'r>,
    script: &'a Arc<Ncs>,
    caller: ObjectId,
    stack: Vec<Value>,
    bp: usize,
    returns: Vec<usize>,
    pc: usize,
    stored: Option<ScriptState>,
}

impl<'a, 'r> Exec<'a, 'r> {
    fn new(vm: &'a Vm<'r>, script: &'a Arc<Ncs>, caller: ObjectId) -> Self {
        Self {
            vm,
            script,
            caller,
            stack: Vec::with_capacity(256),
            bp: 0,
            returns: Vec::new(),
            pc: 0,
            stored: None,
        }
    }

    fn err(&self, kind: VmErrorKind) -> VmError {
        let offset = self.script.instrs.get(self.pc).map_or(0, |i| i.offset);
        VmError { script: self.script.name.clone(), offset, kind }
    }

    fn execute(&mut self, host: &mut dyn ScriptHost) -> Result<u64, VmError> {
        let mut steps = 0u64;
        loop {
            if steps >= self.vm.budget {
                return Err(self.err(VmErrorKind::Budget));
            }
            steps += 1;
            let script = self.script;
            let Some(instr) = script.instrs.get(self.pc) else {
                return Err(self.err(VmErrorKind::FellOff));
            };
            match self.step(instr, host) {
                Ok(true) => {}
                Ok(false) => return Ok(steps),
                Err(kind) => return Err(self.err(kind)),
            }
        }
    }

    // --- stack helpers -------------------------------------------------

    fn pop(&mut self) -> R<Value> {
        self.stack.pop().ok_or(VmErrorKind::StackUnderflow)
    }

    fn pop_int(&mut self) -> R<i32> {
        match self.pop()? {
            Value::Int(v) => Ok(v),
            other => Err(VmErrorKind::TypeMismatch(format!("expected int, got {other:?}"))),
        }
    }

    fn pop_float(&mut self) -> R<f32> {
        match self.pop()? {
            Value::Float(v) => Ok(v),
            other => Err(VmErrorKind::TypeMismatch(format!("expected float, got {other:?}"))),
        }
    }

    fn pop_vector(&mut self) -> R<[f32; 3]> {
        let z = self.pop_float()?;
        let y = self.pop_float()?;
        let x = self.pop_float()?;
        Ok([x, y, z])
    }

    fn push_vector(&mut self, v: [f32; 3]) {
        self.stack.extend(v.map(Value::Float));
    }

    /// Slot index for a negative byte offset relative to `base` (a slot count).
    fn slot(base: usize, offset: i32) -> R<usize> {
        if offset >= 0 || offset % 4 != 0 {
            return Err(VmErrorKind::BadOffset(offset));
        }
        let idx = base as i64 + (offset / 4) as i64;
        if idx < 0 {
            return Err(VmErrorKind::BadOffset(offset));
        }
        Ok(idx as usize)
    }

    fn slots(size: u16) -> R<usize> {
        if !size.is_multiple_of(4) {
            return Err(VmErrorKind::BadOffset(size as i32));
        }
        Ok(size as usize / 4)
    }

    fn copy_down(&mut self, base: usize, offset: i32, size: u16) -> R<()> {
        let n = Self::slots(size)?;
        let dst = Self::slot(base, offset)?;
        let src = self.stack.len().checked_sub(n).ok_or(VmErrorKind::StackUnderflow)?;
        if dst + n > self.stack.len() {
            return Err(VmErrorKind::BadOffset(offset));
        }
        for k in 0..n {
            self.stack[dst + k] = self.stack[src + k].clone();
        }
        Ok(())
    }

    fn copy_top(&mut self, base: usize, offset: i32, size: u16) -> R<()> {
        let n = Self::slots(size)?;
        let src = Self::slot(base, offset)?;
        if src + n > self.stack.len() {
            return Err(VmErrorKind::BadOffset(offset));
        }
        for k in 0..n {
            let v = self.stack[src + k].clone();
            self.stack.push(v);
        }
        Ok(())
    }

    fn add_int_at(&mut self, base: usize, offset: i32, delta: i32) -> R<()> {
        let i = Self::slot(base, offset)?;
        match self.stack.get_mut(i) {
            Some(Value::Int(v)) => {
                *v = v.wrapping_add(delta);
                Ok(())
            }
            Some(other) => Err(VmErrorKind::TypeMismatch(format!("inc/dec on {other:?}"))),
            None => Err(VmErrorKind::BadOffset(offset)),
        }
    }

    fn jump(&mut self, instr: &Instr) -> R<()> {
        let target = instr.jump_target().expect("jump op");
        self.pc = self.script.at(target).ok_or(VmErrorKind::BadJump(target))?;
        Ok(())
    }

    // --- execution -----------------------------------------------------

    /// Executes one instruction. Ok(false) means the script finished.
    fn step(&mut self, instr: &Instr, host: &mut dyn ScriptHost) -> R<bool> {
        let next = self.pc + 1;
        match &instr.op {
            Op::CpDownSp { offset, size } => self.copy_down(self.stack.len(), *offset, *size)?,
            Op::CpTopSp { offset, size } => self.copy_top(self.stack.len(), *offset, *size)?,
            Op::CpDownBp { offset, size } => self.copy_down(self.bp, *offset, *size)?,
            Op::CpTopBp { offset, size } => self.copy_top(self.bp, *offset, *size)?,
            Op::RsAdd(t) => {
                let v = match *t {
                    ty::INT => Value::Int(0),
                    ty::FLOAT => Value::Float(0.0),
                    ty::STRING => Value::String(Arc::from("")),
                    ty::OBJECT => Value::Object(OBJECT_INVALID),
                    0x10..=0x1F => Value::Engine(EngineValue { kind: t - ty::ENGINE0, handle: 0 }),
                    _ => return Err(VmErrorKind::Unsupported(instr.op.to_string())),
                };
                self.stack.push(v);
            }
            Op::ConstI(v) => self.stack.push(Value::Int(*v)),
            Op::ConstF(v) => self.stack.push(Value::Float(*v)),
            Op::ConstS(v) => self.stack.push(Value::String(Arc::from(v.as_str()))),
            Op::ConstO(v) => self.stack.push(Value::Object(match *v {
                0 => self.caller,
                1 | 0xFFFF_FFFF | OBJECT_INVALID => OBJECT_INVALID,
                other => other,
            })),
            Op::Action { routine, argc } => self.action(*routine, *argc, host)?,
            Op::Bin { op, ty: t, size } => self.binop(*op, *t, *size)?,
            Op::Neg(t) => match *t {
                ty::INT => {
                    let v = self.pop_int()?;
                    self.stack.push(Value::Int(v.wrapping_neg()));
                }
                ty::FLOAT => {
                    let v = self.pop_float()?;
                    self.stack.push(Value::Float(-v));
                }
                _ => return Err(VmErrorKind::Unsupported(instr.op.to_string())),
            },
            Op::Comp => {
                let v = self.pop_int()?;
                self.stack.push(Value::Int(!v));
            }
            Op::NotI => {
                let v = self.pop_int()?;
                self.stack.push(Value::Int((v == 0) as i32));
            }
            Op::MovSp(o) => {
                if *o > 0 || o % 4 != 0 {
                    return Err(VmErrorKind::BadOffset(*o));
                }
                let n = (-*o / 4) as usize;
                let len = self.stack.len().checked_sub(n).ok_or(VmErrorKind::StackUnderflow)?;
                self.stack.truncate(len);
            }
            Op::Jmp(_) => {
                self.jump(instr)?;
                return Ok(true);
            }
            Op::Jsr(_) => {
                self.returns.push(next);
                self.jump(instr)?;
                return Ok(true);
            }
            Op::Jz(_) | Op::Jnz(_) => {
                let v = self.pop_int()?;
                let take = if matches!(instr.op, Op::Jz(_)) { v == 0 } else { v != 0 };
                if take {
                    self.jump(instr)?;
                    return Ok(true);
                }
            }
            Op::Retn => match self.returns.pop() {
                Some(ret) => {
                    self.pc = ret;
                    return Ok(true);
                }
                None => return Ok(false),
            },
            Op::Destruct { size, keep_offset, keep_size } => {
                if size % 4 != 0 || keep_offset % 4 != 0 || keep_size % 4 != 0 || *size < 0 || *keep_offset < 0 || *keep_size < 0 {
                    return Err(VmErrorKind::BadOffset(*size as i32));
                }
                let n = *size as usize / 4;
                let start = self.stack.len().checked_sub(n).ok_or(VmErrorKind::StackUnderflow)?;
                let k0 = start + *keep_offset as usize / 4;
                let k1 = k0 + *keep_size as usize / 4;
                if k1 > self.stack.len() {
                    return Err(VmErrorKind::BadOffset(*keep_offset as i32));
                }
                let kept: Vec<Value> = self.stack[k0..k1].to_vec();
                self.stack.truncate(start);
                self.stack.extend(kept);
            }
            Op::DecISp(o) => self.add_int_at(self.stack.len(), *o, -1)?,
            Op::IncISp(o) => self.add_int_at(self.stack.len(), *o, 1)?,
            Op::DecIBp(o) => self.add_int_at(self.bp, *o, -1)?,
            Op::IncIBp(o) => self.add_int_at(self.bp, *o, 1)?,
            Op::SaveBp => {
                // The old BP is pushed as a slot (compiled code accounts for
                // it: #globals pops it with RESTOREBP before MOVSP-ing the
                // globals). BP addresses the slots below the saved one.
                let old = self.bp;
                self.bp = self.stack.len();
                self.stack.push(Value::Int(old as i32));
            }
            Op::RestoreBp => match self.pop()? {
                Value::Int(v) if v >= 0 => self.bp = v as usize,
                other => return Err(VmErrorKind::TypeMismatch(format!("RESTOREBP popped {other:?}"))),
            },
            Op::StoreState { skip, size_bp, size_sp } => {
                let nbp = *size_bp as usize / 4;
                let nsp = *size_sp as usize / 4;
                let g0 = self.bp.checked_sub(nbp).ok_or(VmErrorKind::StackUnderflow)?;
                let l0 = self.stack.len().checked_sub(nsp).ok_or(VmErrorKind::StackUnderflow)?;
                self.stored = Some(ScriptState {
                    script: self.script.clone(),
                    offset: instr.offset + *skip as u32,
                    globals: self.stack[g0..self.bp].to_vec(),
                    locals: self.stack[l0..].to_vec(),
                    caller: self.caller,
                });
            }
            Op::StoreStateAll(_) => return Err(VmErrorKind::Unsupported(instr.op.to_string())),
            Op::Nop => {}
        }
        self.pc = next;
        Ok(true)
    }

    fn action(&mut self, id: u16, argc: u8, host: &mut dyn ScriptHost) -> R<()> {
        let routines = self.vm.routines;
        let routine = routines.get(id).ok_or(VmErrorKind::UnknownRoutine(id))?;
        if argc as usize > routine.params.len() || (argc as usize) < routine.min_args() {
            return Err(VmErrorKind::BadArgCount(routine.name.clone(), argc, routine.params.len()));
        }
        let mut args = Vec::with_capacity(routine.params.len());
        for (i, p) in routine.params.iter().enumerate() {
            let v = if i < argc as usize {
                match p.ty {
                    VarType::Action => Value::Action(Arc::new(self.stored.take().ok_or(VmErrorKind::NoStoredState)?)),
                    VarType::Vector => Value::Vector(self.pop_vector()?),
                    t => {
                        let v = self.pop()?;
                        if !v.fits(t) {
                            return Err(VmErrorKind::TypeMismatch(format!("{} arg {i} ({}): {v:?}", routine.name, p.name)));
                        }
                        v
                    }
                }
            } else {
                self.default_arg(p.ty, p.default.as_ref())
            };
            args.push(v);
        }
        let ret = host.call(Call { caller: self.caller, script: self.script, routine, args: &args });
        let ret = if ret.fits(routine.ret) { ret } else { Value::default_for(routine.ret) };
        match ret {
            Value::Void => {}
            Value::Vector(v) => self.push_vector(v),
            v => self.stack.push(v),
        }
        Ok(())
    }

    fn default_arg(&self, ty: VarType, d: Option<&DefaultValue>) -> Value {
        match (d, ty) {
            (Some(DefaultValue::Int(v)), VarType::Int) => Value::Int(*v),
            (Some(DefaultValue::Int(v)), VarType::Float) => Value::Float(*v as f32),
            (Some(DefaultValue::Float(v)), VarType::Float) => Value::Float(*v),
            (Some(DefaultValue::String(s)), VarType::String) => Value::String(Arc::from(s.as_str())),
            (Some(DefaultValue::ObjectSelf), VarType::Object) => Value::Object(self.caller),
            (Some(DefaultValue::ObjectInvalid), VarType::Object) => Value::Object(OBJECT_INVALID),
            (Some(DefaultValue::Vector(v)), VarType::Vector) => Value::Vector(*v),
            _ => Value::default_for(ty),
        }
    }

    fn binop(&mut self, op: BinOp, t: u8, size: u16) -> R<()> {
        use BinOp::*;
        let mismatch = || VmErrorKind::TypeMismatch(format!("{}{}", op.mnemonic(), crate::ncs::type_suffix(t)));
        // Struct / vector equality: compare `size` bytes slot by slot.
        if t == ty::TT {
            if !matches!(op, Equal | NEqual) {
                return Err(mismatch());
            }
            let n = Self::slots(size)?;
            let len = self.stack.len();
            if len < 2 * n {
                return Err(VmErrorKind::StackUnderflow);
            }
            let eq = self.stack[len - n..] == self.stack[len - 2 * n..len - n];
            self.stack.truncate(len - 2 * n);
            self.stack.push(Value::Int((eq == matches!(op, Equal)) as i32));
            return Ok(());
        }
        match t {
            ty::VV => {
                let b = self.pop_vector()?;
                let a = self.pop_vector()?;
                match op {
                    Add => self.push_vector([a[0] + b[0], a[1] + b[1], a[2] + b[2]]),
                    Sub => self.push_vector([a[0] - b[0], a[1] - b[1], a[2] - b[2]]),
                    Equal => self.stack.push(Value::Int((a == b) as i32)),
                    NEqual => self.stack.push(Value::Int((a != b) as i32)),
                    _ => return Err(mismatch()),
                }
                return Ok(());
            }
            ty::VF => {
                let f = self.pop_float()?;
                let a = self.pop_vector()?;
                match op {
                    Mul => self.push_vector(a.map(|c| c * f)),
                    Div => self.push_vector(a.map(|c| c / f)),
                    _ => return Err(mismatch()),
                }
                return Ok(());
            }
            ty::FV => {
                let a = self.pop_vector()?;
                let f = self.pop_float()?;
                match op {
                    Mul => self.push_vector(a.map(|c| c * f)),
                    _ => return Err(mismatch()),
                }
                return Ok(());
            }
            _ => {}
        }
        let rhs = self.pop()?;
        let lhs = self.pop()?;
        let out = match (t, lhs, rhs) {
            (ty::II, Value::Int(a), Value::Int(b)) => Value::Int(match op {
                LogAnd => (a != 0 && b != 0) as i32,
                LogOr => (a != 0 || b != 0) as i32,
                IncOr => a | b,
                ExcOr => a ^ b,
                BoolAnd => a & b,
                Equal => (a == b) as i32,
                NEqual => (a != b) as i32,
                Geq => (a >= b) as i32,
                Gt => (a > b) as i32,
                Lt => (a < b) as i32,
                Leq => (a <= b) as i32,
                ShLeft => a.wrapping_shl(b as u32),
                // Both right shifts are arithmetic in the original engine;
                // SHRIGHT also mirrors negative values (see notes).
                ShRight => {
                    if a < 0 {
                        -(a.wrapping_neg().wrapping_shr(b as u32))
                    } else {
                        a.wrapping_shr(b as u32)
                    }
                }
                UShRight => a.wrapping_shr(b as u32),
                Add => a.wrapping_add(b),
                Sub => a.wrapping_sub(b),
                Mul => a.wrapping_mul(b),
                Div => {
                    if b == 0 {
                        return Err(VmErrorKind::DivByZero);
                    }
                    a.wrapping_div(b)
                }
                Mod => {
                    if b == 0 {
                        return Err(VmErrorKind::DivByZero);
                    }
                    a.wrapping_rem(b)
                }
            }),
            (ty::FF | ty::IF | ty::FI, l, r) => {
                let a = match l {
                    Value::Int(v) if t == ty::IF => v as f32,
                    Value::Float(v) if t != ty::IF => v,
                    _ => return Err(mismatch()),
                };
                let b = match r {
                    Value::Int(v) if t == ty::FI => v as f32,
                    Value::Float(v) if t != ty::FI => v,
                    _ => return Err(mismatch()),
                };
                match op {
                    Equal => Value::Int((a == b) as i32),
                    NEqual => Value::Int((a != b) as i32),
                    Geq => Value::Int((a >= b) as i32),
                    Gt => Value::Int((a > b) as i32),
                    Lt => Value::Int((a < b) as i32),
                    Leq => Value::Int((a <= b) as i32),
                    Add => Value::Float(a + b),
                    Sub => Value::Float(a - b),
                    Mul => Value::Float(a * b),
                    Div => Value::Float(a / b),
                    _ => return Err(mismatch()),
                }
            }
            (ty::SS, Value::String(a), Value::String(b)) => match op {
                Add => Value::String(Arc::from(format!("{a}{b}"))),
                Equal => Value::Int((a == b) as i32),
                NEqual => Value::Int((a != b) as i32),
                _ => return Err(mismatch()),
            },
            (ty::OO, Value::Object(a), Value::Object(b)) => match op {
                Equal => Value::Int((a == b) as i32),
                NEqual => Value::Int((a != b) as i32),
                _ => return Err(mismatch()),
            },
            (0x30..=0x3F, Value::Engine(a), Value::Engine(b)) => match op {
                Equal => Value::Int((a == b) as i32),
                NEqual => Value::Int((a != b) as i32),
                _ => return Err(mismatch()),
            },
            _ => return Err(mismatch()),
        };
        self.stack.push(out);
        Ok(())
    }
}
