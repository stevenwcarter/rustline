//! Re-export of the segment/style/color types, which now live in the
//! `rustline-abi` crate (the WASM plugin ABI). Kept as a module so existing
//! `rustline_core::segment::…` paths continue to resolve.
pub use rustline_abi::{Color, Segment, Style, ThemeColors};
