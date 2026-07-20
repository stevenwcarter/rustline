//! rustline-core: pure, front-end-agnostic status line rendering.
pub mod context;
pub mod render;
pub mod segment;
pub mod widget;
pub mod widgets;

pub use context::{Context, WindowCtx};
pub use render::{Direction, Theme, render_region};
pub use segment::{Color, Segment, Style};
pub use widget::{Registry, Widget};
