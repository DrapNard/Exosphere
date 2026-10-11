// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! BLAKE3 content-addressed store primitives.

pub mod index;

use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct Cas {
    root: PathBuf,
}

impl Cas {
    /// Creates a CAS rooted at `root`. The directory is created lazily on first write.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Stores `bytes` and returns its BLAKE3 digest.
    pub fn put(&self, bytes: &[u8]) -> io::Result<[u8; 32]> {
        let hash = *blake3::hash(bytes).as_bytes();
        let hex = blake3::Hash::from_bytes(hash).to_hex().to_string();
        let path = self.object_path(&hex);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, bytes)?;
        }
        Ok(hash)
    }

    /// Returns the canonical on-disk path used for one hash.
    pub fn object_path(&self, hex: &str) -> PathBuf {
        let (a, b) = hex.split_at(2);
        self.root.join(a).join(b)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
