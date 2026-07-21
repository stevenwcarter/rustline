//! The rustline WASM plugin host: an Extism runtime with capability-gated
//! host functions (network + filesystem), plus discovery/registration of
//! plugins as `rustline_core::Widget`s. All capability checks happen here —
//! guests have zero ambient authority.

pub mod abi;
pub mod allow;
pub mod capability;
pub mod fetch;
pub mod paths;
pub mod perform;
pub mod state;
