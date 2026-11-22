//! Deciding whether a function can be hooked, from its own machine code.
//!
//! An inline hook overwrites the first bytes of a function with a branch to our
//! handler, and runs the bytes it displaced from a trampoline. Three things can
//! make that silently wrong, and all three are properties of the real bytes
//! rather than of the design:
//!
//! * **the function is shorter than the patch.** `GameMode::tick` in the real
//!   0.14.3 library is six bytes (`ldr r3,[r0,#36]; str r3,[r0,#32]; bx lr`), so
//!   an eight-byte patch would write into whatever follows it. This cannot be
//!   seen in the bytes at all — only the ELF symbol's `st_size` knows where the
//!   function ends — so the length is an input, not an inference.
//! * **a displaced instruction reads the program counter.** `adr`, `ldr rX,[pc,#n]`,
//!   `cbz` and branches are relative to where they sit, so copying them elsewhere
//!   changes what they mean.
//! * **the window ends inside an instruction.** Thumb-2 mixes 16- and 32-bit
//!   encodings, so "eight bytes" is not necessarily a whole number of them.
//!
//! So this planner walks real instruction boundaries, copies whole instructions,
//! and *refuses* — naming the offset and the reason — when it meets anything it
//! cannot prove is safe. A refused function is a function left alone, which is
//! the only acceptable failure mode for patching someone else's process.
//!
//! The accepted encodings are exactly the ones the measured prologues contain,
//! read out of `libminecraftpe.so` of Pocket Edition 0.14.3. Anything else is
//! reported rather than guessed at.

/// Bytes an `LDR.W PC, [PC, #imm]` patch plus its literal occupies.
pub const PATCH_LEN: usize = 8;

/// Longest prologue window this will copy.
pub const MAX_WINDOW: usize = 16;

/// Why a function cannot be hooked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not enough whole instructions to fit the patch.
    TooShort { available: usize, needed: usize },
    /// An instruction that reads the program counter, at this offset.
    PcRelative { offset: usize, what: &'static str },
    /// An encoding this planner does not claim to understand.
    Unknown { offset: usize, halfword: u16 },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::TooShort { available, needed } => {
                write!(f, "function is {available} bytes, patch needs {needed}")
            }
            Refusal::PcRelative { offset, what } => {
                write!(f, "instruction at +{offset} is {what}, which moves when copied")
            }
            Refusal::Unknown { offset, halfword } => {
                write!(f, "unrecognised encoding {halfword:#06x} at +{offset}")
            }
        }
    }
}

/// A prologue that can be displaced safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// Whole instructions, at least [`PATCH_LEN`] bytes, at most [`MAX_WINDOW`].
    pub bytes: Vec<u8>,
}

impl Window {
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// A Thumb-2 instruction is 32 bits when the top five bits of its first
/// halfword are 0b11101, 0b11110 or 0b11111; everything else is 16 bits.
pub fn instruction_len(first: u16) -> usize {
    match first >> 11 {
        0b11101 | 0b11110 | 0b11111 => 4,
        _ => 2,
    }
}

enum Kind {
    Safe,
    Pc(&'static str),
    Unknown,
}

/// Whether one instruction may be copied to a trampoline unchanged.
///
/// The safe set is narrow on purpose: the forms the measured prologues use.
/// Everything else is reported so a new game build fails loudly instead of
/// being patched into a crash.
fn classify(hw: u16, second: u16) -> Kind {
    if instruction_len(hw) == 2 {
        return match hw >> 12 {
            // LSL/LSR/ASR, ADD/SUB register; MOV/CMP/ADD/SUB immediate.
            0x0 | 0x1 | 0x2 | 0x3 => Kind::Safe,
            // 0100: with bit 11 set this is `LDR (literal)` — PC relative. Without
            // it, it is data processing on registers.
            0x4 => {
                if (hw >> 11) & 1 == 1 {
                    Kind::Pc("ldr literal")
                } else {
                    Kind::Safe
                }
            }
            // 0101: data processing; includes BX/BLX which branch but take the
            // address from a register, so they relocate unchanged.
            0x5 => Kind::Safe,
            // 0110-1001: LDR/STR/LDRB/STRB/LDRH/STRH and SP-relative forms.
            0x6 | 0x7 | 0x8 | 0x9 => Kind::Safe,
            // 1010: ADR is PC relative; ADD/SUB SP immediate is not.
            0xA => {
                if (hw >> 11) & 1 == 1 {
                    Kind::Safe
                } else {
                    Kind::Pc("adr")
                }
            }
            // 1011: PUSH, POP, extend, REV, IT, hints are all safe; CBZ/CBNZ
            // are PC-relative branches.
            0xB => match hw & 0xFF00 {
                0xB100 | 0xB900 => Kind::Pc("cbz/cbnz"),
                _ => Kind::Safe,
            },
            // 1100: STM/LDM.
            0xC => Kind::Safe,
            0xD => Kind::Pc("conditional branch"),
            0xE => Kind::Pc("branch"),
            // 1111: SVC and the hint space.
            0xF => Kind::Safe,
            _ => Kind::Unknown,
        };
    }

    // ---- 32-bit encodings -------------------------------------------------
    //
    // ADR.W (and the PC-relative ADD/SUB forms) are relative by construction.
    if (hw & 0xFBFF) == 0xF20F || (hw & 0xFBFF) == 0xF2AF {
        return Kind::Pc("adr.w");
    }
    // TBB/TBH and the 32-bit branch family.
    if (hw & 0xF800) == 0xF000 && (second & 0x8000) != 0 && (hw & 0x0380) != 0 {
        return Kind::Pc("branch-32");
    }
    // LDR/STR (immediate) with Rn = PC is the literal form. The same encoding
    // with any other base register is safe.
    if (hw & 0xFE00) == 0xF800 || (hw & 0xFE00) == 0xF900 {
        return if hw & 0x000F == 0x000F {
            Kind::Pc("ldr literal (32-bit)")
        } else {
            Kind::Safe
        };
    }
    // STMDB/LDMIA on SP, and the VFP load/store group (vpush, vpop, vldr, vstr).
    if hw == 0xE92D || hw == 0xE8BD || (hw & 0xFF00) == 0xEC00 || (hw & 0xFF00) == 0xED00 {
        return Kind::Safe;
    }
    // VMOV between core and VFP registers, and VFP moves.
    if (hw & 0xFFF0) == 0xEE00 || (hw & 0xFFF0) == 0xEE10 {
        return Kind::Safe;
    }
    // ADD.W/SUB.W with an immediate, the two ALU forms in these prologues.
    if hw == 0xF500 || hw == 0xF1A0 || (hw & 0xFFF0) == 0xF1A0 {
        return Kind::Safe;
    }
    Kind::Unknown
}

/// Walk whole instructions until the patch fits, refusing anything unsafe.
///
/// `declared_len` is the symbol's `st_size` when it is known. It is the only
/// evidence of where the function ends, and a function shorter than the patch is
/// refused outright.
pub fn plan(code: &[u8], declared_len: Option<u32>) -> Result<Window, Refusal> {
    if let Some(n) = declared_len {
        if (n as usize) < PATCH_LEN {
            return Err(Refusal::TooShort {
                available: n as usize,
                needed: PATCH_LEN,
            });
        }
    }

    let mut offset = 0usize;
    while offset < PATCH_LEN {
        if offset + 2 > code.len() {
            return Err(Refusal::TooShort {
                available: code.len(),
                needed: PATCH_LEN,
            });
        }
        let hw = u16::from_le_bytes([code[offset], code[offset + 1]]);
        let len = instruction_len(hw);

        if offset + len > code.len() {
            return Err(Refusal::TooShort {
                available: code.len(),
                needed: offset + len,
            });
        }
        if offset + len > MAX_WINDOW {
            return Err(Refusal::TooShort {
                available: MAX_WINDOW,
                needed: offset + len,
            });
        }

        let second = if len == 4 {
            u16::from_le_bytes([code[offset + 2], code[offset + 3]])
        } else {
            0
        };
        match classify(hw, second) {
            Kind::Safe => {}
            Kind::Pc(what) => return Err(Refusal::PcRelative { offset, what }),
            Kind::Unknown => return Err(Refusal::Unknown { offset, halfword: hw }),
        }
        offset += len;
    }

    // The branch back must land on an instruction boundary, which `offset` is.
    Ok(Window {
        bytes: code[..offset].to_vec(),
    })
}

/// The bytes that replace a function's first instruction: `ldr.w pc, [pc, #imm]`
/// followed by the absolute address of the handler.
///
/// The literal has to sit immediately after the instruction, and the PC a Thumb
/// instruction reads is the aligned address of the following instruction, so the
/// immediate depends on the function's alignment. Getting this wrong jumps to a
/// garbage address, which is why it is computed rather than written as a constant.
pub fn patch_bytes(func_addr: u32, hook_addr: u32) -> [u8; PATCH_LEN] {
    let pc = func_addr.wrapping_add(4) & !3;
    let literal = func_addr.wrapping_add(4);
    let imm = literal.wrapping_sub(pc) as u16; // 0 or 2

    let first: u16 = 0xF8DF; // ldr.w Rt, [pc, #imm12], with Rt = PC
    let second: u16 = 0xF000 | (imm & 0x0FFF);

    let mut out = [0u8; PATCH_LEN];
    out[0..2].copy_from_slice(&first.to_le_bytes());
    out[2..4].copy_from_slice(&second.to_le_bytes());
    out[4..8].copy_from_slice(&hook_addr.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real bytes, read out of `libminecraftpe.so` of Pocket Edition 0.14.3.
    /// These are the prologues the planner has to accept or refuse correctly, so
    /// they are the fixtures — not a plausible-looking `push {r4, lr}`.
    const SET_BLOCK: &[u8] = &[
        0x70, 0xb5, // push {r4, r5, r6, lr}
        0x84, 0xb0, // sub sp, #16
        0x08, 0x9d, // ldr r5, [sp, #32]
        0x0a, 0x9c, // ldr r4, [sp, #40]
        0x9d, 0xf8, 0x24, 0x60, // ldrb.w r6, [sp, #36]
        0x2d, 0x78, // ldrb r5, [r5]
    ];

    const GET_BLOCK: &[u8] = &[
        0x00, 0xb5, // push {lr}
        0x83, 0xb0, // sub sp, #12
        0x0a, 0x46, // mov r2, r1
        0x01, 0x46, // mov r1, r0
        0x01, 0xa8, // add r0, sp, #4
        0x97, 0xfd, 0xff, 0xf7, // bl ...
    ];

    const EXPLODE: &[u8] = &[
        0x00, 0xf5, 0xaa, 0x50, // add.w r0, r0, #5440
        0x30, 0xb4, // push {r4, r5}
        0x9d, 0xed, 0x02, 0x0a, // vldr s0, [sp, #8]
        0x04, 0x7b, // ldrb r4, [r0, #12]
        0x9d, 0xf8, 0x0c, 0x50, // ldrb.w r5, [sp, #12]
    ];

    const ATTACK: &[u8] = &[
        0x70, 0xb5, // push {r4, r5, r6, lr}
        0x04, 0x46, // mov r4, r0
        0x80, 0x68, // ldr r0, [r0, #8]
        0x0e, 0x46, // mov r6, r1
        0x15, 0x46, // mov r5, r2
    ];

    const DESTROY_BLOCK: &[u8] = &[
        0x2d, 0xe9, 0xf0, 0x4f, // stmdb sp!, {r4-r11, lr}
        0x05, 0x46, // mov r5, r0
        0x89, 0xb0, // sub sp, #36
        0x08, 0x46, // mov r0, r1
        0x0c, 0x46, // mov r4, r1
    ];

    const GET_FOV: &[u8] = &[
        0x70, 0xb5, // push {r4, r5, r6, lr}
        0x2d, 0xed, 0x04, 0x8b, // vpush {d8-d9}
        0x04, 0x46, // mov r4, r0
        0x08, 0xee, 0x90, 0x1a, // vmov s17, r1
        0x00, 0xf5, 0xb1, 0x55, // add.w r5, r0, #5664
    ];

    /// `GameMode::tick` is six bytes and then the next function begins.
    const TICK: &[u8] = &[
        0x43, 0x6a, // ldr r3, [r0, #36]
        0x03, 0x62, // str r3, [r0, #32]
        0x70, 0x47, // bx lr
        0x00, 0xbf, // nop — the next function's first instruction
    ];

    fn accepted(code: &[u8]) -> Window {
        plan(code, None).unwrap_or_else(|e| panic!("should be hookable: {e}"))
    }

    #[test]
    fn every_real_prologue_is_accepted_at_a_whole_instruction_boundary() {
        for (name, code) in [
            ("setBlockAndData", SET_BLOCK),
            ("getBlock", GET_BLOCK),
            ("explode", EXPLODE),
            ("attack", ATTACK),
            ("destroyBlock", DESTROY_BLOCK),
            ("getFov", GET_FOV),
        ] {
            let w = accepted(code);
            assert!(
                w.len() >= PATCH_LEN && w.len() <= MAX_WINDOW,
                "{name}: window {} out of range",
                w.len()
            );
            // The window must end on an instruction boundary, not inside one.
            let mut off = 0;
            while off < w.len() {
                let hw = u16::from_le_bytes([w.bytes[off], w.bytes[off + 1]]);
                off += instruction_len(hw);
            }
            assert_eq!(off, w.len(), "{name}: window splits an instruction");
        }
    }

    /// The measured counter-example. `st_size` says six bytes, so an eight-byte
    /// patch would write into the function after it. The bytes alone cannot show
    /// this, which is why the length is an input.
    #[test]
    fn the_six_byte_function_is_refused_because_its_size_says_so() {
        let err = plan(TICK, Some(6)).unwrap_err();
        assert!(
            matches!(err, Refusal::TooShort { available: 6, needed: 8 }),
            "expected TooShort, got {err:?}"
        );
    }

    /// Without a declared size the planner will happily walk eight bytes — which
    /// is exactly why the size has to be carried in from the symbol table.
    #[test]
    fn without_a_declared_size_the_same_bytes_look_hookable() {
        assert!(plan(TICK, None).is_ok());
    }

    #[test]
    fn a_declared_size_of_zero_is_refused() {
        assert!(plan(SET_BLOCK, Some(0)).is_err());
    }

    #[test]
    fn a_short_pc_relative_load_is_refused() {
        // ldr r0, [pc, #8]  (01001 imm8)
        let code: &[u8] = &[0x02, 0x48, 0x00, 0xbf, 0x00, 0xbf, 0x00, 0xbf, 0x00, 0xbf];
        let err = plan(code, None).unwrap_err();
        assert!(matches!(err, Refusal::PcRelative { .. }), "got {err:?}");
    }

    #[test]
    fn adr_is_refused() {
        let code: &[u8] = &[0x01, 0xa0, 0x00, 0xbf, 0x00, 0xbf, 0x00, 0xbf, 0x00, 0xbf];
        let err = plan(code, None).unwrap_err();
        assert!(matches!(err, Refusal::PcRelative { .. }), "got {err:?}");
    }

    #[test]
    fn a_branch_inside_the_window_is_refused() {
        let code: &[u8] = &[0xfe, 0xe7, 0x00, 0xbf, 0x00, 0xbf, 0x00, 0xbf, 0x00, 0xbf];
        let err = plan(code, None).unwrap_err();
        assert!(matches!(err, Refusal::PcRelative { .. }), "got {err:?}");
    }

    #[test]
    fn cbz_is_refused_because_it_is_a_relative_branch() {
        // cbz r0, +2
        let code: &[u8] = &[0x00, 0xb1, 0x00, 0xbf, 0x00, 0xbf, 0x00, 0xbf, 0x00, 0xbf];
        let err = plan(code, None).unwrap_err();
        assert!(matches!(err, Refusal::PcRelative { .. }), "got {err:?}");
    }

    #[test]
    fn an_unknown_32_bit_encoding_is_refused_not_guessed() {
        let code: &[u8] = &[0xff, 0xf3, 0x00, 0x00, 0x00, 0xbf, 0x00, 0xbf, 0x00, 0xbf];
        let err = plan(code, None).unwrap_err();
        assert!(
            matches!(err, Refusal::Unknown { .. } | Refusal::PcRelative { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn instruction_lengths_match_the_real_bytes() {
        assert_eq!(instruction_len(0xb570), 2); // push {r4-r6, lr}
        assert_eq!(instruction_len(0x4770), 2); // bx lr
        assert_eq!(instruction_len(0xf89d), 4); // ldrb.w
        assert_eq!(instruction_len(0xe92d), 4); // stmdb
        assert_eq!(instruction_len(0xed2d), 4); // vpush
        assert_eq!(instruction_len(0xf500), 4); // add.w
    }

    #[test]
    fn the_patch_jumps_to_the_handler_from_both_alignments() {
        for addr in [0x530350u32, 0x530352] {
            let hook = 0xdead_b000u32;
            let p = patch_bytes(addr, hook);
            assert_eq!(u16::from_le_bytes([p[0], p[1]]), 0xF8DF);
            let imm = u16::from_le_bytes([p[2], p[3]]) & 0x0FFF;
            let pc = (addr + 4) & !3;
            let literal = addr + 4;
            assert_eq!(pc + imm as u32, literal, "imm must reach the literal");
            assert_eq!(u32::from_le_bytes([p[4], p[5], p[6], p[7]]), hook);
            assert_eq!(imm, if addr % 4 == 0 { 0 } else { 2 });
        }
    }
}
