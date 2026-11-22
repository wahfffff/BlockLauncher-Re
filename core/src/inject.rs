//! Turning "the game library is mapped" into an address.
//!
//! `dlopen(path, RTLD_NOLOAD)` would be the obvious way to ask the linker for a
//! handle, and it returns null in the one case that matters: the game loads its
//! library into its own classloader namespace, this module lives in the default
//! namespace, and a lookup only searches the caller's namespace. Measured on
//! MCPE 0.14.3 / Android 16:
//!
//! ```text
//! /data/app/.../lib/arm/libminecraftpe.so is mapped but dlopen(RTLD_NOLOAD) returned null
//! ```
//!
//! A namespace cannot be crossed from inside it, so nothing here tries. The file is
//! mapped, its path is in `/proc/self/maps`, and the app can read its own native
//! library directory: symbols come from the file on disk, addresses from the
//! mapping. No handle required.
//!
//! # Why the lowest mapping is the one that counts
//!
//! The tempting arithmetic is "base = this segment's start − its file offset", and
//! it is wrong for every segment but the first. Measured, verbatim:
//!
//! ```text
//! e00db000-e07c5000 r-xp 00000000  ... /lib/arm/libminecraftpe.so
//! e07c6000-e0836000 r--p 006ea000  ... /lib/arm/libminecraftpe.so
//! ```
//!
//! `0xe07c6000 - 0x6ea000 = 0xe00dc000`, which is one page above the true base
//! `0xe00db000`. The loader aligns each segment down to a page, so a later
//! segment's start carries that padding. The first mapping has offset 0 and needs
//! no subtraction at all; taking the lowest address is what makes the answer exact
//! rather than approximately right.
//!
//! What makes a wrong base survivable is that it is *detectable*: the bytes at
//! `base + st_value` must equal the bytes at `st_value` in the file. A base off by a
//! page fails that comparison instead of silently patching something else.

use crate::elf::Symbol;

/// One mapping of the library, as reported by `/proc/self/maps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    /// Address the ELF was loaded at: the lowest mapping's start minus its offset.
    pub base: u64,
    /// The path the linker used, which is what the file must be read from.
    pub path: String,
}

/// Find where `target` is mapped, from the text of `/proc/self/maps`.
///
/// The match is on the file name, not on a substring of the whole path: a backup
/// called `libminecraftpe.so.bak` is a different file and must not be picked. Of
/// the segments that do match, the lowest address wins — see the module comment.
pub fn find(maps: &str, target: &str) -> Option<Mapping> {
    let mut best: Option<Mapping> = None;

    for line in maps.lines() {
        let mut fields = line.split_whitespace();
        let Some(range) = fields.next() else { continue };
        let Some(_perms) = fields.next() else { continue };
        let Some(offset) = fields.next() else { continue };
        let Some(_dev) = fields.next() else { continue };
        let Some(_inode) = fields.next() else { continue };
        // A path may contain spaces; everything left is the name.
        let path = fields.collect::<Vec<_>>().join(" ");

        if !path.starts_with('/') || path.rsplit('/').next() != Some(target) {
            continue;
        }

        let Some((start, _end)) = range.split_once('-') else { continue };
        let (Ok(start), Ok(offset)) =
            (u64::from_str_radix(start, 16), u64::from_str_radix(offset, 16))
        else {
            continue;
        };

        let base = start.saturating_sub(offset);
        if best.as_ref().is_none_or(|b| start < b.base) {
            best = Some(Mapping { base, path });
        }
    }

    best
}

/// The exported function called `name`, if the library defines one.
pub fn symbol<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
    symbols
        .iter()
        .find(|s| s.name == name && s.is_exported() && s.kind.is_function())
}

/// Where the file's address becomes the process's address.
pub fn at(base: u64, symbol: &Symbol) -> u64 {
    base.wrapping_add(symbol.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Copied out of a running MCPE 0.14.3 process, including the page gap between
    /// the first segment and the second.
    const REAL: &str = "\
e00db000-e07c5000 r-xp 00000000 fc:2b 770401                            /data/app/~~RcEV==/com.mojang.minecraftpe-x==/lib/arm/libminecraftpe.so
e07c6000-e0836000 r--p 006ea000 fc:2b 770401                            /data/app/~~RcEV==/com.mojang.minecraftpe-x==/lib/arm/libminecraftpe.so
e0836000-e0837000 rw-p 0075a000 fc:2b 770401                            /data/app/~~RcEV==/com.mojang.minecraftpe-x==/lib/arm/libminecraftpe.so
ec14a000-ec2c6000 r--p 00000000 fc:2b 158887                            /data/local/tmp/libblre.so
";

    #[test]
    fn the_load_address_comes_out_of_the_lowest_mapping() {
        let m = find(REAL, "libminecraftpe.so").expect("mapped");
        assert_eq!(m.base, 0xe00db000);
        assert!(m.path.ends_with("/lib/arm/libminecraftpe.so"));
    }

    #[test]
    fn a_later_segment_does_not_move_the_base() {
        // The trap: 0xe07c6000 - 0x6ea000 is 0xe00dc000, one page too high.
        assert_eq!(0xe07c6000u64 - 0x6ea000u64, 0xe00dc000);
        assert_eq!(find(REAL, "libminecraftpe.so").unwrap().base, 0xe00db000);
    }

    #[test]
    fn the_order_lines_arrive_in_does_not_matter() {
        let mut lines: Vec<&str> = REAL.lines().collect();
        lines.reverse();
        let reversed = lines.join("\n");
        assert_eq!(find(&reversed, "libminecraftpe.so").unwrap().base, 0xe00db000);
    }

    #[test]
    fn a_look_alike_file_name_is_not_the_target() {
        let maps = "\
e00db000-e07c5000 r-xp 00000000 fc:2b 1 /data/app/x/lib/arm/libminecraftpe.so.bak
e10db000-e17c5000 r-xp 00000000 fc:2b 2 /data/app/x/lib/arm/libminecraftpe.so
";
        assert_eq!(find(maps, "libminecraftpe.so").unwrap().base, 0xe10db000);
    }

    #[test]
    fn an_absent_library_is_none_not_a_guess() {
        assert!(find(REAL, "libminecraftpe").is_none());
        assert!(find("", "libminecraftpe.so").is_none());
    }

    #[test]
    fn anonymous_mappings_without_a_path_are_skipped() {
        let maps = "\
e0000000-e0100000 rw-p 00000000 00:00 0
e10db000-e17c5000 r-xp 00000000 fc:2b 2 /data/app/x/lib/arm/libminecraftpe.so
";
        assert_eq!(find(maps, "libminecraftpe.so").unwrap().base, 0xe10db000);
    }

    #[test]
    fn a_path_with_a_space_is_kept_whole() {
        let maps = "e10db000-e17c5000 r-xp 00000000 fc:2b 2 /data/app/my games/lib/arm/libminecraftpe.so\n";
        assert_eq!(
            find(maps, "libminecraftpe.so").unwrap().path,
            "/data/app/my games/lib/arm/libminecraftpe.so"
        );
    }
}
