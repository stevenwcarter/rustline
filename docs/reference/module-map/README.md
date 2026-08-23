# Module map

Per-file responsibilities for every crate. Start at the crate that owns the
code you are touching, then open the leaf file it points to.

| Crate | Read when |
|---|---|
| [`rustline-core/`](rustline-core/README.md) | The pure render core: `Context`, segments, powerline rendering, themes, the widget registry and the sixteen built-ins, config parsing and layout algebra. |
| [`rustline-abi.md`](rustline-abi.md) | Anything crossing the WASM boundary: `Segment`/`Style`/`Color`, `WireContext`, host-effect result types, `WidgetName`, `ABI_VERSION`. |
| [`rustline-wasm/`](rustline-wasm/README.md) | The WASM plugin host: capability gates, host functions, sandboxing, manifests, integrity, registration. |
| [`rustline-plugin-sdk.md`](rustline-plugin-sdk.md) | The guest-side SDK a plugin depends on: typed capability wrappers, wire re-exports, `export_plugin!`. |
| [`rustline-bin/`](rustline-bin/README.md) | The CLI front-end: platform reads, `Context` construction, the daemon, subcommands, the TUI editor, logging, bench. |
| [`example-plugins.md`](example-plugins.md) | The five bundled example plugins (`weather`, `counter`, `filewatch`, `httpget`, `cmdrun`) — worked examples per host capability. |
