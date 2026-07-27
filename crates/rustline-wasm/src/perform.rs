//! The capability-checked effect functions. Each returns a structured result
//! and never panics — the host_fn wrappers just serialize these to JSON.
//!
//! `perform_log` is the one exception to "capability-checked": it is
//! **capability-free by design** (see invariant N1) — it only ever writes to
//! the host's `tracing` subscriber, never the network or filesystem, so
//! there is no allowlist to check and no denied-case test for it.

use crate::abi::{
    CachedExecResult, CachedHttpResult, ExecResult, HttpResult, ReadResult, WriteResult,
};
use crate::argv::canonical_argv;
use crate::cache::{
    CacheEntry, EXEC_NAMESPACE, HTTP_NAMESPACE, age_secs, cache_path, is_fresh, read_entry,
    write_entry,
};
use crate::capability::{CapabilityCtx, DenialKind};
use crate::fetch::Fetcher;
use crate::run::{MAX_OUTPUT_BYTES, Runner};
use crate::state::{check_cap, resolve_for_allowlist, sanitize_relpath};

pub fn perform_http_get(ctx: &CapabilityCtx, url: &str, fetcher: &dyn Fetcher) -> HttpResult {
    if !ctx.allowed_urls.allows(url) {
        ctx.observe_denial(DenialKind::Url, url);
        return HttpResult {
            ok: false,
            error: format!("url not allowed: {url}"),
            ..Default::default()
        };
    }
    match fetcher.get(url) {
        Ok((status, body)) => HttpResult {
            ok: true,
            status,
            body,
            error: String::new(),
        },
        Err(error) => HttpResult {
            ok: false,
            error,
            ..Default::default()
        },
    }
}

/// TTL-cached HTTP GET. Gate-first (denied → no fetch, no cache touch); fresh
/// cache hit served without fetching; on a failed/non-2xx refresh, serve the
/// last-good entry stale if present. Only 2xx responses are cached.
pub fn perform_http_get_cached(
    ctx: &CapabilityCtx,
    url: &str,
    ttl_secs: i64,
    now: &str,
    fetcher: &dyn Fetcher,
) -> CachedHttpResult {
    // 1) gate first (invariant N1): a denied url makes no network call and
    //    touches no cache file.
    if !ctx.allowed_urls.allows(url) {
        ctx.observe_denial(DenialKind::Url, url);
        return CachedHttpResult {
            ok: false,
            error: format!("url not allowed: {url}"),
            ..Default::default()
        };
    }

    let dir = ctx.state_dir();
    let path = cache_path(&dir, HTTP_NAMESPACE, url);
    let entry = read_entry(&path);

    // 2) fresh hit → serve without fetching.
    if let Some(e) = &entry
        && let Some(age) = age_secs(now, &e.fetched_at)
        && is_fresh(age, ttl_secs)
    {
        return CachedHttpResult {
            ok: true,
            status: e.status,
            body: e.body.clone(),
            error: String::new(),
            stale: false,
            age_secs: age,
        };
    }

    // 2b) negative-cache backoff: a refresh was attempted within the last TTL
    //     window and we still hold a last-good entry, so serve it immediately
    //     rather than paying another blocking fetch. Without this, a dead
    //     upstream costs a full fetch timeout on EVERY render forever.
    if let Some(e) = &entry
        && let Some(since_attempt) = age_secs(now, &e.last_attempt_at)
        && is_fresh(since_attempt, ttl_secs)
    {
        return CachedHttpResult {
            ok: true,
            status: e.status,
            body: e.body.clone(),
            error: "refresh backoff; serving cached".to_string(),
            stale: true,
            age_secs: age_secs(now, &e.fetched_at).unwrap_or(0),
        };
    }

    // 3) refresh.
    match fetcher.get(url) {
        Ok((status, body)) if (200..300).contains(&status) => {
            let content = serde_json::to_string(&CacheEntry {
                fetched_at: now.to_string(),
                status,
                body: body.clone(),
                last_attempt_at: now.to_string(),
            })
            .unwrap_or_default();
            match write_entry(&dir, &path, &content, ctx.state_size(), ctx.max_state_bytes) {
                Ok(total) => ctx.set_state_size(total),
                Err(error) => {
                    ctx.invalidate_state_size();
                    tracing::warn!(
                        %error,
                        %url,
                        "http cache write failed; returning body unpersisted"
                    );
                }
            }
            CachedHttpResult {
                ok: true,
                status,
                body,
                error: String::new(),
                stale: false,
                age_secs: 0,
            }
        }
        // non-2xx or transport error → refresh failed.
        other => {
            let error = match other {
                Ok((status, _)) => format!("http status {status}"),
                Err(e) => e,
            };
            match entry {
                // serve last-good stale (no egress beyond the failed attempt).
                Some(e) => {
                    let age = age_secs(now, &e.fetched_at).unwrap_or(0);
                    // Record the failed attempt so the next render backs off
                    // instead of re-entering a blocking fetch. body/status/
                    // fetched_at are deliberately untouched: this entry is
                    // still the last-good response.
                    let content = serde_json::to_string(&CacheEntry {
                        fetched_at: e.fetched_at.clone(),
                        status: e.status,
                        body: e.body.clone(),
                        last_attempt_at: now.to_string(),
                    })
                    .unwrap_or_default();
                    match write_entry(&dir, &path, &content, ctx.state_size(), ctx.max_state_bytes)
                    {
                        Ok(total) => ctx.set_state_size(total),
                        Err(write_error) => {
                            ctx.invalidate_state_size();
                            tracing::warn!(error = %write_error, "negative-cache write failed");
                        }
                    }
                    CachedHttpResult {
                        ok: true,
                        status: e.status,
                        body: e.body,
                        error,
                        stale: true,
                        age_secs: age,
                    }
                }
                // Nothing to negative-cache against, and writing a body-less
                // entry would corrupt the stale-serve path — behaviour here
                // is unchanged.
                None => CachedHttpResult {
                    ok: false,
                    error,
                    ..Default::default()
                },
            }
        }
    }
}

/// Run a command on the guest's behalf, gated by `allowed_commands`.
///
/// **Gate first (invariant N1):** the whole canonical argv (program + args,
/// quoted — see `argv::canonical_argv`) is checked against the allowlist
/// before anything is spawned, so a denied call never touches `runner` at
/// all. The gate is on the *whole* argv, not just the program — a grant for
/// `git status*` does not cover `git push`. Once allowed, `program`/`args`
/// are passed straight through to `runner`, never assembled into a shell
/// string.
pub fn perform_exec(
    ctx: &CapabilityCtx,
    program: &str,
    args: &[String],
    runner: &dyn Runner,
) -> ExecResult {
    let candidate = canonical_argv(program, args);
    if !ctx.allowed_commands.allows(&candidate) {
        ctx.observe_denial(DenialKind::Command, &candidate);
        return ExecResult {
            ok: false,
            status: -1,
            error: format!("command not allowed: {candidate}"),
            ..Default::default()
        };
    }
    match runner.run(program, args) {
        Ok((status, stdout, stderr)) => ExecResult {
            ok: true,
            status,
            truncated: is_truncated(&stdout, &stderr),
            stdout,
            stderr,
            error: String::new(),
        },
        Err(error) => ExecResult {
            ok: false,
            status: -1,
            error,
            ..Default::default()
        },
    }
}

/// TTL-cached command run, closely mirroring [`perform_http_get_cached`]'s
/// shape with "2xx" replaced by "exit 0" — but with one deliberate
/// difference: a **non-zero exit is real, fresh data** (the command ran and
/// genuinely reported that status), not a transient failure, so unlike an
/// HTTP non-2xx it is returned as-is rather than triggering a fall back to a
/// stale cached entry. Only a run that couldn't happen at all (denied, spawn
/// failure, or timeout) falls back to the last-good entry, stale, if one
/// exists — a denied run touches no cache file at all, under its own
/// `EXEC_NAMESPACE` so it can never collide with the HTTP cache (see
/// `cache::cache_path`). A fresh cache hit is served without re-running; only
/// a **zero-exit** run is ever written to the cache.
///
/// Known round-trip limitation: [`CacheEntry`] persists only `fetched_at`/
/// `status`/`body`, not `truncated`. So a fresh-hit or stale-serve here always
/// reports `truncated: false`, even when the run that originally populated
/// the entry *was* truncated. `truncated` is purely informational (never a
/// gating/security property), so this isn't worth widening the on-disk cache
/// format for — but a cached `truncated: false` should not be read as
/// authoritative.
pub fn perform_exec_cached(
    ctx: &CapabilityCtx,
    program: &str,
    args: &[String],
    ttl_secs: i64,
    now: &str,
    runner: &dyn Runner,
) -> CachedExecResult {
    let candidate = canonical_argv(program, args);
    if !ctx.allowed_commands.allows(&candidate) {
        ctx.observe_denial(DenialKind::Command, &candidate);
        return CachedExecResult {
            ok: false,
            status: -1,
            error: format!("command not allowed: {candidate}"),
            ..Default::default()
        };
    }

    let dir = ctx.state_dir();
    let path = cache_path(&dir, EXEC_NAMESPACE, &candidate);
    let entry = read_entry(&path);

    if let Some(e) = &entry
        && let Some(age) = age_secs(now, &e.fetched_at)
        && is_fresh(age, ttl_secs)
    {
        return CachedExecResult {
            ok: true,
            status: i32::from(e.status),
            stdout: e.body.clone(),
            // `CacheEntry` doesn't persist stderr (only the successful
            // stdout body is worth caching); a served entry always reports
            // it empty, fresh or stale.
            stderr: String::new(),
            error: String::new(),
            stale: false,
            age_secs: age,
            // Round-trip limitation: `CacheEntry` doesn't persist whether the
            // original run was truncated — see this function's doc comment.
            truncated: false,
        };
    }

    // negative-cache backoff: mirrors `perform_http_get_cached`'s — a refresh
    // was attempted within the last TTL window and we still hold a last-good
    // entry, so serve it immediately rather than paying another blocking
    // spawn/timeout.
    if let Some(e) = &entry
        && let Some(since_attempt) = age_secs(now, &e.last_attempt_at)
        && is_fresh(since_attempt, ttl_secs)
    {
        return CachedExecResult {
            ok: true,
            status: i32::from(e.status),
            stdout: e.body.clone(),
            stderr: String::new(),
            error: "refresh backoff; serving cached".to_string(),
            stale: true,
            age_secs: age_secs(now, &e.fetched_at).unwrap_or(0),
            truncated: false,
        };
    }

    match runner.run(program, args) {
        Ok((0, stdout, stderr)) => {
            let content = serde_json::to_string(&CacheEntry {
                fetched_at: now.to_string(),
                status: 0,
                body: stdout.clone(),
                last_attempt_at: now.to_string(),
            })
            .unwrap_or_default();
            match write_entry(&dir, &path, &content, ctx.state_size(), ctx.max_state_bytes) {
                Ok(total) => ctx.set_state_size(total),
                Err(error) => {
                    ctx.invalidate_state_size();
                    tracing::warn!(
                        %error,
                        %candidate,
                        "exec cache write failed; returning body unpersisted"
                    );
                }
            }
            CachedExecResult {
                ok: true,
                status: 0,
                truncated: is_truncated(&stdout, &stderr),
                stdout,
                stderr,
                error: String::new(),
                stale: false,
                age_secs: 0,
            }
        }
        // Ran, but a non-zero exit is data, not an error: return it as-is
        // without caching it (only a successful run is worth serving stale
        // later).
        Ok((status, stdout, stderr)) => CachedExecResult {
            ok: true,
            status,
            truncated: is_truncated(&stdout, &stderr),
            stdout,
            stderr,
            error: String::new(),
            stale: false,
            age_secs: 0,
        },
        // Couldn't run at all (denied is already handled above; this is a
        // spawn failure or timeout): serve the last-good entry stale if one
        // exists, exactly like a failed HTTP refresh.
        Err(error) => match entry {
            Some(e) => {
                let age = age_secs(now, &e.fetched_at).unwrap_or(0);
                // Record the failed attempt so the next render backs off
                // instead of re-entering a blocking spawn/timeout. body/
                // status/fetched_at are deliberately untouched: this entry is
                // still the last-good response.
                let content = serde_json::to_string(&CacheEntry {
                    fetched_at: e.fetched_at.clone(),
                    status: e.status,
                    body: e.body.clone(),
                    last_attempt_at: now.to_string(),
                })
                .unwrap_or_default();
                match write_entry(&dir, &path, &content, ctx.state_size(), ctx.max_state_bytes) {
                    Ok(total) => ctx.set_state_size(total),
                    Err(write_error) => {
                        ctx.invalidate_state_size();
                        tracing::warn!(error = %write_error, "negative-cache write failed");
                    }
                }
                CachedExecResult {
                    ok: true,
                    status: i32::from(e.status),
                    stdout: e.body,
                    stderr: String::new(),
                    error,
                    stale: true,
                    age_secs: age,
                    // Round-trip limitation: see this function's doc comment.
                    truncated: false,
                }
            }
            // Nothing to negative-cache against, and writing a body-less
            // entry would corrupt the stale-serve path — behaviour here is
            // unchanged.
            None => CachedExecResult {
                ok: false,
                error,
                ..Default::default()
            },
        },
    }
}

/// True iff either stream hit [`MAX_OUTPUT_BYTES`] and was truncated. See
/// `run::MAX_OUTPUT_BYTES`'s doc comment for why comparing the lossily
/// UTF-8-converted string's length this way is still a safe (no
/// false-negative) signal.
fn is_truncated(stdout: &str, stderr: &str) -> bool {
    stdout.len() >= MAX_OUTPUT_BYTES || stderr.len() >= MAX_OUTPUT_BYTES
}

pub fn perform_state_read(ctx: &CapabilityCtx, relpath: &str) -> ReadResult {
    let rel = match sanitize_relpath(relpath) {
        Ok(r) => r,
        Err(error) => {
            return ReadResult {
                ok: false,
                error,
                ..Default::default()
            };
        }
    };
    let full = ctx.state_dir().join(rel);
    match std::fs::read_to_string(&full) {
        Ok(contents) => ReadResult {
            ok: true,
            exists: true,
            contents,
            error: String::new(),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ReadResult {
            ok: true,
            exists: false,
            ..Default::default()
        },
        Err(e) => ReadResult {
            ok: false,
            error: e.to_string(),
            ..Default::default()
        },
    }
}

pub fn perform_state_write(ctx: &CapabilityCtx, relpath: &str, contents: &str) -> WriteResult {
    let rel = match sanitize_relpath(relpath) {
        Ok(r) => r,
        Err(error) => return WriteResult { ok: false, error },
    };
    let dir = ctx.state_dir();
    let full = dir.join(rel);
    let new_len = contents.len() as u64;
    if check_cap(ctx.state_size(), &full, new_len, ctx.max_state_bytes).is_err() {
        // The refusal may be the memo's own fault: it can go stale-high
        // whenever the state dir shrinks behind this ctx's back — a user
        // clearing the plugin's state dir, a crashed-render staging orphan
        // being reaped, a concurrent render's cache eviction. `write_entry`'s
        // quota-failure path already self-heals by re-measuring after its
        // own eviction; a bare refusal here previously did not, so it never
        // recovered until something else happened to invalidate the memo.
        // Invalidating and re-checking once costs a walk only on this rare
        // refusal path, and can only make the decision more accurate, never
        // looser (invariant N3).
        ctx.invalidate_state_size();
        if let Err(error) = check_cap(ctx.state_size(), &full, new_len, ctx.max_state_bytes) {
            return WriteResult { ok: false, error };
        }
    }
    // Captured once now that the check above has passed: nothing between
    // here and the write below changes the memo, so re-reading `state_size()`
    // again in the success arm would only return this same value.
    let current_size = ctx.state_size();
    if let Some(parent) = full.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return WriteResult {
            ok: false,
            error: e.to_string(),
        };
    }
    // Read before the write: the file being replaced (if any) still has its
    // old size right up until `write_atomic` lands, and the memo update
    // below needs that old size to net out correctly against the new one.
    let replaced = std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
    match rustline_core::atomic_write::write_atomic(&full, contents.as_bytes()) {
        Ok(()) => {
            ctx.set_state_size(
                current_size
                    .saturating_sub(replaced)
                    .saturating_add(new_len),
            );
            WriteResult {
                ok: true,
                error: String::new(),
            }
        }
        Err(e) => {
            // A failed write should leave the memo untouched in the common
            // case (`write_atomic` cleans up its own staging file), but
            // trusting that here couples this call site to that internal
            // detail. Invalidating costs one extra walk on the next write and
            // is always correct — when in doubt, invalidate (invariant N3).
            ctx.invalidate_state_size();
            WriteResult {
                ok: false,
                error: e.to_string(),
            }
        }
    }
}

/// Read an arbitrary file, gated by `allowed_paths` — the **read** allowlist;
/// `allowed_write_paths` does not authorize a read (see `perform_file_write`
/// for the write counterpart and why the two are separate).
///
/// **resolve → match → act (invariant N1, order load-bearing):** the path is
/// resolved for symlinks (`resolve_for_allowlist`) strictly before the
/// allowlist check, and the check strictly before the filesystem is touched —
/// resolving after matching would let a symlink component slip past the
/// allowlist entirely.
pub fn perform_file_read(ctx: &CapabilityCtx, path: &str) -> ReadResult {
    let norm = match resolve_for_allowlist(path, ctx.resolve_symlinks) {
        Ok(p) => p,
        Err(error) => {
            return ReadResult {
                ok: false,
                error,
                ..Default::default()
            };
        }
    };
    if !ctx.allowed_paths.allows(&norm) {
        ctx.observe_denial(DenialKind::Path, &norm);
        return ReadResult {
            ok: false,
            error: format!("path not allowed: {norm}"),
            ..Default::default()
        };
    }
    match std::fs::read_to_string(&norm) {
        Ok(contents) => ReadResult {
            ok: true,
            exists: true,
            contents,
            error: String::new(),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ReadResult {
            ok: true,
            exists: false,
            ..Default::default()
        },
        Err(e) => ReadResult {
            ok: false,
            error: e.to_string(),
            ..Default::default()
        },
    }
}

/// Write an arbitrary file, gated by `allowed_write_paths` — the **write**
/// allowlist, deliberately separate from the read-only `allowed_paths`. The
/// two used to be one list: approving a manifest that requested a path
/// advertised as "read your aliases to show a badge" also handed the plugin
/// arbitrary overwrite of that file (see `PluginConfig::allowed_write_paths`'s
/// doc for the exploit and the migration ruling — an existing `allowed_paths`
/// entry now grants read only).
///
/// **resolve → match → act (invariant N1, order load-bearing):** see
/// `perform_file_read`'s doc — the same ordering applies here.
pub fn perform_file_write(ctx: &CapabilityCtx, path: &str, contents: &str) -> WriteResult {
    let norm = match resolve_for_allowlist(path, ctx.resolve_symlinks) {
        Ok(p) => p,
        Err(error) => return WriteResult { ok: false, error },
    };
    if !ctx.allowed_write_paths.allows(&norm) {
        ctx.observe_denial(DenialKind::Path, &norm);
        return WriteResult {
            ok: false,
            error: format!("path not allowed: {norm}"),
        };
    }
    // Deliberately not `write_atomic`: `path` is an arbitrary allowlisted
    // absolute path a plugin asked to write, not a temp/cache/state file this
    // module owns. Rename-replace would change the target's inode and
    // permissions and break existing hard links — not what a plugin writing
    // to a user-designated path should do.
    match std::fs::write(&norm, contents.as_bytes()) {
        Ok(()) => WriteResult {
            ok: true,
            error: String::new(),
        },
        Err(e) => WriteResult {
            ok: false,
            error: e.to_string(),
        },
    }
}

/// Longest guest log message forwarded to `tracing`, in bytes — this bound
/// includes the `TRUNCATION_MARKER` appended on truncation, so a truncated
/// message's *body* is capped to `MAX_GUEST_LOG_BYTES - TRUNCATION_MARKER.len()`
/// and the total never exceeds this constant. Keeping the marker inside the
/// budget (rather than appended on top of it) is what makes this doc
/// literally true instead of quietly overselling by `TRUNCATION_MARKER.len()`
/// bytes — harmless at 2 KiB, but wrong if this is ever tuned against a real
/// limit such as a syslog line cap.
pub(crate) const MAX_GUEST_LOG_BYTES: usize = 2 * 1024;

/// Most `rl_log` calls forwarded per plugin instance per process.
///
/// 256 and "per process, not per render" are both deliberate choices, not
/// defaults that happened: the cap exists to bound how much a buggy loop or
/// hostile plugin can write to the user's disk over the life of the
/// process — the abuse case — not to give a well-behaved plugin a fresh
/// debugging allowance every render. Resetting the budget per render would
/// defeat it against exactly the failure mode it exists to stop, since a
/// plugin logging every render would then log forever. That is harmless on
/// the CLI path, where a fresh process (and so a fresh budget) starts every
/// render; it means the counter accumulates across the daemon's whole
/// lifetime once a plugin instance is warm there — see
/// [`CapabilityCtx`]'s `log_calls` field
/// for what that costs a well-behaved plugin under the daemon.
pub(crate) const MAX_GUEST_LOG_CALLS: u32 = 256;

/// Appended to a guest message that was truncated. Budgeted *inside*
/// `MAX_GUEST_LOG_BYTES` (see its doc), not added on top of it.
const TRUNCATION_MARKER: &str = "…(truncated)";

/// Truncate `msg` to at most `MAX_GUEST_LOG_BYTES` total, including
/// `TRUNCATION_MARKER` when it truncates, and never splitting a character.
/// The message is guest-supplied UTF-8, so a byte-index slice would panic on
/// a multi-byte char straddling the cap.
fn truncate_guest_msg(msg: &str) -> std::borrow::Cow<'_, str> {
    if msg.len() <= MAX_GUEST_LOG_BYTES {
        return std::borrow::Cow::Borrowed(msg);
    }
    let body_cap = MAX_GUEST_LOG_BYTES.saturating_sub(TRUNCATION_MARKER.len());
    let mut end = body_cap;
    while end > 0 && !msg.is_char_boundary(end) {
        end -= 1;
    }
    std::borrow::Cow::Owned(format!("{}{TRUNCATION_MARKER}", &msg[..end]))
}

/// Emit a guest log message through the host's `tracing` subscriber.
///
/// Unlike every other function in this module, `rl_log` is the **one
/// intentional capability-free host function** (invariant N1): it never
/// touches the network or filesystem, so there is no `CapabilityCtx`
/// allowlist to check and — unlike `perform_http_get`/`perform_state_read`/
/// etc. — no "denied" case to test. `plugin` (from `ctx.name`) tags the log
/// line so multi-plugin output stays attributable; an unrecognized `level`
/// string degrades to `info` (keeping the original string as a field) rather
/// than dropping the message or panicking, matching invariant N2 (a plugin
/// must never break the bar).
///
/// Capability-free does not mean unbounded, though: a guest with 16 MB of
/// memory and a 500 M-fuel render budget can still issue thousands of calls
/// or one huge message in a single render, and the log this function writes
/// to is the user's only diagnostic when something goes wrong — so it must
/// never be the thing that fills their disk. `msg` is truncated to
/// `MAX_GUEST_LOG_BYTES` and calls are capped at `MAX_GUEST_LOG_CALLS` per
/// instance per process — "per process" means per render on the CLI path,
/// but per daemon lifetime under the daemon, since a `CapabilityCtx` there is
/// built once and kept warm (see `MAX_GUEST_LOG_CALLS`'s doc and
/// [`CapabilityCtx`]'s `log_calls` field
/// for what that costs a well-behaved plugin). Neither check is a capability
/// gate: nothing is denied, no `observe_denial` fires, and there is still no
/// allowlist — every call still reaches `tracing`, just bounded in size and
/// rate rather than admitted or refused.
pub fn perform_log(ctx: &CapabilityCtx, level: &str, msg: &str) {
    let calls = ctx.claim_log_call();
    if calls > MAX_GUEST_LOG_CALLS {
        if calls == MAX_GUEST_LOG_CALLS + 1 {
            tracing::warn!(
                target: "rustline_wasm::guest",
                plugin = %ctx.name,
                limit = MAX_GUEST_LOG_CALLS,
                "guest log rate limit reached; further messages dropped this process"
            );
        }
        return;
    }
    let plugin = &ctx.name;
    let msg = truncate_guest_msg(msg);
    match level {
        "error" => tracing::error!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        "warn" => tracing::warn!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        "info" => tracing::info!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        "debug" => tracing::debug!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        "trace" => tracing::trace!(target: "rustline_wasm::guest", %plugin, "{msg}"),
        _ => {
            tracing::info!(target: "rustline_wasm::guest", %plugin, orig_level = %level, "{msg}")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::capability::{CapabilityCtx, DenialKind, DenialObserver};
    use rustline_core::PluginConfig;

    /// A spy [`DenialObserver`] that records every `(plugin, kind, target)`
    /// it's told about, so a denied-case test can assert the seam fired
    /// alongside the existing `ok:false`/no-side-effect assertions.
    #[derive(Default, Clone)]
    struct SpyObserver(Arc<Mutex<Vec<(String, DenialKind, String)>>>);

    impl DenialObserver for SpyObserver {
        fn observe(&self, plugin: &str, kind: DenialKind, target: &str) {
            self.0
                .lock()
                .unwrap()
                .push((plugin.to_string(), kind, target.to_string()));
        }
    }

    impl SpyObserver {
        fn records(&self) -> Vec<(String, DenialKind, String)> {
            self.0.lock().unwrap().clone()
        }
    }

    struct FakeFetcher(u16, &'static str);
    impl crate::fetch::Fetcher for FakeFetcher {
        fn get(&self, _url: &str) -> Result<(u16, String), String> {
            Ok((self.0, self.1.to_string()))
        }
    }
    struct DeadFetcher;
    impl crate::fetch::Fetcher for DeadFetcher {
        fn get(&self, _url: &str) -> Result<(u16, String), String> {
            Err("connection refused".into())
        }
    }

    struct CountingFetcher {
        calls: std::sync::Arc<AtomicUsize>,
        status: u16,
        body: &'static str,
    }
    impl crate::fetch::Fetcher for CountingFetcher {
        fn get(&self, _url: &str) -> Result<(u16, String), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok((self.status, self.body.to_string()))
        }
    }

    fn ctx_with(urls: &[&str], root: std::path::PathBuf) -> CapabilityCtx {
        ctx_with_cap(urls, root, 16)
    }

    /// A `CapabilityCtx` with `allowed_paths`/`allowed_write_paths`/
    /// `resolve_symlinks` set directly, for the read/write producer tests
    /// below. The tempdir root is canonicalized before use as the state
    /// root: `tempfile::tempdir()` can itself sit under a symlink on some
    /// systems (e.g. macOS's `/tmp` -> `/private/tmp`), and this ctx must not
    /// introduce that as an incidental extra symlink hop unrelated to the
    /// symlink each test is actually exercising.
    fn test_ctx_paths(
        dir: &tempfile::TempDir,
        allowed_paths: &[&str],
        allowed_write_paths: &[&str],
        resolve_symlinks: bool,
    ) -> CapabilityCtx {
        let root = std::fs::canonicalize(dir.path()).unwrap_or_else(|_| dir.path().to_path_buf());
        let pc = PluginConfig {
            allowed_paths: allowed_paths.iter().map(|s| s.to_string()).collect(),
            allowed_write_paths: allowed_write_paths.iter().map(|s| s.to_string()).collect(),
            resolve_symlinks,
            ..PluginConfig::default()
        };
        CapabilityCtx::from_config("weather", &pc, root)
    }

    // The cache tests wrap each body in a JSON envelope (fetched_at + status +
    // body), so even a short body's entry is well over `ctx_with`'s 16-byte
    // cap; tests that need a write to actually persist use a roomy cap here,
    // leaving `ctx_with`'s tight cap for the tests that exercise it directly.
    fn ctx_with_cap(
        urls: &[&str],
        root: std::path::PathBuf,
        max_state_bytes: u64,
    ) -> CapabilityCtx {
        let pc = PluginConfig {
            allowed_urls: urls.iter().map(|s| s.to_string()).collect(),
            max_state_bytes,
            ..PluginConfig::default()
        };
        CapabilityCtx::from_config("weather", &pc, root)
    }

    #[test]
    fn http_denied_when_not_allowlisted_makes_no_request() {
        let spy = SpyObserver::default();
        let ctx = ctx_with(&[], std::env::temp_dir()).with_observer(Arc::new(spy.clone()));
        let r = perform_http_get(&ctx, "https://wttr.in/48183", &FakeFetcher(200, "hi"));
        assert!(!r.ok);
        assert!(r.error.contains("not allowed"));
        assert_eq!(
            spy.records(),
            vec![(
                "weather".to_string(),
                DenialKind::Url,
                "https://wttr.in/48183".to_string()
            )]
        );
    }

    #[test]
    fn http_allowed_returns_body() {
        let ctx = ctx_with(&["https://wttr.in/*"], std::env::temp_dir());
        let r = perform_http_get(&ctx, "https://wttr.in/48183", &FakeFetcher(200, "sunny"));
        assert!(r.ok);
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "sunny");
    }

    #[test]
    fn http_transport_error_reports_not_ok() {
        let ctx = ctx_with(&["https://wttr.in/*"], std::env::temp_dir());
        let r = perform_http_get(&ctx, "https://wttr.in/48183", &DeadFetcher);
        assert!(!r.ok);
        assert!(r.error.contains("refused"));
    }

    #[test]
    fn state_write_then_read_roundtrips_and_enforces_cap() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&[], root.path().to_path_buf());
        let w = perform_state_write(&ctx, "weather.json", "0123456789"); // 10 bytes, cap 16
        assert!(w.ok, "{:?}", w.error);
        let r = perform_state_read(&ctx, "weather.json");
        assert!(r.ok && r.exists);
        assert_eq!(r.contents, "0123456789");
        // read of an absent file: ok but exists=false
        let miss = perform_state_read(&ctx, "nope.json");
        assert!(miss.ok && !miss.exists);
        // a second big write over cap is refused
        let over = perform_state_write(&ctx, "big.json", "0123456789ABCDEF01"); // 18 > 16
        assert!(!over.ok);
        assert!(over.error.contains("quota"));
    }

    /// A single oversized write already fails the cap check regardless of
    /// what `current_size` it's handed, so
    /// `state_write_then_read_roundtrips_and_enforces_cap` above can't tell a
    /// correctly-updated memo from one that's stuck at its seed value. This
    /// pins the case that actually distinguishes them: two writes, each
    /// individually within the cap, whose *combined* total is not — a memo
    /// that never advances past its initial (empty-dir) reading would let
    /// the second one through.
    #[test]
    fn state_write_refuses_a_second_write_that_only_exceeds_cap_cumulatively() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&[], root.path().to_path_buf(), 20);
        let first = perform_state_write(&ctx, "a.json", &"a".repeat(15));
        assert!(first.ok, "{:?}", first.error);
        // True total is now 15 bytes. A second, independent 10-byte file
        // would put the true total at 25 > cap 20, and must be refused.
        let second = perform_state_write(&ctx, "b.json", &"b".repeat(10));
        assert!(
            !second.ok,
            "a write that only exceeds the cap once combined with the first must still be refused"
        );
        assert!(second.error.contains("quota"));
    }

    /// Fix pass 1 (Important 2): before this task every `check_cap` re-walked,
    /// so a refusal caused by a stale-high memo self-corrected on the very
    /// next write. After the memo, a bare refusal on this path never
    /// invalidated, so nothing forced a re-walk — reachable whenever the
    /// state dir shrinks behind this ctx's back (a user clearing the
    /// plugin's state dir, a crashed-render staging orphan being reaped, a
    /// concurrent render's cache eviction). This pins the self-heal: a
    /// refusal caused purely by a stale-high memo must recover on its own
    /// retry, without needing a later write to invalidate it first.
    #[test]
    fn state_write_self_heals_a_stale_high_memo_on_refusal() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&[], root.path().to_path_buf(), 100);
        // Simulate the state dir having shrunk behind this ctx's back: the
        // memo believes it's already full, even though the real dir is
        // empty.
        ctx.set_state_size(1_000);
        let w = perform_state_write(&ctx, "a.json", "hello");
        assert!(
            w.ok,
            "a refusal driven only by a stale-high memo must self-heal via one re-walk: {:?}",
            w.error
        );
    }

    /// The self-heal above must not turn every refusal into a pass: when the
    /// state dir really is over cap, the re-walk confirms that and the write
    /// stays refused.
    #[test]
    fn state_write_stays_refused_when_the_re_walk_confirms_it() {
        let root = tempfile::tempdir().unwrap();
        let cap = 10;
        let ctx = ctx_with_cap(&[], root.path().to_path_buf(), cap);
        std::fs::create_dir_all(ctx.state_dir()).unwrap();
        std::fs::write(ctx.state_dir().join("existing.json"), "z".repeat(9)).unwrap();
        let w = perform_state_write(&ctx, "a.json", "0123456789"); // 9 + 10 > cap 10
        assert!(
            !w.ok,
            "a write that truly exceeds cap must stay refused after the self-heal retry"
        );
        assert!(w.error.contains("quota"));
    }

    /// The load-bearing performance claim this whole task exists for: a
    /// sequence of writes through the real per-render entry point pays one
    /// `dir_size` walk total (the memo's seed), not one per write.
    #[test]
    fn many_state_writes_pay_exactly_one_dir_size_walk() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&[], root.path().to_path_buf(), 1_000_000);
        crate::state::reset_walk_count();
        for i in 0..50 {
            let w = perform_state_write(&ctx, &format!("f{i}.json"), "x");
            assert!(w.ok, "{:?}", w.error);
        }
        assert_eq!(
            crate::state::walk_count(),
            1,
            "50 writes should cost exactly one dir_size walk, not one per write"
        );
    }

    #[test]
    fn state_write_rejects_traversal() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&[], root.path().to_path_buf());
        let w = perform_state_write(&ctx, "../escape", "x");
        assert!(!w.ok);
        assert!(w.error.contains("traversal"));
    }

    // Pins that `perform_state_write` actually goes through `write_atomic`
    // rather than truncating the target in place: `fs::write` reuses the
    // target file's inode, `write_atomic` replaces it via `rename`. Nothing
    // else here would notice a regression to a bare `fs::write` —
    // `state_write_then_read_roundtrips_and_enforces_cap` reads back the same
    // bytes either way.
    #[test]
    #[cfg(unix)]
    fn state_write_replaces_the_file_rather_than_truncating_it_in_place() {
        use std::os::unix::fs::MetadataExt as _;

        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&[], root.path().to_path_buf(), 1_000);
        let w = perform_state_write(&ctx, "weather.json", "first");
        assert!(w.ok, "{:?}", w.error);
        let path = ctx.state_dir().join("weather.json");
        let first_ino = std::fs::metadata(&path).unwrap().ino();
        let w = perform_state_write(&ctx, "weather.json", "second, and longer");
        assert!(w.ok, "{:?}", w.error);
        let second_ino = std::fs::metadata(&path).unwrap().ino();
        assert_ne!(
            first_ino, second_ino,
            "perform_state_write must replace the file, not truncate it in place"
        );
    }

    #[test]
    fn file_read_denied_when_not_allowlisted() {
        let spy = SpyObserver::default();
        let ctx = ctx_with(&[], std::env::temp_dir()).with_observer(Arc::new(spy.clone()));
        let r = perform_file_read(&ctx, "/etc/hostname");
        assert!(!r.ok);
        assert!(r.error.contains("not allowed"));
        assert_eq!(
            spy.records(),
            vec![(
                "weather".to_string(),
                DenialKind::Path,
                "/etc/hostname".to_string()
            )]
        );
    }

    #[test]
    fn file_write_denied_when_not_allowlisted() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("should_not_be_written.txt");
        let spy = SpyObserver::default();
        let ctx = ctx_with(&[], std::env::temp_dir()).with_observer(Arc::new(spy.clone()));
        let w = perform_file_write(&ctx, target.to_str().unwrap(), "secret");
        assert!(!w.ok);
        assert!(w.error.contains("not allowed"));
        assert!(!target.exists(), "denied write must not create the file");
        assert_eq!(
            spy.records(),
            vec![(
                "weather".to_string(),
                DenialKind::Path,
                target.to_str().unwrap().to_string()
            )]
        );
    }

    // The following eight tests characterize the read/write path-grant split
    // and the symlink escape it closes (invariant N1: `allowed_paths` is
    // read-only, `allowed_write_paths` is the separate write grant; a grant
    // is matched against the resolved location, not the name, once
    // `resolve_symlinks` is on). Every legitimate producer of a read or write
    // grant gets a test here — this narrows a shared funnel, so nothing is
    // optional.

    #[test]
    fn a_read_grant_still_reads() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("notes.txt");
        std::fs::write(&f, "hi").unwrap();
        let ctx = test_ctx_paths(&dir, &[&format!("{}/*", dir.path().display())], &[], false);
        let r = perform_file_read(&ctx, f.to_str().unwrap());
        assert!(r.ok && r.exists && r.contents == "hi");
    }

    #[test]
    fn a_read_grant_alone_does_not_permit_a_write() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("notes.txt");
        std::fs::write(&f, "original").unwrap();
        let ctx = test_ctx_paths(&dir, &[&format!("{}/*", dir.path().display())], &[], false);
        let w = perform_file_write(&ctx, f.to_str().unwrap(), "pwned");
        assert!(!w.ok, "allowed_paths is read-only");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "original");
    }

    #[test]
    fn a_write_grant_permits_a_write() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("notes.txt");
        let ctx = test_ctx_paths(&dir, &[], &[&format!("{}/*", dir.path().display())], false);
        let w = perform_file_write(&ctx, f.to_str().unwrap(), "written");
        assert!(w.ok, "{}", w.error);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "written");
    }

    #[test]
    fn a_write_grant_alone_does_not_permit_a_read() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("notes.txt");
        std::fs::write(&f, "secret").unwrap();
        let ctx = test_ctx_paths(&dir, &[], &[&format!("{}/*", dir.path().display())], false);
        let r = perform_file_read(&ctx, f.to_str().unwrap());
        assert!(!r.ok, "a write grant is not a read grant");
    }

    #[test]
    fn a_symlink_under_a_grant_is_denied_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("bashrc");
        std::fs::write(&target, "original").unwrap();
        let link = dir.path().join("notes.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let glob = format!("{}/*", dir.path().display());
        let ctx = test_ctx_paths(&dir, &[&glob], &[&glob], false);

        assert!(!perform_file_read(&ctx, link.to_str().unwrap()).ok);
        assert!(!perform_file_write(&ctx, link.to_str().unwrap(), "pwned").ok);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    }

    #[test]
    fn resolve_symlinks_matches_the_allowlist_against_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("bashrc");
        std::fs::write(&target, "original").unwrap();
        let link = dir.path().join("notes.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let glob = format!("{}/*", dir.path().display());
        let ctx = test_ctx_paths(&dir, &[&glob], &[&glob], true);

        // Resolution happens BEFORE matching, so the escape is caught by the
        // allowlist rather than slipping past it.
        assert!(!perform_file_write(&ctx, link.to_str().unwrap(), "pwned").ok);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    }

    #[test]
    fn resolve_symlinks_allows_a_link_whose_target_is_inside_the_grant() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        std::fs::write(&real, "hi").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let glob = format!("{}/*", dir.path().display());
        let ctx = test_ctx_paths(&dir, &[&glob], &[], true);
        let r = perform_file_read(&ctx, link.to_str().unwrap());
        assert!(r.ok && r.contents == "hi", "{}", r.error);
    }

    #[test]
    fn a_not_yet_existing_write_target_under_a_grant_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("new.txt");
        let glob = format!("{}/*", dir.path().display());
        let ctx = test_ctx_paths(&dir, &[], &[&glob], true);
        let w = perform_file_write(&ctx, f.to_str().unwrap(), "fresh");
        assert!(w.ok, "{}", w.error);
    }

    #[test]
    fn cached_denied_url_makes_no_request_and_no_cache_file() {
        let root = tempfile::tempdir().unwrap();
        let spy = SpyObserver::default();
        let ctx = ctx_with(&[], root.path().to_path_buf()).with_observer(Arc::new(spy.clone())); // empty allowlist
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let f = CountingFetcher {
            calls: calls.clone(),
            status: 200,
            body: "x",
        };
        let r = perform_http_get_cached(
            &ctx,
            "https://wttr.in/48183",
            1800,
            "2026-07-20T12:00:00-04:00",
            &f,
        );
        assert!(!r.ok);
        assert!(r.error.contains("not allowed"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "denied url must not hit the network"
        );
        // gate-first: no cache dir/file was created either
        assert!(!ctx.state_dir().join("__http_cache__").exists());
        assert_eq!(
            spy.records(),
            vec![(
                "weather".to_string(),
                DenialKind::Url,
                "https://wttr.in/48183".to_string()
            )]
        );
    }

    #[test]
    fn cached_first_fetch_populates_then_serves_within_ttl_without_refetch() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&["https://wttr.in/*"], root.path().to_path_buf(), 1_000_000);
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let f = CountingFetcher {
            calls: calls.clone(),
            status: 200,
            body: "sunny-72",
        };
        let url = "https://wttr.in/48183";
        // first call at T0: fetch + cache
        let r1 = perform_http_get_cached(&ctx, url, 1800, "2026-07-20T12:00:00-04:00", &f);
        assert!(r1.ok && !r1.stale);
        assert_eq!(r1.body, "sunny-72");
        // second call 10 min later: served from cache, NO new fetch
        let r2 = perform_http_get_cached(&ctx, url, 1800, "2026-07-20T12:10:00-04:00", &f);
        assert!(r2.ok && !r2.stale);
        assert_eq!(r2.body, "sunny-72");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "exactly one network call within the window"
        );
        assert_eq!(r2.age_secs, 600);
    }

    #[test]
    fn cached_expired_ttl_refetches() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&["https://wttr.in/*"], root.path().to_path_buf(), 1_000_000);
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let f = CountingFetcher {
            calls: calls.clone(),
            status: 200,
            body: "b",
        };
        let url = "https://wttr.in/48183";
        perform_http_get_cached(&ctx, url, 1800, "2026-07-20T12:00:00-04:00", &f);
        // 2h later -> expired -> refetch
        perform_http_get_cached(&ctx, url, 1800, "2026-07-20T14:00:00-04:00", &f);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cached_transport_failure_serves_stale_then_empty() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&["https://wttr.in/*"], root.path().to_path_buf(), 1_000_000);
        let url = "https://wttr.in/48183";
        // seed a good entry via a live fetch
        let live = FakeFetcher(200, "good-55");
        perform_http_get_cached(&ctx, url, 1800, "2026-07-20T09:00:00-04:00", &live);
        // 6h later fetch fails -> serve stale
        let r = perform_http_get_cached(&ctx, url, 1800, "2026-07-20T15:00:00-04:00", &DeadFetcher);
        assert!(r.ok && r.stale, "stale served on failure: {r:?}");
        assert_eq!(r.body, "good-55");
        assert!(r.age_secs > 0);
        // a *different* url that has never been cached -> not ok
        let miss = perform_http_get_cached(
            &ctx,
            "https://wttr.in/90210",
            1800,
            "2026-07-20T15:00:00-04:00",
            &DeadFetcher,
        );
        assert!(!miss.ok);
    }

    #[test]
    fn cached_non_2xx_does_not_overwrite_good_entry() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&["https://wttr.in/*"], root.path().to_path_buf(), 1_000_000);
        let url = "https://wttr.in/48183";
        perform_http_get_cached(
            &ctx,
            url,
            1800,
            "2026-07-20T09:00:00-04:00",
            &FakeFetcher(200, "good"),
        );
        // expired + a 500 response -> must NOT cache the error; serves the good stale body
        let r = perform_http_get_cached(
            &ctx,
            url,
            1,
            "2026-07-20T12:00:00-04:00",
            &FakeFetcher(500, "error-page"),
        );
        assert!(r.ok && r.stale);
        assert_eq!(r.body, "good");
    }

    #[test]
    fn cached_write_over_quota_still_returns_body() {
        let root = tempfile::tempdir().unwrap();
        // ctx_with sets max_state_bytes = 16; a 40-byte body can't be cached
        let ctx = ctx_with(&["https://wttr.in/*"], root.path().to_path_buf());
        let body = "0123456789012345678901234567890123456789"; // 40 bytes > 16
        let r = perform_http_get_cached(
            &ctx,
            "https://wttr.in/48183",
            1800,
            "2026-07-20T12:00:00-04:00",
            &FakeFetcher(200, body),
        );
        assert!(r.ok, "fetched body is returned even if it can't be cached");
        assert_eq!(r.body, body);
        // nothing persisted -> a second call refetches (cache miss)
        assert!(
            crate::cache::read_entry(&crate::cache::cache_path(
                &ctx.state_dir(),
                crate::cache::HTTP_NAMESPACE,
                "https://wttr.in/48183"
            ))
            .is_none()
        );
    }

    /// A quota failure can still evict — `write_entry` empties a cache
    /// namespace looking for room before giving up (see `cache::write_entry`)
    /// — so the directory can shrink even though the write it was trying to
    /// make room for ultimately fails. If the memo isn't invalidated on that
    /// failure it keeps reporting the old, now too-high size, which would
    /// wedge every later write that the (now smaller) directory could
    /// actually satisfy — the "stale-high memo" failure mode invariant N3
    /// warns against.
    #[test]
    fn failed_cache_write_invalidates_the_memo_rather_than_trusting_a_stale_value() {
        let root = tempfile::tempdir().unwrap();
        let cap = 50;
        let ctx = ctx_with_cap(&["https://x/*"], root.path().to_path_buf(), cap);
        let ns = ctx.state_dir().join(crate::cache::HTTP_NAMESPACE);
        std::fs::create_dir_all(&ns).unwrap();
        for i in 0..5 {
            std::fs::write(ns.join(format!("{i}.json")), "z".repeat(100)).unwrap();
        }
        // Seed the memo to the true, already-over-cap size.
        let seeded = ctx.state_size();
        assert!(
            seeded > cap,
            "the pre-populated namespace must already exceed the tiny cap"
        );

        // A response so large that even evicting the *entire* namespace
        // can't make it fit: the write fails, but eviction empties the
        // namespace on the way there, so the true total drops to ~0.
        let body = "x".repeat(500);
        let fetcher = ScriptedFetcher::ok(200, &body);
        let r = perform_http_get_cached(&ctx, "https://x/y", 60, NOW, &fetcher);
        assert!(
            r.ok,
            "the live-fetched body is still returned even though caching failed"
        );

        assert_eq!(
            ctx.state_size(),
            crate::state::dir_size(&ctx.state_dir()),
            "a failed write must invalidate the memo so the next read re-walks \
             to the truth, rather than trusting the stale pre-write size"
        );
    }

    /// A [`Fetcher`] fake that can be scripted to fail, unlike
    /// [`CountingFetcher`] (which always succeeds) — the negative-cache
    /// backoff tests below need to assert exactly how many times a *failing*
    /// fetch was attempted.
    struct ScriptedFetcher {
        calls: AtomicUsize,
        reply: Result<(u16, String), String>,
    }

    impl ScriptedFetcher {
        fn ok(status: u16, body: &str) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                reply: Ok((status, body.to_string())),
            }
        }

        fn err(message: &str) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                reply: Err(message.to_string()),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl crate::fetch::Fetcher for ScriptedFetcher {
        fn get(&self, _url: &str) -> Result<(u16, String), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.reply.clone()
        }
    }

    #[test]
    fn a_failing_refresh_is_not_retried_within_the_backoff_window() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&["https://x/*"], root.path().to_path_buf(), 1_000_000);
        let url = "https://x/y";

        // Seed a good entry at T0.
        let ok = ScriptedFetcher::ok(200, "body");
        let r = perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:00:00Z", &ok);
        assert!(r.ok && !r.stale);
        assert_eq!(ok.calls(), 1);

        // T0+120s: TTL lapsed, upstream now down -> one attempt, stale served.
        let down = ScriptedFetcher::err("connection refused");
        let r = perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:02:00Z", &down);
        assert!(r.ok && r.stale, "last-good entry is served");
        assert_eq!(down.calls(), 1);

        // T0+130s: still inside the backoff window -> NO new attempt.
        let r = perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:02:10Z", &down);
        assert!(r.ok && r.stale, "still served stale");
        assert_eq!(down.calls(), 1, "no second fetch inside the backoff window");
        // The entry served is still the last-good response, byte-for-byte:
        // only `last_attempt_at` may have moved. A failure arm that
        // corrupted `body`/`status` (or reported the wrong `age_secs`)
        // would still satisfy the assertions above.
        assert_eq!(r.body, "body");
        assert_eq!(r.status, 200);
        assert_eq!(r.age_secs, 130);
    }

    #[test]
    fn the_backoff_window_lapses_and_allows_another_attempt() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&["https://x/*"], root.path().to_path_buf(), 1_000_000);
        let url = "https://x/y";
        let ok = ScriptedFetcher::ok(200, "body");
        perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:00:00Z", &ok);

        let down = ScriptedFetcher::err("connection refused");
        perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:02:00Z", &down);
        assert_eq!(down.calls(), 1);
        // one full TTL after the failed attempt
        perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:03:01Z", &down);
        assert_eq!(down.calls(), 2, "backoff lapsed, retry allowed");
    }

    #[test]
    fn a_successful_refresh_clears_the_backoff() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&["https://x/*"], root.path().to_path_buf(), 1_000_000);
        let url = "https://x/y";
        let ok = ScriptedFetcher::ok(200, "one");
        perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:00:00Z", &ok);
        let ok2 = ScriptedFetcher::ok(200, "two");
        let r = perform_http_get_cached(&ctx, url, 60, "2026-07-26T12:02:00Z", &ok2);
        assert_eq!(r.body, "two");
        assert!(!r.stale);
    }

    #[test]
    fn a_failing_refresh_with_no_prior_entry_still_reports_failure() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx_with_cap(&["https://x/*"], root.path().to_path_buf(), 1_000_000);
        let down = ScriptedFetcher::err("connection refused");
        let r = perform_http_get_cached(&ctx, "https://x/y", 60, "2026-07-26T12:00:00Z", &down);
        assert!(!r.ok, "nothing to serve stale");
    }

    /// Fixed instants for the exec-cache TTL tests below, mirroring the
    /// literal RFC3339 strings the HTTP-cache tests above use directly:
    /// `LATER` is two hours after `NOW`, comfortably past every TTL those
    /// tests exercise (60s and 3600s).
    const NOW: &str = "2026-07-20T12:00:00-04:00";
    const LATER: &str = "2026-07-20T14:00:00-04:00";

    /// Records every run it is asked to perform, so a test can assert a
    /// denied call never reached the runner at all.
    struct RecordingRunner {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        reply: Result<(i32, String, String), String>,
    }

    impl RecordingRunner {
        fn ok(status: i32, stdout: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                reply: Ok((status, stdout.to_string(), String::new())),
            }
        }
        fn failing(message: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                reply: Err(message.to_string()),
            }
        }
        fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Runner for RecordingRunner {
        fn run(&self, program: &str, args: &[String]) -> Result<(i32, String, String), String> {
            self.calls
                .lock()
                .unwrap()
                .push((program.to_string(), args.to_vec()));
            self.reply.clone()
        }
    }

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// The command-allowlist counterpart to `ctx_with`/`ctx_with_cap` above:
    /// a `CapabilityCtx` whose `allowed_commands` is `patterns`, paired with a
    /// [`SpyObserver`] so a denied-case test can assert the denial fired.
    fn ctx_with_commands(patterns: &[&str]) -> (CapabilityCtx, SpyObserver) {
        ctx_with_commands_in(patterns, &std::env::temp_dir())
    }

    fn ctx_with_commands_in(
        patterns: &[&str],
        root: &std::path::Path,
    ) -> (CapabilityCtx, SpyObserver) {
        let pc = PluginConfig {
            allowed_commands: patterns.iter().map(|s| s.to_string()).collect(),
            ..PluginConfig::default()
        };
        let spy = SpyObserver::default();
        let ctx = CapabilityCtx::from_config("p", &pc, root.to_path_buf())
            .with_observer(Arc::new(spy.clone()));
        (ctx, spy)
    }

    #[test]
    fn exec_denied_never_reaches_the_runner_and_records_the_denial() {
        let (ctx, observer) = ctx_with_commands(&[]);
        let runner = RecordingRunner::ok(0, "should not happen");
        let out = perform_exec(&ctx, "playerctl", &argv(&["metadata"]), &runner);

        assert!(!out.ok);
        assert!(out.error.contains("playerctl metadata"), "{}", out.error);
        assert!(
            runner.calls().is_empty(),
            "gate-first: no spawn on a denied argv"
        );
        assert_eq!(
            observer.records(),
            vec![(
                "p".to_string(),
                DenialKind::Command,
                "playerctl metadata".to_string()
            )]
        );
    }

    #[test]
    fn exec_gates_on_the_whole_argv_not_just_the_program() {
        let (ctx, _o) = ctx_with_commands(&["git status*"]);
        let runner = RecordingRunner::ok(0, "");
        // Same program, different subcommand -> denied.
        let out = perform_exec(&ctx, "git", &argv(&["push", "--force"]), &runner);
        assert!(
            !out.ok,
            "a grant for `git status*` must not cover `git push`"
        );
        assert!(runner.calls().is_empty());

        let out = perform_exec(&ctx, "git", &argv(&["status", "--porcelain"]), &runner);
        assert!(out.ok);
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn exec_allowed_returns_status_and_streams_verbatim() {
        let (ctx, _o) = ctx_with_commands(&["echo*"]);
        let runner = RecordingRunner::ok(0, "hello\n");
        let out = perform_exec(&ctx, "echo", &argv(&["hello"]), &runner);
        assert!(out.ok);
        assert_eq!(out.status, 0);
        assert_eq!(out.stdout, "hello\n");
        assert!(out.error.is_empty());
        // The spawn got the program and args untouched -- no shell in between.
        assert_eq!(runner.calls(), vec![("echo".to_string(), argv(&["hello"]))]);
    }

    #[test]
    fn exec_a_nonzero_exit_is_data_not_an_error() {
        let (ctx, _o) = ctx_with_commands(&["false*"]);
        let runner = RecordingRunner::ok(1, "");
        let out = perform_exec(&ctx, "false", &[], &runner);
        assert!(out.ok, "the process ran; `ok` is not about its exit code");
        assert_eq!(out.status, 1);
    }

    #[test]
    fn exec_a_spawn_failure_is_reported_without_panicking() {
        let (ctx, _o) = ctx_with_commands(&["missing*"]);
        let runner = RecordingRunner::failing("no such file");
        let out = perform_exec(&ctx, "missing", &[], &runner);
        assert!(!out.ok);
        assert_eq!(out.status, -1);
        assert!(out.error.contains("no such file"));
    }

    #[test]
    fn exec_cached_denied_touches_neither_the_runner_nor_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _o) = ctx_with_commands_in(&[], dir.path());
        let runner = RecordingRunner::ok(0, "x");
        let out = perform_exec_cached(&ctx, "date", &[], 60, NOW, &runner);
        assert!(!out.ok);
        assert!(runner.calls().is_empty());
        assert!(
            !ctx.state_dir().join(EXEC_NAMESPACE).exists(),
            "a denied call must not create the cache dir"
        );
    }

    #[test]
    fn exec_cached_serves_a_fresh_entry_without_rerunning() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _o) = ctx_with_commands_in(&["date*"], dir.path());
        let runner = RecordingRunner::ok(0, "first");
        let a = perform_exec_cached(&ctx, "date", &[], 3600, NOW, &runner);
        assert_eq!(a.stdout, "first");
        assert!(!a.stale);

        let runner2 = RecordingRunner::ok(0, "second");
        let b = perform_exec_cached(&ctx, "date", &[], 3600, NOW, &runner2);
        assert_eq!(b.stdout, "first", "served from cache");
        assert!(runner2.calls().is_empty(), "fresh hit must not re-run");
    }

    #[test]
    fn exec_cached_reruns_once_the_ttl_has_expired() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _o) = ctx_with_commands_in(&["date*"], dir.path());
        perform_exec_cached(&ctx, "date", &[], 60, NOW, &RecordingRunner::ok(0, "first"));

        let runner = RecordingRunner::ok(0, "second");
        let out = perform_exec_cached(&ctx, "date", &[], 60, LATER, &runner);
        assert_eq!(out.stdout, "second");
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn exec_cached_does_not_cache_a_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _o) = ctx_with_commands_in(&["flaky*"], dir.path());
        perform_exec_cached(
            &ctx,
            "flaky",
            &[],
            3600,
            NOW,
            &RecordingRunner::ok(3, "bad"),
        );

        // Nothing was cached, so the next call runs again rather than serving `bad`.
        let runner = RecordingRunner::ok(0, "good");
        let out = perform_exec_cached(&ctx, "flaky", &[], 3600, NOW, &runner);
        assert_eq!(out.stdout, "good");
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn exec_cached_a_nonzero_refresh_does_not_serve_the_stale_good_entry() {
        // Unlike a spawn failure/timeout, a non-zero exit is fresh, real data
        // — it must be returned as-is, not swapped for a stale prior entry.
        // `exec_cached_does_not_cache_a_nonzero_exit` above never seeded a
        // prior entry, so it can't distinguish this from the opposite
        // (HTTP-non-2xx-style) interpretation; this test seeds a good entry
        // first so the two interpretations actually diverge.
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _o) = ctx_with_commands_in(&["git*"], dir.path());
        perform_exec_cached(&ctx, "git", &[], 60, NOW, &RecordingRunner::ok(0, "main"));
        let out = perform_exec_cached(
            &ctx,
            "git",
            &[],
            60,
            LATER,
            &RecordingRunner::ok(128, "fatal: not a repo"),
        );
        assert!(
            !out.stale,
            "a non-zero exit is fresh data, not a stale fallback"
        );
        assert_eq!(out.status, 128);
    }

    #[test]
    fn exec_a_failing_refresh_is_not_retried_within_the_backoff_window() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _o) = ctx_with_commands_in(&["date*"], dir.path());

        // Seed a good entry at T0.
        let ok = RecordingRunner::ok(0, "good");
        let r = perform_exec_cached(&ctx, "date", &[], 60, NOW, &ok);
        assert!(r.ok && !r.stale);
        assert_eq!(ok.calls().len(), 1);

        // T0+120s: TTL lapsed, spawn now fails -> one attempt, stale served.
        let down = RecordingRunner::failing("boom");
        let r = perform_exec_cached(&ctx, "date", &[], 60, "2026-07-20T12:02:00-04:00", &down);
        assert!(r.ok && r.stale, "last-good entry is served");
        assert_eq!(down.calls().len(), 1);

        // T0+130s: still inside the backoff window -> NO new attempt.
        let r = perform_exec_cached(&ctx, "date", &[], 60, "2026-07-20T12:02:10-04:00", &down);
        assert!(r.ok && r.stale, "still served stale");
        assert_eq!(
            down.calls().len(),
            1,
            "no second run inside the backoff window"
        );
        // The entry served is still the last-good response, byte-for-byte:
        // only `last_attempt_at` may have moved.
        assert_eq!(r.stdout, "good");
        assert_eq!(r.age_secs, 130);
    }

    #[test]
    fn exec_the_backoff_window_lapses_and_allows_another_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _o) = ctx_with_commands_in(&["date*"], dir.path());
        perform_exec_cached(&ctx, "date", &[], 60, NOW, &RecordingRunner::ok(0, "good"));

        let down = RecordingRunner::failing("boom");
        perform_exec_cached(&ctx, "date", &[], 60, "2026-07-20T12:02:00-04:00", &down);
        assert_eq!(down.calls().len(), 1);
        // one full TTL after the failed attempt
        perform_exec_cached(&ctx, "date", &[], 60, "2026-07-20T12:03:01-04:00", &down);
        assert_eq!(down.calls().len(), 2, "backoff lapsed, retry allowed");
    }

    #[test]
    fn exec_cached_serves_the_last_good_entry_stale_when_a_refresh_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _o) = ctx_with_commands_in(&["date*"], dir.path());
        perform_exec_cached(&ctx, "date", &[], 60, NOW, &RecordingRunner::ok(0, "good"));

        let out = perform_exec_cached(
            &ctx,
            "date",
            &[],
            60,
            LATER,
            &RecordingRunner::failing("boom"),
        );
        assert!(out.ok, "a usable (stale) body is present");
        assert!(out.stale);
        assert_eq!(out.stdout, "good");
        assert!(out.age_secs > 0);
    }

    #[test]
    fn exec_and_http_caches_never_collide_on_the_same_key_string() {
        let dir = tempfile::tempdir().unwrap();
        let http = crate::cache::cache_path(dir.path(), HTTP_NAMESPACE, "same-key");
        let exec = crate::cache::cache_path(dir.path(), EXEC_NAMESPACE, "same-key");
        assert_ne!(http, exec);
    }
}

/// `perform_log` has no `CapabilityCtx` to pass through a fake `Fetcher`, so
/// these tests capture real `tracing` events with a minimal hand-rolled
/// `Subscriber` (no test-only dep needed — `Subscriber`/`Visit` are already
/// part of the `tracing` crate) rather than reusing the fixtures above.
#[cfg(test)]
mod log_tests {
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use super::{MAX_GUEST_LOG_BYTES, MAX_GUEST_LOG_CALLS, TRUNCATION_MARKER, perform_log};
    use crate::capability::CapabilityCtx;
    use rustline_core::PluginConfig;

    /// One captured event: its severity plus every field (incl. the implicit
    /// `message` field carrying the formatted log text) as `(name, debug)`.
    type CapturedEvent = (Level, Vec<(String, String)>);

    #[derive(Default)]
    struct FieldVisitor(Vec<(String, String)>);

    impl Visit for FieldVisitor {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0
                .push((field.name().to_string(), format!("{value:?}")));
        }
    }

    /// A subscriber that accepts every event (no level filtering) and
    /// records it, purely so a test can assert what `perform_log` emitted.
    struct RecordingSubscriber(Arc<Mutex<Vec<CapturedEvent>>>);

    impl Subscriber for RecordingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut visitor = FieldVisitor::default();
            event.record(&mut visitor);
            self.0
                .lock()
                .unwrap()
                .push((*event.metadata().level(), visitor.0));
        }

        fn enter(&self, _span: &Id) {}
        fn exit(&self, _span: &Id) {}
    }

    /// Run `f` under a scoped recording subscriber and return every event it
    /// emitted, in order. Reads the shared buffer back out through the
    /// `Mutex` rather than `Arc::try_unwrap`-ing it: `tracing`'s dispatcher
    /// machinery can still be holding its own clone of the `Arc` briefly
    /// after `with_default` returns, so asserting a unique strong count here
    /// is not reliable.
    fn capture(f: impl FnOnce()) -> Vec<CapturedEvent> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = RecordingSubscriber(events.clone());
        tracing::subscriber::with_default(subscriber, f);
        events.lock().unwrap().clone()
    }

    fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
        fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Run `f` under the recording subscriber and return every emitted
    /// event's `message` field, newline-joined, so the size/rate tests below
    /// can assert on truncation markers and rate-limit text with plain
    /// string ops instead of walking `CapturedEvent`s by hand.
    fn with_recording_subscriber(f: impl FnOnce()) -> String {
        capture(f)
            .iter()
            .filter_map(|(_, fields)| field(fields, "message"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn test_ctx(name: &str, dir: &std::path::Path) -> CapabilityCtx {
        CapabilityCtx::from_config(name, &PluginConfig::default(), dir.to_path_buf())
    }

    #[test]
    fn maps_each_known_level_string_to_the_matching_tracing_level() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx("weather", dir.path());
        for (level_str, expected) in [
            ("error", Level::ERROR),
            ("warn", Level::WARN),
            ("info", Level::INFO),
            ("debug", Level::DEBUG),
            ("trace", Level::TRACE),
        ] {
            let events = capture(|| perform_log(&ctx, level_str, "hello"));
            assert_eq!(events.len(), 1, "level {level_str}");
            let (level, fields) = &events[0];
            assert_eq!(*level, expected, "level {level_str}");
            assert_eq!(field(fields, "message"), Some("hello"));
            assert_eq!(field(fields, "plugin"), Some("weather"));
        }
    }

    #[test]
    fn unrecognized_level_degrades_to_info_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx("weather", dir.path());
        let events = capture(|| perform_log(&ctx, "bogus", "still logged"));
        assert_eq!(events.len(), 1);
        let (level, fields) = &events[0];
        assert_eq!(*level, Level::INFO);
        assert_eq!(field(fields, "message"), Some("still logged"));
        assert_eq!(field(fields, "orig_level"), Some("bogus"));
    }

    #[test]
    fn every_level_string_logs_with_no_network_or_filesystem_effect() {
        // N1: rl_log is the one intentional capability-free host fn. There is
        // no `CapabilityCtx` allowlist here to deny against, so — unlike the
        // other six `perform_*` functions — there is no "denied" case to
        // test; this just pins that every level (incl. an unknown one)
        // completes with no side effect beyond the emitted tracing event.
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx("no-capability-needed", dir.path());
        for level in ["error", "warn", "info", "debug", "trace", "unknown"] {
            let events = capture(|| perform_log(&ctx, level, "logged"));
            assert_eq!(events.len(), 1, "level {level}");
        }
    }

    /// No test in this module gives `perform_log` a real temp dir:
    /// `rl_log` is capability-free (N1) and never touches the filesystem, so
    /// a `tempfile::tempdir()` here would imply a capability that must not
    /// exist. `std::path::PathBuf::from("/tmp")` is a placeholder
    /// `state_root` that is never read, matching `capability.rs`'s own
    /// capability tests.
    fn unused_state_root() -> std::path::PathBuf {
        std::path::PathBuf::from("/tmp")
    }

    #[test]
    fn an_oversized_message_is_truncated() {
        let ctx = CapabilityCtx::from_config("p", &PluginConfig::default(), unused_state_root());
        let captured = with_recording_subscriber(|| {
            perform_log(&ctx, "info", &"x".repeat(10_000));
        });
        assert!(
            captured.len() <= MAX_GUEST_LOG_BYTES,
            "message was truncated to the documented cap"
        );
        assert!(captured.contains(TRUNCATION_MARKER), "truncation is marked");
    }

    #[test]
    fn truncation_never_splits_a_multibyte_char() {
        let ctx = CapabilityCtx::from_config("p", &PluginConfig::default(), unused_state_root());
        // 3-byte chars straddling the byte cap: a naive &msg[..CAP] panics.
        let msg = "€".repeat(MAX_GUEST_LOG_BYTES);
        let captured = with_recording_subscriber(|| {
            perform_log(&ctx, "info", &msg); // must not panic
        });
        assert!(captured.contains(TRUNCATION_MARKER));
    }

    #[test]
    fn the_call_rate_is_capped_and_reported_once() {
        let ctx = CapabilityCtx::from_config("p", &PluginConfig::default(), unused_state_root());
        let captured = with_recording_subscriber(|| {
            for i in 0..(MAX_GUEST_LOG_CALLS + 50) {
                perform_log(&ctx, "info", &format!("line {i}"));
            }
        });
        assert!(
            captured.contains(&format!("line {}", MAX_GUEST_LOG_CALLS - 1)),
            "the last in-budget call is forwarded"
        );
        assert!(!captured.contains(&format!("line {}", MAX_GUEST_LOG_CALLS + 10)));
        assert_eq!(
            captured.matches("guest log rate limit reached").count(),
            1,
            "the limit is reported exactly once"
        );
    }

    #[test]
    fn the_budget_is_per_plugin_instance() {
        let a = CapabilityCtx::from_config("a", &PluginConfig::default(), unused_state_root());
        let b = CapabilityCtx::from_config("b", &PluginConfig::default(), unused_state_root());
        let captured = with_recording_subscriber(|| {
            for _ in 0..MAX_GUEST_LOG_CALLS {
                perform_log(&a, "info", "from a");
            }
            perform_log(&b, "info", "from b"); // b has its own budget (N4)
        });
        assert!(captured.contains("from b"));
    }
}
