// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Save-data identity and transaction metadata.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SaveDataType {
    Account,
    Device,
    Cache,
    System,
    Temporary,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SaveDataAttribute {
    pub program_id: u64,
    pub user_id: [u8; 16],
    pub save_type: SaveDataType,
    pub index: u16,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    Clean,
    Dirty,
    Committing,
}
#[derive(Debug, Clone)]
pub struct SaveMetadata {
    pub attribute: SaveDataAttribute,
    pub game_version: u32,
    pub transaction: TransactionState,
}
