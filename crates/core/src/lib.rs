//! Core library of ed2k-vault: hashing, link building, scanning and output formats.

pub mod aich;
pub mod ed2k;
pub mod error;

/// Size of an ed2k part (the unit hashed with MD4).
pub const PART_SIZE: u64 = 9_728_000;

/// Size of an AICH block (the leaf unit hashed with SHA-1).
pub const AICH_BLOCK_SIZE: u64 = 184_320;
