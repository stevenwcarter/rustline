//! rustline-core: pure, front-end-agnostic status line rendering.
pub mod ansi;
pub mod assemble;
pub mod config;
pub mod context;
pub mod render;
pub mod segment;
pub mod themes;
pub mod widget;
pub mod widgets;

pub use ansi::tmux_to_ansi;
pub use assemble::{assign_palette, render_named_region, render_window};
pub use config::{
    ClickBinding, ClickBindings, ColorOverride, Config, LogConfig, PluginConfig, PluginSource,
    ThemeConfig, WidgetClick,
};
pub use context::{
    Battery, BatteryState, Context, CpuUsage, DiskInfo, GitInfo, MediaInfo, MemInfo, NetIface,
    Throughput, WindowCtx,
};
pub use render::{Direction, RANGE_NAME_MAX_BYTES, Theme, render_region};
pub use segment::{Color, Segment, Style, ThemeColors};
pub use themes::{builtin_theme, builtin_theme_names};
pub use widget::{Registry, Widget, WidgetDescriptor, WidgetSource};
