# NCS virtual machine (crates/k2-script)

Sources: the torlack NCS spec (xoreos-docs/specs/torlack/ncs.html) and
xoreos src/aurora/nwscript (read for knowledge). Verified against every
compiled script in the Aspyr K2 install.

## Routine table

- Parsed at runtime from the game's own `nwscript.nss` (scripts.bif). Nothing
  derived from it is stored in the repo.
- ACTION's routine number is the declaration order of function prototypes:
  877 in K2 (K1 has 772). Index 0 is `Random`.
- K2's file declares 4 engine structures: effect, event, location, talent
  (ENGINE_STRUCTURE_0..3). There are no K2-only engine types.
- The file is Windows-1252 with CRLF. A few K2 prototypes spell types oddly:
  `SetAvailableNPCId(INT nNPC, OBJECT_ID oidNPC)`. Treat `INT` as int and
  `OBJECT_ID` as object.
- Defaults: literals (`0.0f` style floats included), named constants from the
  same file, OBJECT_SELF, OBJECT_INVALID and vector literals `[0.0,0.0,0.0]`.
  All of them resolve; the test asserts none are left unparsed.

## Encoding

- "NCS V1.0", then 0x42 + big-endian u32 file size; code starts at 13.
- All operands are big-endian. Jumps are relative to the start of the jumping
  instruction.
- CONSTO: 0 = OBJECT_SELF; 1, 0xFFFFFFFF and 0x7F000000 all mean
  OBJECT_INVALID (engine id 0x7F000000).
- EQUALTT/NEQUALTT carry an extra u16 byte size (structs and vectors).
- All 5120 NCS resources (chitin plus every module capsule) parse; no unknown
  opcodes. STORE_STATEALL (0x1C) never appears.

## Semantics that differ from the spec

- SAVEBP pushes the old BP onto the stack as a slot and RESTOREBP pops it.
  The spec says SP doesn't change, but shipped code proves otherwise: the
  #globals stub does `SAVEBP; RSADDI; JSR main; CPDOWNSP -(4*(globals+3))`,
  which only reaches the outer return slot if the saved-BP slot exists.
  BP-relative offsets address the slots below the saved one (-4 = last
  global).
- SHRIGHT on a negative value mirrors around zero (-((-a) >> b)), and
  USHRIGHT is an arithmetic shift too, following Skywing's NWNScriptLib
  notes.
- Vectors are three float slots: x deepest, z on top. Engine routines pop
  them as z, y, x.
- An `action` parameter takes no stack space. It consumes the state stored
  by the most recent STORE_STATE.

## STORE_STATE and deferred actions

`STORE_STATE skip, size_bp, size_sp` is always followed by a JMP over a code
block that ends in RETN. The block starts at `this_instruction + skip`
(skip is the type byte, always 0x10 = the 10-byte STORE_STATE plus the 6-byte
JMP). The VM captures the `size_bp` bytes of globals below BP and the top
`size_sp` bytes of locals into a `ScriptState`, along with the script and
OBJECT_SELF. The next ACTION with an `action` parameter
(DelayCommand, AssignCommand, ActionDoCommand, ...) hands it to the host as
`Value::Action`. To run it later, call `Vm::resume`: it pushes the globals,
sets BP above them, pushes the locals and runs from the block start until
the block's RETN.

## Compiler junk

The original compiler sometimes leaves slots on the stack. Example:
`901mal/a_sensor_cs` jumps out of a switch case that declared locals and
skips the case's MOVSP, so 2 slots are left. The engine tolerates this, so
the VM does too. `Outcome::leftover` reports it, and a return value is read
from slot 0 only when the script starts with the `RSADDx; JSR` entry stub.

## Results (stub host, builtins for pure routines)

- 877 routines parsed.
- 5120 scripts parsed (chitin plus all 82 modules), 3732 unique by content,
  all run to completion. 3304 deferred blocks were resumed. All 23 unique
  k_con_* conditionals return an int.
- Without the pure builtins, 28 scripts loop forever: the stub's
  GetStringLength returned 0, so padding loops never ended. That is a host
  artifact, not a VM bug, and is why `Builtins` exists.

## Routine coverage

Which routines the shipped scripts call, how often, and what k2-world does
with each (real, stub, noop, inert effect, missing) is in
docs/notes/script-coverage.md. The call counts come from a scratch scanner
over the install (not committed); `examples/rscan.rs` prints the callers of
one routine. The status column is a static read of the dispatch code, so a
"real" row has an implementation arm but was not checked line by line.
