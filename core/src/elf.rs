//! Just enough of the ELF format to answer one question: does this shared object
//! export the symbol a hook would have to replace, and at what address?
//!
//! The library this reads is 7.7 MB of C++ compiled by whatever NDK Minecraft PE
//! was built with in 2015, and the launcher opens it on a phone. So the parser
//! trusts nothing it is told: every offset is bounds-checked against the buffer,
//! a section table that points outside the file is a refusal rather than a
//! slice, and an unsupported class is named instead of silently decoded as the
//! wrong width.
//!
//! Deliberately absent: relocation processing, DWARF, and versioned symbol
//! lookup. None of them changes whether a symbol is exported, which is the only
//! thing this module is for.

use crate::error::{Error, Result};

const EI_CLASS: usize = 4;
const EI_DATA: usize = 5;
const SHT_DYNSYM: u32 = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Elf32,
    Elf64,
}

/// ELF symbol types, restricted to the three that matter here. Anything else
/// keeps its raw number so a caller can report it without this enum growing a
/// variant per STT_* constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolType(pub u8);

impl SymbolType {
    pub const NOTYPE: SymbolType = SymbolType(0);
    pub const OBJECT: SymbolType = SymbolType(1);
    pub const FUNC: SymbolType = SymbolType(2);

    pub fn is_function(self) -> bool {
        self == SymbolType::FUNC
    }

    pub fn is_object(self) -> bool {
        self == SymbolType::OBJECT
    }

    pub fn name(self) -> &'static str {
        match self.0 {
            0 => "notype",
            1 => "object",
            2 => "function",
            3 => "section",
            4 => "file",
            5 => "common",
            6 => "tls",
            _ => "other",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub value: u64,
    pub size: u64,
    pub kind: SymbolType,
    /// `st_shndx`: 0 is SHN_UNDEF, i.e. imported rather than exported.
    pub section: u16,
}

impl Symbol {
    /// Exported means "defined here". An undefined entry is the library asking
    /// its loader for the symbol, and hooking that would patch the wrong thing.
    pub fn is_exported(&self) -> bool {
        self.section != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub class: Class,
    /// `e_machine`. 40 is EM_ARM, 183 is EM_AARCH64.
    pub machine: u16,
    pub symbol_count: usize,
}

impl Header {
    pub fn arch_name(&self) -> &'static str {
        match (self.class, self.machine) {
            (Class::Elf32, 40) => "armeabi-v7a",
            (Class::Elf64, 183) => "arm64-v8a",
            (Class::Elf32, 3) => "x86",
            (Class::Elf64, 62) => "x86_64",
            _ => "unknown",
        }
    }
}

fn need<'a>(b: &'a [u8], at: usize, len: usize, what: &str) -> Result<&'a [u8]> {
    b.get(at..at + len)
        .ok_or_else(|| Error::BadElf(format!("{what} at {at} runs past {} bytes", b.len())))
}

fn u16le(b: &[u8], at: usize) -> Result<u16> {
    let s = need(b, at, 2, "a u16")?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

fn u32le(b: &[u8], at: usize) -> Result<u32> {
    let s = need(b, at, 4, "a u32")?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u64le(b: &[u8], at: usize) -> Result<u64> {
    let s = need(b, at, 8, "a u64")?;
    Ok(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

struct Section {
    kind: u32,
    offset: usize,
    size: usize,
    link: u32,
    entsize: usize,
}

/// Parse the header and the dynamic symbol table.
///
/// Returns the header separately from the symbols because the caller usually
/// wants to report the architecture even when the symbol table is unusable.
pub fn parse(buf: &[u8]) -> Result<(Header, Vec<Symbol>)> {
    let ident = need(buf, 0, 16, "the ELF identification")?;
    if &ident[0..4] != b"\x7fELF" {
        return Err(Error::BadElf("not an ELF object".into()));
    }
    let class = match ident[EI_CLASS] {
        1 => Class::Elf32,
        2 => Class::Elf64,
        other => {
            return Err(Error::BadElf(format!(
                "ELF class {other} is neither 32-bit (1) nor 64-bit (2)"
            )))
        }
    };
    if ident[EI_DATA] != 1 {
        return Err(Error::BadElf(
            "only little-endian objects are supported".into(),
        ));
    }

    let (shoff, shentsize, shnum, machine) = match class {
        Class::Elf32 => (
            u32le(buf, 0x20)? as usize,
            u16le(buf, 0x2e)? as usize,
            u16le(buf, 0x30)? as usize,
            u16le(buf, 0x12)?,
        ),
        Class::Elf64 => (
            u64le(buf, 0x28)? as usize,
            u16le(buf, 0x3a)? as usize,
            u16le(buf, 0x3c)? as usize,
            u16le(buf, 0x12)?,
        ),
    };

    let sections = read_sections(buf, class, shoff, shentsize, shnum)?;
    let dynsym = sections
        .iter()
        .find(|s| s.kind == SHT_DYNSYM)
        .ok_or_else(|| Error::BadElf("no section of type SHT_DYNSYM".into()))?;

    let symbols = read_symbols(buf, class, dynsym, &sections)?;

    let header = Header {
        class,
        machine,
        symbol_count: symbols.len(),
    };
    Ok((header, symbols))
}

fn read_sections(
    buf: &[u8],
    class: Class,
    shoff: usize,
    shentsize: usize,
    shnum: usize,
) -> Result<Vec<Section>> {
    let expected = match class {
        Class::Elf32 => 40,
        Class::Elf64 => 64,
    };
    if shentsize < expected {
        return Err(Error::BadElf(format!(
            "section header entries are {shentsize} bytes, {expected} is the minimum"
        )));
    }
    if shnum == 0 || shoff == 0 {
        return Err(Error::BadElf(
            "no section table (stripped to the point of being unreadable)".into(),
        ));
    }

    // Check the whole table fits before reading any entry. A count that does not
    // fit is a corrupt file, and reading entry by entry would just report the
    // same thing later with a less useful message.
    let end = shoff
        .checked_add(shentsize.checked_mul(shnum).ok_or_else(|| {
            Error::BadElf("section count times entry size overflows".into())
        })?)
        .ok_or_else(|| Error::BadElf("section table offset overflows".into()))?;
    if end > buf.len() {
        return Err(Error::BadElf(format!(
            "section table ends at {end}, the file is {} bytes",
            buf.len()
        )));
    }

    let mut out = Vec::with_capacity(shnum);
    for i in 0..shnum {
        let at = shoff + i * shentsize;
        let (kind, offset, size, link, entsize) = match class {
            Class::Elf32 => (
                u32le(buf, at + 4)?,
                u32le(buf, at + 16)? as usize,
                u32le(buf, at + 20)? as usize,
                u32le(buf, at + 24)?,
                u32le(buf, at + 36)? as usize,
            ),
            Class::Elf64 => (
                u32le(buf, at + 4)?,
                u64le(buf, at + 24)? as usize,
                u64le(buf, at + 32)? as usize,
                u32le(buf, at + 40)?,
                u64le(buf, at + 56)? as usize,
            ),
        };
        out.push(Section {
            kind,
            offset,
            size,
            link,
            entsize,
        });
    }
    Ok(out)
}

fn read_symbols(
    buf: &[u8],
    class: Class,
    dynsym: &Section,
    sections: &[Section],
) -> Result<Vec<Symbol>> {
    let expected = match class {
        Class::Elf32 => 16,
        Class::Elf64 => 24,
    };
    let entsize = if dynsym.entsize == 0 {
        expected
    } else {
        dynsym.entsize
    };
    if entsize < expected {
        return Err(Error::BadElf(format!(
            "symbol entries are {entsize} bytes, {expected} is the minimum"
        )));
    }

    let table = buf
        .get(dynsym.offset..dynsym.offset + dynsym.size)
        .ok_or_else(|| Error::BadElf("the dynamic symbol table runs past the file".into()))?;

    // The string table is the section the symbol table links to.
    let strtab = sections
        .get(dynsym.link as usize)
        .ok_or_else(|| Error::BadElf(format!("dynsym links to section {}", dynsym.link)))?;
    let strings = buf
        .get(strtab.offset..strtab.offset + strtab.size)
        .ok_or_else(|| Error::BadElf("the string table runs past the file".into()))?;

    let mut out = Vec::with_capacity(table.len() / entsize);
    for raw in table.chunks_exact(entsize) {
        let (name_at, info, other_shndx, value, size) = match class {
            Class::Elf32 => (
                u32le(raw, 0)? as usize,
                raw[12],
                u16le(raw, 14)?,
                u32le(raw, 4)? as u64,
                u32le(raw, 8)? as u64,
            ),
            Class::Elf64 => (
                u32le(raw, 0)? as usize,
                raw[4],
                u16le(raw, 6)?,
                u64le(raw, 8)?,
                u64le(raw, 16)?,
            ),
        };

        // Index 0 is the reserved null entry; a symbol with no name is not one
        // a hook could refer to, so it is dropped rather than carried around.
        if name_at == 0 {
            continue;
        }
        let tail = strings
            .get(name_at..)
            .ok_or_else(|| Error::BadElf(format!("symbol name offset {name_at} is past the string table")))?;
        let len = tail.iter().position(|&c| c == 0).ok_or_else(|| {
            Error::BadElf(format!("symbol name at {name_at} is not terminated"))
        })?;
        let name = std::str::from_utf8(&tail[..len])
            .map_err(|_| Error::BadElf(format!("symbol name at {name_at} is not UTF-8")))?
            .to_string();

        out.push(Symbol {
            name,
            value,
            size,
            kind: SymbolType(info & 0xf),
            // The st_shndx field is u16 both ways; the two-byte read above is
            // the value itself, not a packed pair.
            section: other_shndx,
        });
    }
    Ok(out)
}
