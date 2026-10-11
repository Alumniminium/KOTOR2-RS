//! VM semantics on hand-assembled bytecode.

use std::sync::Arc;

use k2_script::{Call, Ncs, Routines, ScriptHost, StubHost, Value, Vm};

const NSS: &[u8] = b"
#define ENGINE_NUM_STRUCTURES 1
#define ENGINE_STRUCTURE_0 effect
int TRUE = 1;
int Random(int nMax);
void DelayCommand(float fSeconds, action aAction);
string IntToString(int n);
vector Vector(float x=0.0f, float y=0.0f, float z=0.0f);
void Mark(int n, object o = OBJECT_SELF);
";

/// Tiny assembler: big-endian operands, NCS header prepended.
#[derive(Default)]
struct Asm(Vec<u8>);

impl Asm {
    fn op(mut self, op: u8, ty: u8) -> Self {
        self.0.extend([op, ty]);
        self
    }
    fn i32(mut self, v: i32) -> Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    fn u16(mut self, v: u16) -> Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }
    fn f32(mut self, v: f32) -> Self {
        self.0.extend(v.to_bits().to_be_bytes());
        self
    }
    fn consti(self, v: i32) -> Self {
        self.op(0x04, 0x03).i32(v)
    }
    fn build(self) -> Arc<Ncs> {
        let mut b = b"NCS V1.0".to_vec();
        b.push(0x42);
        b.extend(((self.0.len() + 13) as u32).to_be_bytes());
        b.extend(self.0);
        Arc::new(Ncs::parse("t", &b).expect("parse"))
    }
}

#[derive(Default)]
struct Rec {
    stub: StubHost,
    marks: Vec<(i32, u32)>,
}

impl ScriptHost for Rec {
    fn call(&mut self, call: Call<'_>) -> Value {
        if call.routine.name == "Mark" {
            self.marks.push((call.args[0].as_int().unwrap(), call.args[1].as_object().unwrap()));
            return Value::Void;
        }
        self.stub.call(call)
    }
}

fn routines() -> Routines {
    Routines::parse(NSS).expect("nss")
}

#[test]
fn parses_tiny_nss() {
    let r = routines();
    assert_eq!(r.len(), 5);
    assert_eq!(r.get(3).unwrap().name, "Vector");
    assert_eq!(r.get(4).unwrap().min_args(), 1);
}

#[test]
fn conditional_with_globals_and_arithmetic() {
    // int g = 40; int StartingConditional() { return g + 2 == 42; }
    let ncs = Asm::default()
        .op(0x02, 0x03) // 0d RSADDI (return slot)
        .op(0x1E, 0).i32(8) // 0f JSR #globals -> 17
        .op(0x20, 0) // 15 RETN
        .op(0x02, 0x03) // 17 RSADDI g
        .consti(40) // 19
        .op(0x01, 0x01).i32(-8).u16(4) // 1f CPDOWNSP -8,4
        .op(0x1B, 0).i32(-4) // 27 MOVSP -4
        .op(0x2A, 0) // 2d SAVEBP (pushes old BP)
        .op(0x02, 0x03) // 2f RSADDI (main ret)
        .op(0x1E, 0).i32(30) // 31 JSR main -> 4f
        .op(0x01, 0x01).i32(-16).u16(4) // 37 CPDOWNSP -16,4 (outer ret)
        .op(0x1B, 0).i32(-4) // 3f MOVSP -4
        .op(0x2B, 0) // 45 RESTOREBP
        .op(0x1B, 0).i32(-4) // 47 MOVSP -4 (globals)
        .op(0x20, 0) // 4d RETN
        .op(0x27, 0x01).i32(-4).u16(4) // 4f CPTOPBP -4,4 (g)
        .consti(2)
        .op(0x14, 0x20) // ADDII
        .consti(42)
        .op(0x0B, 0x20) // EQUALII
        .op(0x01, 0x01).i32(-8).u16(4) // CPDOWNSP -8,4 (main ret)
        .op(0x1B, 0).i32(-4)
        .op(0x20, 0)
        .build();
    let r = routines();
    let vm = Vm::new(&r);
    let out = vm.run(&ncs, &mut StubHost::default(), 7).expect("run");
    assert_eq!(out.ret, Some(Value::Int(1)));
    assert_eq!(out.leftover, 0);
}

#[test]
fn store_state_and_resume() {
    // void main() { int n = 5; DelayCommand(1.0, Mark(n)); }
    let ncs = Asm::default()
        .op(0x1E, 0).i32(8)
        .op(0x20, 0)
        .op(0x02, 0x03)
        .consti(5)
        .op(0x01, 0x01).i32(-8).u16(4)
        .op(0x1B, 0).i32(-4)
        .op(0x2C, 0x10).i32(0).i32(4)
        .op(0x1D, 0).i32(21) // 35 JMP -> 4a
        .op(0x03, 0x01).i32(-4).u16(4) // 3b
        .op(0x05, 0).u16(4).u8(1) // 43
        .op(0x20, 0) // 48
        .op(0x04, 0x04).f32(1.0) // 4a CONSTF 1.0
        .op(0x05, 0).u16(1).u8(2) // 50 ACTION DelayCommand
        .op(0x1B, 0).i32(-4) // 55 MOVSP -4 (n)
        .op(0x20, 0) // 5b RETN
        .build();
    let r = routines();
    let vm = Vm::new(&r);
    let mut host = Rec::default();
    vm.run(&ncs, &mut host, 9).expect("run");
    assert!(host.marks.is_empty());
    assert_eq!(host.stub.deferred.len(), 1);
    let state = host.stub.deferred.pop().unwrap();
    assert_eq!(state.offset, 0x3b);
    vm.resume(&state, &mut host).expect("resume");
    assert_eq!(host.marks, vec![(5, 9)], "deferred Mark(n) with OBJECT_SELF default");
}

#[test]
fn destruct_and_struct_equality() {
    // Push 1 2 3, keep the middle: DESTRUCT 12, 4, 4 -> [2].
    // Then push 2 and compare as 1-slot structs: EQUALTT 4 -> 1.
    let ncs = Asm::default()
        .consti(1)
        .consti(2)
        .consti(3)
        .op(0x21, 0x01).u16(12).u16(4).u16(4)
        .consti(2)
        .op(0x0B, 0x24).u16(4)
        .op(0x20, 0)
        .build();
    let r = routines();
    let vm = Vm::new(&r);
    // Not a conditional entry stub, so ret is None; inspect leftover instead.
    let out = vm.run(&ncs, &mut StubHost::default(), 0).expect("run");
    assert_eq!(out.leftover, 1);
}

#[test]
fn vector_routine_roundtrip() {
    // v = Vector(1,2,3); (v - v) == Vector() as a 12-byte EQUALTT -> 1.
    let r = Routines::parse(b"vector Vector(float x=0.0f, float y=0.0f, float z=0.0f);").unwrap();
    let ncs = Asm::default()
        .op(0x02, 0x03) // ret slot
        .op(0x1E, 0).i32(8)
        .op(0x20, 0)
        .op(0x04, 0x04).f32(3.0)
        .op(0x04, 0x04).f32(2.0)
        .op(0x04, 0x04).f32(1.0)
        .op(0x05, 0).u16(0).u8(3) // Vector(1,2,3)
        .op(0x03, 0x01).i32(-12).u16(12) // copy it
        .op(0x15, 0x3A) // SUBVV -> 0,0,0
        .op(0x05, 0).u16(0).u8(0) // Vector() -> 0,0,0
        .op(0x0B, 0x24).u16(12) // EQUALTT 12
        .op(0x01, 0x01).i32(-8).u16(4) // into the ret slot
        .op(0x1B, 0).i32(-4)
        .op(0x20, 0)
        .build();
    let vm = Vm::new(&r);
    let out = vm.run(&ncs, &mut StubHost::default(), 0).expect("run");
    assert_eq!(out.ret, Some(Value::Int(1)));
}
