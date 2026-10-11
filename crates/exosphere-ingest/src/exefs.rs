// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! ExeFS view. Switch ExeFS is a PFS0 filesystem.

use crate::{
    IngestError,
    pfs0::{self, Pfs0},
};
pub fn parse(bytes: &[u8]) -> Result<Pfs0, IngestError> {
    pfs0::parse(bytes)
}
