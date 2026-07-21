pub mod cwd;
pub mod datetime;
pub mod hostname;
pub mod loadavg;
mod net;
pub mod pane_id;
pub mod windows;

pub use cwd::Cwd;
pub use datetime::DateTime;
pub use hostname::Hostname;
pub use loadavg::LoadAvg;
pub use pane_id::PaneId;
pub use windows::Windows;

use crate::Config;
use crate::widget::Registry;

impl Registry {
    /// Build a [`Registry`] pre-populated with all six built-in widgets,
    /// configuring the ones with options (`datetime`, `cwd`) from `cfg`.
    pub fn with_builtins(cfg: &Config) -> Registry {
        let mut registry = Registry::new();
        registry.register("pane_id", Box::new(|| Box::new(PaneId)));
        registry.register("hostname", Box::new(|| Box::new(Hostname)));
        registry.register("windows", Box::new(|| Box::new(Windows)));
        registry.register("loadavg", Box::new(|| Box::new(LoadAvg)));

        let format = cfg.widgets.datetime.format.clone();
        registry.register(
            "datetime",
            Box::new(move || {
                Box::new(DateTime {
                    format: format.clone(),
                })
            }),
        );

        let abbreviate_home = cfg.widgets.cwd.abbreviate_home;
        registry.register("cwd", Box::new(move || Box::new(Cwd { abbreviate_home })));

        registry
    }
}
