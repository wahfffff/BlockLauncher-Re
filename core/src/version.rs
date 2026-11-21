//! Minecraft version strings, and the ordering the launcher needs from them.
//!
//! These are not semver. The versions this launcher deals with are
//! `0.1.1`, `0.14.0`, `1.1.5.1`, `1.16.201.01`, and occasionally a suffix like
//! `0.15.0b1`. Comparing them as strings makes `0.9.0` sort above `0.14.0`, so
//! every comparison in this crate goes through `GameVersion`.

use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameVersion {
    raw: String,
    parts: Vec<u32>,
    /// Trailing non-numeric marker such as the `b1` in `0.15.0b1`. Builds with
    /// such a suffix are pre-releases of the version they name.
    suffix: Option<String>,
}

impl GameVersion {
    pub fn parse(s: &str) -> Option<Self> {
        let t = s.trim();
        if t.is_empty() || t.len() > 32 {
            return None;
        }
        let mut parts = Vec::new();
        let mut suffix: Option<String> = None;

        for (i, seg) in t.split('.').enumerate() {
            if i >= 8 {
                return None;
            }
            let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                return None;
            }
            // Guard against "00000000000000000001" overflowing the parse.
            if digits.len() > 9 {
                return None;
            }
            parts.push(digits.parse::<u32>().ok()?);
            let rest = &seg[digits.len()..];
            if !rest.is_empty() {
                if suffix.is_some() {
                    return None;
                }
                suffix = Some(rest.to_ascii_lowercase());
            }
        }
        if parts.is_empty() {
            return None;
        }
        Some(GameVersion {
            raw: t.to_string(),
            parts,
            suffix,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    fn at(&self, i: usize) -> u32 {
        self.parts.get(i).copied().unwrap_or(0)
    }

    /// Ordering by number first, then by suffix: a build with a suffix sorts
    /// before the plain release of the same number.
    pub fn compare(&self, other: &Self) -> Ordering {
        let n = self.parts.len().max(other.parts.len());
        for i in 0..n {
            match self.at(i).cmp(&other.at(i)) {
                Ordering::Equal => {}
                ord => return ord,
            }
        }
        match (&self.suffix, &other.suffix) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => a.cmp(b),
        }
    }
}

impl std::fmt::Display for GameVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

/// Inclusive range used by a native mod's compatibility declaration. Both ends
/// are optional: "any 0.x" is `min=0.0.0, max=0.99.99`, "any version" leaves
/// both `None`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GameRange {
    pub min: Option<GameVersion>,
    pub max: Option<GameVersion>,
}

impl GameRange {
    pub fn contains(&self, v: &GameVersion) -> bool {
        if let Some(min) = &self.min {
            if v.compare(min) == Ordering::Less {
                return false;
            }
        }
        if let Some(max) = &self.max {
            if v.compare(max) == Ordering::Greater {
                return false;
            }
        }
        true
    }

    pub fn is_unbounded(&self) -> bool {
        self.min.is_none() && self.max.is_none()
    }
}
