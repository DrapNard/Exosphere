// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Optional development-time SM53 oracle using envytools `envydis gm107`.
//!
//! This module is never required by the runtime. It exists to accelerate reverse engineering and
//! coverage work: raw shader bytes can be disassembled by a locally installed envytools binary,
//! cached, compared with Exosphere's decoder, and then promoted into native semantic decoding.

use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[derive(Debug, Clone)]
pub struct OracleLine {
    pub raw: String,
    pub address: Option<u64>,
    pub mnemonic: Option<String>,
    pub operands: Option<String>,
}

pub fn available(program: &Path) -> bool {
    Command::new(program)
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

pub fn disassemble_gm107(program: &Path, bytes: &[u8]) -> Result<Vec<OracleLine>, String> {
    let mut child = Command::new(program)
        .args(["-m", "gm107"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .as_mut()
        .ok_or("envydis stdin unavailable")?
        .write_all(bytes)
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(parse_line)
        .collect())
}

fn parse_line(line: &str) -> OracleLine {
    let trimmed = line.trim();
    let (address, rest) = if let Some((left, right)) = trimmed.split_once(':') {
        (
            u64::from_str_radix(left.trim_start_matches("0x").trim(), 16).ok(),
            right.trim(),
        )
    } else {
        (None, trimmed)
    };
    let mut parts = rest
        .splitn(2, char::is_whitespace)
        .filter(|s| !s.is_empty());
    let mnemonic = parts.next().map(str::to_owned);
    let operands = parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    OracleLine {
        raw: line.to_owned(),
        address,
        mnemonic,
        operands,
    }
}
