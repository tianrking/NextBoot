//! Host-testable virtual disk image metadata helpers.

#![no_std]

extern crate alloc;

pub mod vdi;
pub mod vhdx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSpanSource {
    Image { file_offset: u64 },
    Parent,
    Zero,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageSpan {
    pub virtual_offset: u64,
    pub byte_count: u64,
    pub source: ImageSpanSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagePlanError {
    Invalid,
    Unsupported,
}
