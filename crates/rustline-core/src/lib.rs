//! rustline-core: pure, front-end-agnostic status line rendering.
pub mod context;
pub mod segment;

pub use context::{Context, WindowCtx};
pub use segment::{Color, Segment, Style};
