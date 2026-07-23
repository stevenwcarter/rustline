use crate::{Context, Segment};
use std::collections::HashMap;

/// Something that can render itself into one or more [`Segment`]s given the
/// current [`Context`].
pub trait Widget {
    fn render(&self, ctx: &Context) -> Vec<Segment>;

    /// The clickable status-line range name for this widget, if it opts into
    /// click-to-toggle. Default `None` (not clickable). A widget returns
    /// `Some(name)` only when it has an alternate view and `name` fits tmux's
    /// 15-byte `range=user|X` limit; the assemble layer wraps its cells in
    /// `#[range=user|<name>]…#[norange]` when so.
    fn range_name(&self) -> Option<&str> {
        None
    }
}

/// A widget constructor, boxed so the registry can hold a heterogeneous set
/// of them keyed by name.
type Factory = Box<dyn Fn() -> Box<dyn Widget> + Send + Sync>;

/// Where a registered widget came from: compiled into the binary, or
/// discovered from a WASM plugin at runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetSource {
    Builtin,
    Plugin,
}

/// A description of a registered widget, independent of building an
/// instance. This is the enabling abstraction for a future `widget` command
/// that can list/describe what's available without touching `Context`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetDescriptor {
    /// The registry name (the layout/config key, or a plugin's `.wasm` stem).
    pub name: String,
    /// A one-line, human-readable description of what the widget shows.
    pub summary: String,
    /// Whether the widget carries a `[widgets.<name>]` (or plugin `options`)
    /// config table, as opposed to always rendering the same way.
    pub configurable: bool,
    /// Whether this widget is compiled in or came from a discovered plugin.
    pub source: WidgetSource,
}

/// A name-to-factory table for widgets, populated at startup from built-in
/// and/or plugin widget constructors and consulted when resolving the
/// widget names listed in user config. Also keeps an ordered list of
/// [`WidgetDescriptor`]s so callers can enumerate/describe what's registered
/// without building an instance.
#[derive(Default)]
pub struct Registry {
    factories: HashMap<String, Factory>,
    descriptors: Vec<WidgetDescriptor>,
}

impl Registry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a widget constructor under `name`, overwriting any existing
    /// registration for that name. Records a minimal descriptor (`summary`
    /// equal to `name`, not configurable, [`WidgetSource::Builtin`]); use
    /// [`Registry::register_described`] to supply a fuller one.
    pub fn register(
        &mut self,
        name: &str,
        factory: Box<dyn Fn() -> Box<dyn Widget> + Send + Sync>,
    ) {
        self.register_described(
            WidgetDescriptor {
                name: name.to_string(),
                summary: name.to_string(),
                configurable: false,
                source: WidgetSource::Builtin,
            },
            factory,
        );
    }

    /// Register a widget constructor along with a full [`WidgetDescriptor`],
    /// overwriting any existing registration (factory and descriptor) for
    /// that name.
    pub fn register_described(
        &mut self,
        desc: WidgetDescriptor,
        factory: Box<dyn Fn() -> Box<dyn Widget> + Send + Sync>,
    ) {
        self.factories.insert(desc.name.clone(), factory);
        self.descriptors.retain(|d| d.name != desc.name);
        self.descriptors.push(desc);
    }

    /// Every registered widget's descriptor, in registration order.
    pub fn descriptors(&self) -> &[WidgetDescriptor] {
        &self.descriptors
    }

    /// Every registered widget's name, in registration order.
    pub fn available_names(&self) -> impl Iterator<Item = &str> {
        self.descriptors.iter().map(|d| d.name.as_str())
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
            now: Local
                .with_ymd_and_hms(2026, 7, 20, 17, 49, 0)
                .single()
                .unwrap(),
            ..Default::default()
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
    fn range_name_defaults_to_none() {
        assert_eq!(Fixed("x").range_name(), None);
    }

    #[test]
    fn descriptors_list_registrations_in_order() {
        let mut r = Registry::new();
        r.register("a", Box::new(|| Box::new(Fixed("A"))));
        r.register("b", Box::new(|| Box::new(Fixed("B"))));
        let names: Vec<&str> = r.descriptors().iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
        assert_eq!(r.available_names().collect::<Vec<_>>(), vec!["a", "b"]);
    }

    #[test]
    fn register_backcompat_minimal_descriptor() {
        let mut r = Registry::new();
        r.register("a", Box::new(|| Box::new(Fixed("A"))));
        let desc = &r.descriptors()[0];
        assert_eq!(desc.name, "a");
        assert_eq!(desc.summary, "a");
        assert!(!desc.configurable);
        assert_eq!(desc.source, WidgetSource::Builtin);
    }

    #[test]
    fn register_described_overwrites_prior_descriptor_for_same_name() {
        let mut r = Registry::new();
        r.register("a", Box::new(|| Box::new(Fixed("A"))));
        r.register_described(
            WidgetDescriptor {
                name: "a".to_string(),
                summary: "the real A".to_string(),
                configurable: true,
                source: WidgetSource::Plugin,
            },
            Box::new(|| Box::new(Fixed("A2"))),
        );
        assert_eq!(r.descriptors().len(), 1);
        let desc = &r.descriptors()[0];
        assert_eq!(desc.summary, "the real A");
        assert!(desc.configurable);
        assert_eq!(desc.source, WidgetSource::Plugin);
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
