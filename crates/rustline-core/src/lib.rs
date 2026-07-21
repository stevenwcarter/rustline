//! rustline-core: pure, front-end-agnostic status line rendering.
pub mod ansi;
pub mod assemble;
pub mod config;
pub mod context;
pub mod render;
pub mod segment;
pub mod widget;
pub mod widgets;

pub use ansi::tmux_to_ansi;
pub use assemble::{assign_palette, render_named_region, render_window};
pub use config::{Config, LogConfig, PluginConfig};
pub use context::{Battery, BatteryState, Context, CpuUsage, MemInfo, NetIface, WindowCtx};
pub use render::{Direction, Theme, render_region};
pub use segment::{Color, Segment, Style};
pub use widget::{Registry, Widget};
