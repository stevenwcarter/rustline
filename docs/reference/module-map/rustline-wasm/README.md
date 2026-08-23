# Module map — `rustline-wasm`

The WASM plugin host: an Extism (wasmtime) runtime with nine host functions —
eight capability-gated plus the capability-free guest logger `rl_log` —
per-plugin allowlists, a sandboxed/quota-bounded state dir, and discovery of
`*.wasm` files into `Widget` registrations. Zero ambient authority.

Anything here is governed by WASM invariants N1–N4 — see
[`../../invariants.md`](../../invariants.md) before changing a gate.

| File | Read when |
|---|---|
| [`capabilities.md`](capabilities.md) | Allowlists, path/argv sandboxing, quotas, TTL cache primitives, the `Fetcher`/`Runner` seams (`allow.rs`, `argv.rs`, `state.rs`, `paths.rs`, `abi.rs`, `cache.rs`, `capability.rs`, `fetch.rs`, `run.rs`). |
| [`host.md`](host.md) | Adding or changing a host function, plugin instantiation, manifests, denial recording, checksum verification, discovery/registration (`perform.rs`, `host.rs`, `manifest.rs`, `denials.rs`, `integrity.rs`, `lib.rs`). |
