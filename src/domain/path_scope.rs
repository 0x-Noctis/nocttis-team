//! Scope path yang dinormalisasi untuk file lease (M4-002).
//!
//! Setiap pola `allowed_paths` direduksi menjadi salah satu dari dua bentuk:
//! - `Exact(path)`   — satu file/direktori literal tanpa wildcard;
//! - `Subtree(dir)`  — semua yang ada di bawah `dir` (`dir` kosong = seluruh repository).
//!
//! Pola yang mengandung wildcard dipetakan ke `Subtree` dari awalan literalnya (direktori sebelum
//! komponen pertama yang berisi `* ? [ ] { }`). Itu SUPERSET dari file yang sebenarnya cocok, jadi deteksi
//! overlap bersifat konservatif: bisa menolak dua scope yang sebenarnya tidak beririsan (mis. `src/*.rs`
//! dan `src/*.ts`), tetapi tidak pernah meloloskan scope yang beririsan. Perbandingan tidak peka huruf
//! besar/kecil karena repository bisa dipakai di filesystem yang case-insensitive.

use std::fmt;

const GLOB_CHARS: [char; 6] = ['*', '?', '[', ']', '{', '}'];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeError {
    Empty,
    Absolute,
    Traversal,
    ControlCharacter,
}

impl fmt::Display for ScopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "path scope must not be empty",
            Self::Absolute => "path scope must be relative",
            Self::Traversal => "path scope must not contain '..'",
            Self::ControlCharacter => "path scope must not contain control characters",
        })
    }
}

impl std::error::Error for ScopeError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathScope {
    Exact(String),
    /// Awalan direktori; string kosong berarti seluruh repository.
    Subtree(String),
}

impl PathScope {
    pub fn parse(pattern: &str) -> Result<Self, ScopeError> {
        if pattern.trim().is_empty() {
            return Err(ScopeError::Empty);
        }
        if pattern.chars().any(char::is_control) {
            return Err(ScopeError::ControlCharacter);
        }
        let unified = pattern.trim().replace('\\', "/");
        let bytes = unified.as_bytes();
        // Absolut: "/x", "//server", atau drive Windows "C:/x".
        if unified.starts_with('/') || bytes.get(1) == Some(&b':') {
            return Err(ScopeError::Absolute);
        }
        let trailing_slash = unified.ends_with('/');
        let mut literal: Vec<String> = Vec::new();
        let mut wildcard = false;
        for component in unified.split('/') {
            match component {
                "" | "." => {}
                ".." => return Err(ScopeError::Traversal),
                part if !wildcard && !part.contains(GLOB_CHARS) => {
                    literal.push(part.to_lowercase())
                }
                // Komponen pertama yang berwildcard: sisanya diabaikan (superset).
                _ => wildcard = true,
            }
        }
        let prefix = literal.join("/");
        // "dir/" atau pola berwildcard berarti seluruh isi direktori.
        // "." (atau pola kosong setelah normalisasi) berarti seluruh repository, bukan satu path literal.
        Ok(if wildcard || trailing_slash || prefix.is_empty() {
            Self::Subtree(prefix)
        } else {
            Self::Exact(prefix)
        })
    }

    /// Bentuk kanonik yang disimpan di database; `src/*`, `src/**`, dan `src/` semuanya menjadi `src/**`.
    pub fn pattern(&self) -> String {
        match self {
            Self::Exact(path) if path.is_empty() => "**".to_owned(),
            Self::Exact(path) => path.clone(),
            Self::Subtree(prefix) if prefix.is_empty() => "**".to_owned(),
            Self::Subtree(prefix) => format!("{prefix}/**"),
        }
    }

    /// True bila ada kemungkinan satu file berada di kedua scope.
    pub fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exact(left), Self::Exact(right)) => left == right,
            (Self::Exact(path), Self::Subtree(prefix))
            | (Self::Subtree(prefix), Self::Exact(path)) => within(path, prefix),
            (Self::Subtree(left), Self::Subtree(right)) => {
                within(left, right) || within(right, left)
            }
        }
    }
}

/// `path` sama dengan `prefix` atau berada di bawahnya pada batas komponen ("src2" tidak di bawah "src").
fn within(path: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(pattern: &str) -> PathScope {
        PathScope::parse(pattern).unwrap()
    }

    #[test]
    fn normalizes_to_canonical_patterns() {
        for (input, canonical) in [
            ("src/a.rs", "src/a.rs"),
            ("./src//a/./b.rs", "src/a/b.rs"),
            ("src\\a\\b.rs", "src/a/b.rs"),
            ("SRC/A.rs", "src/a.rs"),
            ("src/", "src/**"),
            ("src/*", "src/**"),
            ("src/**", "src/**"),
            ("src/**/", "src/**"),
            ("**", "**"),
            ("**/*.rs", "**"),
            ("*.md", "**"),
            ("src/*/mod.rs", "src/**"),
            ("src/a*.rs", "src/**"),
            ("src/[ab].rs", "src/**"),
            ("src/{a,b}.rs", "src/**"),
            ("docs/guide/?.md", "docs/guide/**"),
            (".", "**"),
        ] {
            assert_eq!(scope(input).pattern(), canonical, "{input}");
        }
    }

    #[test]
    fn rejects_unsafe_or_empty_patterns() {
        for (input, error) in [
            ("", ScopeError::Empty),
            ("   ", ScopeError::Empty),
            ("../x", ScopeError::Traversal),
            ("a/../b", ScopeError::Traversal),
            ("src/..", ScopeError::Traversal),
            ("/etc/passwd", ScopeError::Absolute),
            ("//server/share", ScopeError::Absolute),
            ("C:\\work\\x", ScopeError::Absolute),
            ("src/a\u{0}.rs", ScopeError::ControlCharacter),
            ("src/a\n.rs", ScopeError::ControlCharacter),
        ] {
            assert_eq!(PathScope::parse(input), Err(error), "{input:?}");
        }
    }

    #[test]
    fn overlap_is_conservative_and_symmetric() {
        // (kiri, kanan, beririsan?)
        for (left, right, expected) in [
            ("src/a.rs", "src/a.rs", true),
            ("src/a.rs", "SRC/A.RS", true),
            ("src/a.rs", "src/b.rs", false),
            ("src/a.rs", "src/**", true),
            ("src/a.rs", "src/", true),
            ("src/a.rs", "src2/**", false),
            ("src", "src/**", true),
            ("src2", "src/**", false),
            ("src/**", "src/api/**", true),
            ("src/api/**", "src/web/**", false),
            ("src/api/x.rs", "src/*/y.rs", true),
            ("**", "anything/at/all.rs", true),
            ("**/*.rs", "docs/x.md", true),
            ("src/*.rs", "src/*.ts", true),
            ("src/a.rs/x", "src/a.rs", false),
            ("src/a.rs/**", "src/a.rs", true),
            ("docs/**", "tests/**", false),
        ] {
            let (a, b) = (scope(left), scope(right));
            assert_eq!(a.overlaps(&b), expected, "{left} vs {right}");
            assert_eq!(b.overlaps(&a), expected, "{right} vs {left}");
        }
    }
}
