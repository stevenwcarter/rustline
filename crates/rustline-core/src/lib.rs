//! rustline-core: pure, front-end-agnostic status line rendering.
pub mod ansi;
pub mod assemble;
pub mod atomic_write;
pub mod config;
pub mod context;
pub mod diag;
pub mod range_name;
pub mod render;
pub mod segment;
pub mod themes;
pub mod widget;
pub mod widgets;

pub use ansi::{StyledSpan, parse_markup, tmux_to_ansi};
pub use assemble::{assign_palette, render_named_region, render_window, render_windows};
pub use config::{
    ClickBinding, ClickBindings, ColorOverride, Config, Layout, LayoutChange, LayoutEditError,
    LogConfig, PluginConfig, PluginSource, Region, ThemeConfig, WidgetClick, WidgetPlacement,
    layout_disable, layout_enable, layout_move, layout_nudge, widget_placements,
};
pub use context::{
    Battery, BatteryState, Context, CpuUsage, DiskInfo, GitInfo, MediaInfo, MemInfo, NetIface,
    Throughput, WindowCtx,
};
pub use range_name::{NameError, RANGE_NAME_MAX_BYTES, RangeName};
pub use render::{Direction, Theme, render_region};
pub use segment::{Color, Segment, Style, ThemeColors};
pub use themes::{builtin_theme, builtin_theme_names};
pub use widget::{Registry, Widget, WidgetDescriptor, WidgetSource};
