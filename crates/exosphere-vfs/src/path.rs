// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Normalized guest paths.

use crate::types::{FsError, FsResult};

/// A mount-relative, normalized guest path.
///
/// `..`, NUL bytes and host-specific path prefixes are rejected before any backend sees a path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GuestPath(String);

impl GuestPath {
    pub fn root() -> Self {
        Self("/".into())
    }

    pub fn parse(input: &str) -> FsResult<Self> {
        if input.as_bytes().contains(&0) {
            return Err(FsError::InvalidPath);
        }
        let replaced = input.replace('\\', "/");
        let mut out = Vec::new();
        for part in replaced.split('/') {
            match part {
                "" | "." => {}
                ".." => return Err(FsError::PathEscapesMount),
                value if value.contains(':') => return Err(FsError::InvalidPath),
                value => out.push(value),
            }
        }
        Ok(Self(format!("/{}", out.join("/"))))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn join(&self, child: &str) -> FsResult<Self> {
        Self::parse(&format!("{}/{}", self.0.trim_end_matches('/'), child))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_escape() {
        assert_eq!(GuestPath::parse("a/../b"), Err(FsError::PathEscapesMount));
    }
    #[test]
    fn normalizes() {
        assert_eq!(GuestPath::parse("//a/./b/").unwrap().as_str(), "/a/b");
    }
}
