use crate::{Context, Segment};
use std::collections::HashMap;

/// Something that can render itself into one or more [`Segment`]s given the
/// current [`Context`].
pub trait Widget {
    fn render(&self, ctx: &Context) -> Vec<Segment>;
}

/// A widget constructor, boxed so the registry can hold a heterogeneous set
/// of them keyed by name.
type Factory = Box<dyn Fn() -> Box<dyn Widget> + Send + Sync>;

/// A name-to-factory table for widgets, populated at startup from built-in
/// and/or plugin widget constructors and consulted when resolving the
/// widget names listed in user config.
#[derive(Default)]
pub struct Registry {
    factories: HashMap<String, Factory>,
}

impl Registry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a widget constructor under `name`, overwriting any existing
    /// registration for that name.
    pub fn register(
        &mut self,
        name: &str,
        factory: Box<dyn Fn() -> Box<dyn Widget> + Send + Sync>,
    ) {
        self.factories.insert(name.to_string(), factory);
    }

    /// Build a single widget by name, if registered.
    pub fn build(&self, name: &str) -> Option<Box<dyn Widget>> {
        self.factories.get(name).map(|factory| factory())
    }

    /// Whether a widget is already registered under `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.factories.contains_key(name)
    }

    /// Build widgets for each name in order, skipping (and logging) any
    /// name that isn't registered.
    ///
    /// Unknown widget names in user config must never be fatal: a typo in a
    /// config file shouldn't take down the whole status line, so unknown
    /// names are just logged and dropped.
    pub fn resolve(&self, names: &[String]) -> Vec<Box<dyn Widget>> {
        names
            .iter()
            .filter_map(|name| {
                let widget = self.build(name);
                if widget.is_none() {
                    tracing::warn!(widget = %name, "unknown widget, skipping");
                }
                widget
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Segment};
    use chrono::{Local, TimeZone};

    struct Fixed(&'static str);
    impl Widget for Fixed {
        fn render(&self, _ctx: &Context) -> Vec<Segment> {
            vec![Segment::new(self.0)]
        }
    }

    fn ctx() -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/home/steve".into(),
            hostname: "h".into(),
            loadavg: None,
            now: Local
                .with_ymd_and_hms(2026, 7, 20, 17, 49, 0)
                .single()
                .unwrap(),
            window: None,
            interfaces: Vec::new(),
            battery: None,
            cpu: None,
            memory: None,
            os: String::new(),
            arch: String::new(),
            toggled: Default::default(),
        }
    }

    #[test]
    fn contains_reports_registration() {
        let mut r = Registry::new();
        assert!(!r.contains("a"));
        r.register("a", Box::new(|| Box::new(Fixed("A"))));
        assert!(r.contains("a"));
    }

    #[test]
    fn resolve_skips_unknown_and_keeps_order() {
        let mut r = Registry::new();
        r.register("a", Box::new(|| Box::new(Fixed("A"))));
        r.register("b", Box::new(|| Box::new(Fixed("B"))));
        let widgets = r.resolve(&["a".into(), "missing".into(), "b".into()]);
        let texts: Vec<String> = widgets
            .iter()
            .flat_map(|w| w.render(&ctx()))
            .map(|s| s.text)
            .collect();
        assert_eq!(texts, vec!["A".to_string(), "B".to_string()]);
    }
}
