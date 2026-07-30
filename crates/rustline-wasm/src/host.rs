//! Extism instantiation: bind the capability-gated host functions to each
//! plugin instance's `CapabilityCtx`, and wrap the instance as a `Widget` that
//! degrades to empty segments on any error.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LockResult, Mutex, MutexGuard};
use std::time::Duration;

use extism::{Manifest, PTR, PluginBuilder, UserData, Wasm, host_fn};
use rustline_abi::ABI_VERSION;
use rustline_core::{Context, RangeName, Segment, Widget, WidgetName};

use crate::abi::{CachedExecResult, ExecResult, RenderInput};
use crate::capability::CapabilityCtx;
use crate::fetch::UreqFetcher;
use crate::paths::wasmtime_cache_config_path;
use crate::perform::{
    perform_exec, perform_exec_cached, perform_file_read, perform_file_write, perform_http_get,
    perform_http_get_cached, perform_log, perform_state_read, perform_state_write,
};
use crate::run::ProcessRunner;

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string())
}

host_fn!(rl_http_get(user_data: CapabilityCtx; url: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_http_get(&ctx, &url, &UreqFetcher)))
});

host_fn!(rl_http_get_cached(user_data: CapabilityCtx; url: String, ttl_secs: String, now: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    let ttl: i64 = ttl_secs.parse().unwrap_or(0);
    Ok(json(&perform_http_get_cached(&ctx, &url, ttl, &now, &UreqFetcher)))
});

host_fn!(rl_state_read(user_data: CapabilityCtx; relpath: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_state_read(&ctx, &relpath)))
});

host_fn!(rl_state_write(user_data: CapabilityCtx; relpath: String, contents: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_state_write(&ctx, &relpath, &contents)))
});

host_fn!(rl_file_read(user_data: CapabilityCtx; path: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_file_read(&ctx, &path)))
});

host_fn!(rl_file_write(user_data: CapabilityCtx; path: String, contents: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    Ok(json(&perform_file_write(&ctx, &path, &contents)))
});

/// Decode the JSON array a guest passes as its `args`. Extism host functions
/// carry scalars and strings, so a vector crosses the boundary encoded — the
/// same "encode it, parse host-side" shape as `rl_http_get_cached`'s
/// `ttl_secs: String`. A malformed value is an error, never a panic and never
/// a spawn.
fn decode_args(json: &str) -> Result<Vec<String>, String> {
    serde_json::from_str::<Vec<String>>(json).map_err(|e| format!("invalid args: {e}"))
}

host_fn!(rl_exec(user_data: CapabilityCtx; program: String, args_json: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    let result = match decode_args(&args_json) {
        Ok(args) => perform_exec(&ctx, &program, &args, &ProcessRunner),
        // Malformed args never reach the gate or a spawn.
        Err(error) => ExecResult { ok: false, status: -1, error, ..Default::default() },
    };
    Ok(json(&result))
});

host_fn!(rl_exec_cached(user_data: CapabilityCtx; program: String, args_json: String, ttl_secs: String, now: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    let ttl: i64 = ttl_secs.parse().unwrap_or(0);
    let result = match decode_args(&args_json) {
        Ok(args) => perform_exec_cached(&ctx, &program, &args, ttl, &now, &ProcessRunner),
        Err(error) => CachedExecResult { ok: false, status: -1, error, ..Default::default() },
    };
    Ok(json(&result))
});

// `rl_log` is the one intentional capability-free host function (invariant
// N1): it only writes to the host's `tracing` subscriber, so — unlike the
// eight wrappers above — there is no allowlist check here. `user_data` is
// still used, but only to read this instance's plugin name for the log
// fields, not to gate anything.
host_fn!(rl_log(user_data: CapabilityCtx; level: String, msg: String) -> String {
    let ctx = user_data.get()?;
    let ctx = ctx.lock().unwrap();
    perform_log(&ctx, &level, &msg);
    Ok(String::new())
});

/// Whether [`build_plugin_with_cache`] points wasmtime at its on-disk
/// compilation cache. Production always uses [`CompileCache::Enabled`]; the
/// bench's build-timing A/B pass uses [`CompileCache::Disabled`] to isolate the
/// Cranelift compile cost from the warm-cache deserialize path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompileCache {
    /// Point wasmtime at the on-disk compile cache (the production default).
    Enabled,
    /// Force a full Cranelift compile every build (cache turned off).
    Disabled,
}

/// Build an Extism plugin from wasm bytes with wasi off, fuel + timeout +
/// memory caps, and the nine host functions (eight capability-gated, plus the
/// capability-free `rl_log`) bound to this instance's `CapabilityCtx`. Uses the
/// on-disk compile cache (see [`build_plugin_with_cache`]).
pub fn build_plugin(wasm: &[u8], ctx: CapabilityCtx) -> Result<extism::Plugin, extism::Error> {
    build_plugin_with_cache(wasm, ctx, CompileCache::Enabled)
}

/// [`build_plugin`] with explicit control over the wasmtime compile cache.
///
/// With [`CompileCache::Enabled`], wasmtime is pointed at an on-disk cache under
/// the state root so a later cold spawn deserializes an unchanged plugin's
/// precompiled artifact instead of re-running Cranelift. Cache setup is
/// best-effort: if `wasmtime_cache_config_path()` can't produce a config, the
/// build simply proceeds without a cache — never fatal (invariant N2: a plugin
/// never breaks the bar). Guest authority is untouched either way (N1–N4).
pub fn build_plugin_with_cache(
    wasm: &[u8],
    ctx: CapabilityCtx,
    cache: CompileCache,
) -> Result<extism::Plugin, extism::Error> {
    let ud = UserData::new(ctx);
    let manifest = Manifest::new([Wasm::data(wasm.to_vec())])
        .with_timeout(Duration::from_secs(10))
        .with_memory_max(256); // 256 pages ≈ 16 MB
    let mut builder = PluginBuilder::new(manifest).with_wasi(false);
    builder = match cache {
        CompileCache::Enabled => match wasmtime_cache_config_path() {
            Some(cfg) => builder.with_cache_config(cfg),
            None => builder,
        },
        CompileCache::Disabled => builder.with_cache_disabled(),
    };
    builder
        .with_fuel_limit(500_000_000)
        .with_function("rl_http_get", [PTR], [PTR], ud.clone(), rl_http_get)
        .with_function(
            "rl_http_get_cached",
            [PTR, PTR, PTR],
            [PTR],
            ud.clone(),
            rl_http_get_cached,
        )
        .with_function("rl_state_read", [PTR], [PTR], ud.clone(), rl_state_read)
        .with_function(
            "rl_state_write",
            [PTR, PTR],
            [PTR],
            ud.clone(),
            rl_state_write,
        )
        .with_function("rl_file_read", [PTR], [PTR], ud.clone(), rl_file_read)
        .with_function(
            "rl_file_write",
            [PTR, PTR],
            [PTR],
            ud.clone(),
            rl_file_write,
        )
        .with_function("rl_exec", [PTR, PTR], [PTR], ud.clone(), rl_exec)
        .with_function(
            "rl_exec_cached",
            [PTR, PTR, PTR, PTR],
            [PTR],
            ud.clone(),
            rl_exec_cached,
        )
        .with_function("rl_log", [PTR, PTR], [PTR], ud, rl_log)
        .build()
}

/// A discovered WASM plugin, rendered as a widget. Cheap to clone (shares the
/// instance behind an `Arc<Mutex<…>>`); any error/timeout/malformed output
/// degrades to empty segments so a plugin never breaks the bar.
#[derive(Clone)]
pub struct WasmWidget {
    plugin: Arc<Mutex<extism::Plugin>>,
    options: Arc<serde_json::Value>,
    name: WidgetName,
    /// One-shot latch for the poisoned-mutex warning. The daemon renders this
    /// widget every tick, so without it a single earlier panic would log once
    /// per refresh forever.
    poison_reported: Arc<AtomicBool>,
    /// One-shot latch for the malformed-output warning, for the same reason.
    /// A guest whose wire shape disagrees with this host returns malformed
    /// output on *every* tick, not once — so an unlatched warn here would be
    /// ~86k lines/day at the default 1 s interval, rotating away the very log
    /// this warning exists to appear in.
    decode_reported: Arc<AtomicBool>,
}

impl WasmWidget {
    pub fn new(plugin: extism::Plugin, options: serde_json::Value, name: &str) -> Self {
        Self {
            plugin: Arc::new(Mutex::new(plugin)),
            options: Arc::new(options),
            name: WidgetName::from(name),
            poison_reported: Arc::new(AtomicBool::new(false)),
            decode_reported: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Widget for WasmWidget {
    fn render(&self, ctx: &Context) -> Vec<Segment> {
        let input = RenderInput {
            context: ctx,
            config: &self.options,
            abi_version: ABI_VERSION,
        };
        let payload = match serde_json::to_string(&input) {
            Ok(p) => p,
            Err(error) => {
                tracing::warn!(%error, "failed to serialize render input");
                return Vec::new();
            }
        };
        // A poisoned mutex means an EARLIER render panicked while holding it.
        // Bailing out returned empty segments forever — in the CLI path each
        // render is a fresh process so it self-heals, but under the W48 daemon
        // this widget is warm for the process lifetime, so one panic killed
        // the plugin permanently and silently, fixable only by restarting the
        // daemon. `daemon.rs`'s `handle_request` already recovers ITS state
        // mutex the same way. Recovery is sound because the guarded value is
        // an `extism::Plugin` whose own state lives in the wasm instance: a
        // panic in `plugin.call` unwinds Rust-side without leaving a
        // partially-updated Rust struct behind, and genuinely broken guest
        // state surfaces as the next `call` returning `Err`, which the arm
        // below already degrades to empty segments (N2).
        let (mut plugin, _) = recover_poisoned(
            self.plugin.lock(),
            &self.poison_reported,
            self.name.as_str(),
        );
        match plugin.call::<&str, &str>("render", &payload) {
            Ok(out) => decode_render_output(self.name.as_str(), out, &self.decode_reported).0,
            Err(error) => {
                tracing::warn!(%error, "plugin render failed, rendering empty");
                Vec::new()
            }
        }
    }

    fn range_name(&self) -> Option<RangeName> {
        // A plugin is clickable when its name parses as a `RangeName`; the
        // guest decides whether to honor `context.toggled`.
        plugin_range_name(self.name.as_str())
    }
}

/// A plugin's clickable range name: `Some(name)` when it parses as a
/// [`RangeName`]; else `None` (this now also rejects a charset-violating
/// `.wasm` stem, not just an over-length one — the same rule, checked once).
/// Pulled out of `WasmWidget::range_name` so the boundary can be pinned by a
/// hermetic unit test without needing a real `extism::Plugin` instance.
fn plugin_range_name(name: &str) -> Option<RangeName> {
    RangeName::parse(name).ok()
}

/// Decode a guest's `render` output, returning the segments plus the decode
/// error when there was one.
///
/// Split out of [`WasmWidget::render`] so the decode outcome can be asserted by
/// a hermetic unit test without a real `extism::Plugin`, the same way
/// `plugin_range_name` is. `parse_render_output` stays as-is (it is `pub`, and
/// its `unwrap_or_default` contract — malformed output never breaks the bar,
/// invariant N2 — is unchanged); this only makes the failure *reportable*.
///
/// `reported` latches the warning for the same reason [`recover_poisoned`]
/// does: this condition does not clear by itself. A guest whose wire shape
/// disagrees with the host fails to decode on every tick, so warning each time
/// would bury the log rather than inform it. The `Option<Error>` is still
/// returned unconditionally, so suppressing the *warning* never suppresses the
/// caller's ability to see that a decode failed.
fn decode_render_output(
    name: &str,
    out: &str,
    reported: &AtomicBool,
) -> (Vec<Segment>, Option<serde_json::Error>) {
    match serde_json::from_str::<Vec<Segment>>(out) {
        Ok(segments) => (segments, None),
        Err(error) => {
            if !reported.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    plugin = %name,
                    %error,
                    len = out.len(),
                    "malformed plugin render output, rendering empty"
                );
            }
            (Vec::new(), Some(error))
        }
    }
}

/// Resolve a possibly-poisoned plugin lock: hand back the guard either way, and
/// warn at most once per widget. Returns the guard plus whether this call was
/// the one that emitted the warning.
///
/// Split out of [`WasmWidget::render`] for the same reason as
/// [`decode_render_output`]: so a hermetic unit test can drive a genuinely
/// poisoned mutex through the real decision instead of re-modelling it. It is
/// generic over the guarded type only so a test can use a plain `Mutex<i32>` —
/// the latch and recovery it exercises are this same code, just instantiated at
/// a different `T`.
///
/// The one-shot latch matters because a warm daemon renders this widget every
/// tick; suppressing the *warning* must never suppress the *recovery*, which is
/// why the guard is returned unconditionally.
fn recover_poisoned<'a, T>(
    locked: LockResult<MutexGuard<'a, T>>,
    reported: &AtomicBool,
    name: &str,
) -> (MutexGuard<'a, T>, bool) {
    match locked {
        Ok(guard) => (guard, false),
        Err(poisoned) => {
            let first = !reported.swap(true, Ordering::Relaxed);
            if first {
                tracing::warn!(
                    plugin = %name,
                    "plugin mutex was poisoned by an earlier panic; recovering"
                );
            }
            (poisoned.into_inner(), first)
        }
    }
}

#[cfg(test)]
mod tests {
    use rustline_abi::ABI_VERSION;
    use rustline_core::{Config, Context, RangeName, Registry, WidgetName};

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{decode_args, decode_render_output, plugin_range_name, recover_poisoned};
    use crate::abi::{RenderInput, parse_render_output};

    /// A minimal `Context` with `toggled` set to `{name}`, for pinning the
    /// host→guest seam that carries click-toggle state across the wasm
    /// boundary. Fields not listed here come from `Context::default()`.
    fn sample_ctx_with_toggle(name: &str) -> Context {
        Context {
            session_name: "0".into(),
            window_index: "0".into(),
            pane_index: "0".into(),
            pane_current_path: "/".into(),
            home: "/home/steve".into(),
            hostname: "h".into(),
            now: chrono::Local::now(),
            toggled: std::collections::BTreeSet::from([WidgetName::from(name)]),
            ..Default::default()
        }
    }

    #[test]
    fn render_input_serializes_toggled_for_guests() {
        // Build a minimal Context with a toggled entry and assert the guest
        // payload carries it — this is the seam a plugin depends on to honor
        // toggling.
        let json = serde_json::to_string(&RenderInput {
            context: &sample_ctx_with_toggle("weather"),
            config: &serde_json::json!({}),
            abi_version: ABI_VERSION,
        })
        .unwrap();
        assert!(
            json.contains("\"toggled\""),
            "payload carries toggled: {json}"
        );
        assert!(
            json.contains("weather"),
            "payload carries the toggled name: {json}"
        );
    }

    #[test]
    fn parse_output_degrades_on_malformed() {
        assert!(parse_render_output("not json").is_empty());
        let good = r#"[{"text":"x","style":{"fg":null,"bg":null,"bold":false}}]"#;
        assert_eq!(parse_render_output(good).len(), 1);
    }

    #[test]
    fn parse_render_output_still_degrades_to_empty() {
        // `parse_render_output` is no longer on the render path — that is
        // `decode_render_output` above, which has its own tests and holds the
        // N2 guarantee for the bar. This pins the `pub` fn's own contract,
        // since it stays part of the crate's API for external callers.
        assert!(parse_render_output("not json").is_empty());
        assert!(parse_render_output(r#"{"segments":[]}"#).is_empty());
        assert!(!parse_render_output(r#"[{"text":"hi","style":{}}]"#).is_empty());
    }

    #[test]
    fn decode_render_output_reports_the_error_for_malformed_json() {
        // The distinguishing signal the log was missing: which plugin, and why.
        let reported = AtomicBool::new(false);
        let (segs, err) = decode_render_output("weather", r#"{"segments":[]}"#, &reported);
        assert!(segs.is_empty());
        assert!(err.is_some(), "a decode failure must be reportable");

        let (segs, err) =
            decode_render_output("weather", r#"[{"text":"hi","style":{}}]"#, &reported);
        assert_eq!(segs.len(), 1);
        assert!(err.is_none());
    }

    #[test]
    fn a_repeatedly_malformed_guest_is_reported_once_but_still_decodes_every_tick() {
        // A guest whose wire shape disagrees with the host fails on EVERY
        // tick, so the warn is latched — but, exactly as with
        // `recover_poisoned`, suppressing the warning must never suppress the
        // outcome: the caller still gets the error and the empty segments.
        let reported = AtomicBool::new(false);
        for _ in 0..3 {
            let (segs, err) = decode_render_output("weather", r#"{"segments":[]}"#, &reported);
            assert!(segs.is_empty(), "still degrades to empty (N2)");
            assert!(err.is_some(), "and still reports the error to the caller");
        }
        assert!(
            reported.load(Ordering::Relaxed),
            "the latch is armed after the first failure"
        );
    }

    #[test]
    fn a_well_formed_guest_leaves_the_decode_latch_unarmed() {
        // So a plugin that only later starts emitting garbage still gets its
        // one warning.
        let reported = AtomicBool::new(false);
        let (segs, err) =
            decode_render_output("weather", r#"[{"text":"hi","style":{}}]"#, &reported);
        assert_eq!(segs.len(), 1);
        assert!(err.is_none());
        assert!(!reported.load(Ordering::Relaxed));
    }

    /// A genuinely poisoned `Mutex`, for driving `recover_poisoned`'s real
    /// `Err` arm rather than a stand-in for it.
    fn poisoned_mutex(value: u32) -> Arc<Mutex<u32>> {
        let m = Arc::new(Mutex::new(value));
        let m2 = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison it");
        })
        .join();
        assert!(m.lock().is_err(), "the mutex must actually be poisoned");
        m
    }

    #[test]
    fn a_poisoned_lock_is_recovered_rather_than_disabling_the_plugin() {
        // In the daemon this widget is warm for the process lifetime, so
        // bailing out on Err(_) meant one panic killed that plugin
        // permanently and silently.
        let m = poisoned_mutex(7);
        let reported = AtomicBool::new(false);

        let (guard, first) = recover_poisoned(m.lock(), &reported, "weather");
        assert_eq!(*guard, 7, "recovery must still hand back the value");
        assert!(first, "the first poisoned lock reports");
    }

    #[test]
    fn poison_is_reported_once_but_recovered_every_time() {
        // The latch must suppress the WARNING without ever suppressing the
        // RECOVERY — a widget that logged once and then went silently empty
        // forever would be the original bug wearing a hat.
        let m = poisoned_mutex(7);
        let reported = AtomicBool::new(false);

        let (first_guard, first) = recover_poisoned(m.lock(), &reported, "weather");
        assert!(first, "first report happens");
        assert_eq!(*first_guard, 7);
        drop(first_guard);

        for _ in 0..3 {
            let (guard, reported_again) = recover_poisoned(m.lock(), &reported, "weather");
            assert!(!reported_again, "subsequent ticks stay quiet");
            assert_eq!(*guard, 7, "but still recover the guard");
        }
    }

    #[test]
    fn a_healthy_lock_neither_reports_nor_arms_the_latch() {
        let m: Arc<Mutex<u32>> = Arc::new(Mutex::new(7));
        let reported = AtomicBool::new(false);

        let (guard, first) = recover_poisoned(m.lock(), &reported, "weather");
        assert_eq!(*guard, 7);
        assert!(!first, "an unpoisoned lock reports nothing");
        drop(guard);
        assert!(
            !reported.load(Ordering::Relaxed),
            "and must leave the latch unarmed for a real poisoning later"
        );
    }

    #[test]
    fn plugin_range_name_pins_the_15_byte_boundary() {
        // Gives the wasm side its own boundary pin, independent of the
        // rustline-core one, without needing a real `extism::Plugin`.
        let fifteen = "fifteen_bytes__";
        assert_eq!(fifteen.len(), 15);
        assert_eq!(
            plugin_range_name(fifteen),
            Some(RangeName::parse(fifteen).unwrap())
        );

        let sixteen = "this_name_is_16b";
        assert_eq!(sixteen.len(), 16);
        assert_eq!(plugin_range_name(sixteen), None);
    }

    #[test]
    fn plugin_range_name_rejects_a_charset_violating_stem() {
        // T1: a `.wasm` filename stem carrying a `#` used to still be
        // "clickable" here (the old check was length-only), which would have
        // written it unescaped into `#[range=user|...]` markup — the same
        // forgery hole `RangeName` closes for widget/instance names.
        assert_eq!(plugin_range_name("a#[norange]"), None);
    }

    #[test]
    fn register_plugins_missing_dir_is_noop() {
        let mut reg = Registry::new();
        crate::register_plugins(
            &mut reg,
            &Config::default(),
            std::path::Path::new("/no/such/dir"),
            &["weather".into()],
        );
        assert!(!reg.contains("weather"));
    }

    #[test]
    fn decode_args_accepts_a_json_array_and_rejects_anything_else() {
        assert_eq!(
            decode_args(r#"["metadata","--format","{{title}}"]"#).unwrap(),
            vec![
                "metadata".to_string(),
                "--format".to_string(),
                "{{title}}".to_string()
            ]
        );
        assert_eq!(decode_args("[]").unwrap(), Vec::<String>::new());
        assert!(decode_args("not json").is_err());
        assert!(decode_args(r#"{"a":1}"#).is_err());
        assert!(decode_args(r#"[1,2]"#).is_err(), "numbers are not args");
    }

    #[test]
    fn register_plugins_skips_not_needed_and_garbage_wasm() {
        let dir = tempfile::tempdir().unwrap();
        // a garbage .wasm that IS needed -> instantiation fails -> skipped, no panic
        std::fs::write(dir.path().join("weather.wasm"), b"not real wasm").unwrap();
        // a .wasm that is NOT in `needed` -> never touched
        std::fs::write(dir.path().join("other.wasm"), b"nope").unwrap();
        let mut reg = Registry::new();
        crate::register_plugins(
            &mut reg,
            &Config::default(),
            dir.path(),
            &["weather".into()],
        );
        assert!(!reg.contains("weather"));
        assert!(!reg.contains("other"));
    }
}
