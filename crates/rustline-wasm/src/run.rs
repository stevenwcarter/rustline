//! Subprocess execution for the exec capability.
//!
//! `Runner` is the seam (mirroring `fetch::Fetcher`) that lets every gate test
//! in `perform.rs` run without spawning anything: the capability decision is
//! made before `run` is ever called, and a recording fake proves it.
//!
//! `ProcessRunner` is the only production implementation. It spawns the
//! program **directly — there is no shell anywhere in this path**, so nothing
//! re-parses the arguments and there is no quoting or word-splitting surface.
//! A guest that wants a shell must be granted one explicitly and visibly
//! (`allowed_commands = ["sh -c *"]`); the host never introduces one.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Wall-clock bound on waiting for the immediate child to exit. Deliberately
/// under Extism's 10 s plugin timeout so a hung command surfaces to the
/// guest as a renderable failure result rather than killing the whole
/// plugin render (invariant N2). This is not quite the bound on the whole
/// `run` call, though: collecting output after the wait can add up to two
/// more [`OUTPUT_GRACE`] periods (a few hundred ms) on top — see its doc for
/// why that piece can't be folded into this same deadline.
pub const EXEC_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-stream output cap. Beyond this the stream is truncated and
/// `perform_exec`/`perform_exec_cached` flag the result `truncated`.
///
/// Those two functions detect truncation with `stdout.len() >=
/// MAX_OUTPUT_BYTES` on the `String` this module hands back — which has
/// already gone through [`read_capped`]'s lossy UTF-8 conversion, so its byte
/// length isn't literally the raw byte count that was read. That comparison
/// is still safe (never a false negative) because `String::from_utf8_lossy`
/// can only ever grow a byte sequence, never shrink it: every valid byte
/// passes through unchanged, and Unicode's "maximal subpart" replacement rule
/// turns each 1–3-byte ill-formed run into a fixed 3-byte U+FFFD. So when
/// [`read_capped`] truly stopped at the `MAX_OUTPUT_BYTES` raw-byte cap, the
/// returned string's length is always `>= MAX_OUTPUT_BYTES`. The only
/// possible imprecision runs the other way — arbitrary binary output that
/// happens to be invalid UTF-8 right at the boundary can occasionally trip
/// the flag on a stream that wasn't actually capped — which is acceptable
/// since `truncated` is purely informational for a guest/user, never a gating
/// or security property.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Grace period for a reader thread to report in *after* [`kill_group`] has
/// fired. Bounded, not unbounded: `kill_group` only reaches processes still
/// in the child's own process group (see its doc), so a descendant that
/// escaped it via `setsid`/`setpgid` (a properly-daemonizing program, or a
/// plugin config that runs one — `emacs --daemon`, `ssh -f`, `tmux
/// new-session -d`, `sh -c "setsid long_thing &"`) keeps its inherited
/// stdout/stderr open indefinitely even after the kill, and its output can
/// no longer be collected. Waiting this long for a *reachable* reader
/// thread to notice its pipe closed costs nothing in the normal case (it
/// closes within microseconds of the kill); past it, `run` returns with
/// whatever each stream produced so far — empty for a stream whose writer
/// escaped, never a hang. This is what makes [`EXEC_TIMEOUT`] the whole
/// run's real bound: `EXEC_TIMEOUT` plus, at most, two of these grace
/// periods (one per stream).
const OUTPUT_GRACE: Duration = Duration::from_millis(250);

/// How the host runs a command. The `perform_exec*` gate decides *whether* to
/// call this; the implementation decides *how*.
pub trait Runner {
    /// Run `program` with `args`. `Ok((exit_code, stdout, stderr))` when the
    /// process ran to completion (whatever its exit code — a non-zero exit is
    /// data, not an error; killed-by-signal maps to `-1`); `Err(message)` on a
    /// spawn failure or a timeout.
    fn run(&self, program: &str, args: &[String]) -> Result<(i32, String, String), String>;
}

/// The production runner: direct spawn, no shell, piped stdout/stderr, stdin
/// closed, inherited environment and working directory, wall-clock bounded,
/// output capped.
pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<(i32, String, String), String> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            // A child that reads stdin gets EOF immediately instead of
            // blocking until the timeout.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Put the child in its own process group (pgid == its own pid) so a
        // descendant it backgrounds -- `sh -c "long_thing &"`, or any
        // double-forking daemonizer -- stays reachable as a unit. Without
        // this, `kill_group` below (killing just the immediate child's pid)
        // would leave such a descendant running and holding the piped
        // stdout/stderr open indefinitely.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn {program}: {e}"))?;

        // Drain both pipes on their own threads, started *before* the wait
        // loop below runs at all: a child that fills a pipe buffer would
        // otherwise block forever while we're stuck waiting for it to exit,
        // since we'd never be reading the other end to relieve the pressure.
        // Each thread reports its result over a channel rather than a
        // `JoinHandle`, so the deadline below can bound *receiving* that
        // result (`recv_timeout`) instead of having to join — and thus
        // unboundedly block on — the thread itself.
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let (out_tx, out_rx) = mpsc::channel();
        let (err_tx, err_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = out_tx.send(read_capped(stdout_pipe));
        });
        std::thread::spawn(move || {
            let _ = err_tx.send(read_capped(stderr_pipe));
        });

        // One deadline bounds both waiting for the immediate child to exit
        // AND the first pass at collecting its output, not just the wait
        // loop. Without covering the second half too, a child that
        // backgrounds a descendant sharing its piped stdout/stderr could
        // exit promptly (the wait loop below returns normally, no timeout)
        // while the descendant keeps the pipes open indefinitely, hanging
        // the output-collection step unboundedly -- which is exactly the
        // case `process_runner_does_not_hang_when_a_backgrounded_descendant_
        // outlives_the_immediate_child` below guards against. That first
        // pass alone isn't the whole story, though: see the `OUTPUT_GRACE`
        // comment below for the descendant that also escapes the kill this
        // deadline triggers.
        let deadline = Instant::now() + EXEC_TIMEOUT;

        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    kill_and_reap(&mut child);
                    return Err(format!(
                        "{program} exceeded the {}s exec timeout",
                        EXEC_TIMEOUT.as_secs()
                    ));
                }
                Err(e) => {
                    kill_and_reap(&mut child);
                    return Err(format!("failed to wait on {program}: {e}"));
                }
            }
        };

        // The immediate child has exited (and, since `try_wait` returned
        // `Some`, is already reaped -- no zombie). A backgrounded descendant
        // may still be holding the piped fds open, though, so collect both
        // streams bounded by the SAME deadline rather than joining
        // unboundedly. If either hasn't arrived by then, kill the whole
        // process group -- closing an ordinary (still-in-group) descendant's
        // inherited fds too.
        let out_first = out_rx.recv_timeout(remaining(deadline));
        let err_first = err_rx.recv_timeout(remaining(deadline));
        if out_first.is_err() || err_first.is_err() {
            kill_group(&mut child);
        }
        // That kill doesn't guarantee either reader thread is about to
        // report in, though: `kill_group` only reaches the child's own
        // process group (see its doc), so a descendant that escaped it
        // (`setsid`/`setpgid`, or a properly-daemonizing program) is left
        // running with the piped fds still open. A plain `recv()` here would
        // then block for as long as THAT process lives -- unbounded, and the
        // one place this module's "the whole run is bounded" guarantee used
        // to not actually hold. `OUTPUT_GRACE` caps the wait instead: a
        // reachable reader reports in almost immediately after the kill, and
        // an unreachable one's stream comes back empty rather than hanging
        // this thread (and, transitively, e.g. the daemon's shared render
        // lock) forever.
        let stdout =
            out_first.unwrap_or_else(|_| out_rx.recv_timeout(OUTPUT_GRACE).unwrap_or_default());
        let stderr =
            err_first.unwrap_or_else(|_| err_rx.recv_timeout(OUTPUT_GRACE).unwrap_or_default());

        // `status.code()` is `None` when the process was killed by a signal;
        // there's no exit code to report in that case, so map it to `-1`
        // (this is the only path that ever has a real `ExitStatus` — the
        // timeout/wait-failure paths above never reach here at all).
        Ok((status.code().unwrap_or(-1), stdout, stderr))
    }
}

/// Time remaining until `deadline`, floored at zero so a `recv_timeout` call
/// made after the deadline has already passed returns immediately (a
/// non-blocking check) instead of underflowing/blocking.
fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// Kill a still-running child (its whole process group — see [`kill_group`])
/// and reap it (`wait` after `kill`, so it never lingers as a zombie).
/// Best-effort: a `kill`/`wait` failure is ignored, since the caller is
/// already on its way to returning an error.
fn kill_and_reap(child: &mut Child) {
    kill_group(child);
    let _ = child.wait();
}

/// Kill every process in `child`'s process group.
///
/// On unix, `child` was spawned with `process_group(0)` (see `run` above),
/// which sets its pgid to its own pid — so `child.id()` doubles as the group
/// id. Signaling the *group* rather than just `child`'s own pid reaches a
/// descendant that inherited the piped stdout/stderr and is still holding
/// them open even though the immediate child has already exited (e.g. `sh -c
/// "long_thing &"`), which a plain `child.kill()` would not.
#[cfg(unix)]
fn kill_group(child: &mut Child) {
    // SAFETY: `child.id()` is the pid of a process this host process itself
    // spawned with `process_group(0)`, so it is also that process's own
    // process group id -- we have standing to signal it. `SIGKILL` cannot be
    // caught, blocked, or ignored, so this cannot deadlock or leave the
    // target in a partial state. `killpg` returning `ESRCH` (every process in
    // the group has already exited on its own) is a normal, ignorable
    // outcome; there is nothing actionable to do with any `killpg` error
    // here regardless, since the caller is already on the error/timeout
    // path.
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
}

/// Non-unix fallback: no process-group concept here, so this only ever
/// reaches the immediate child -- the same reach `child.kill()` had before
/// this hardening. Accepted as-is: rustline targets Linux and macOS (both
/// unix), where `kill_group` above applies instead.
#[cfg(not(unix))]
fn kill_group(child: &mut Child) {
    let _ = child.kill();
}

/// Read at most [`MAX_OUTPUT_BYTES`] from `pipe`, lossily as UTF-8 so binary
/// output degrades to replacement characters instead of failing the whole
/// run. `pipe` is `None` when the child had no such stream — doesn't happen
/// given `ProcessRunner` always requests `Stdio::piped()`, but keeps this
/// total rather than panicking.
fn read_capped<R: Read>(pipe: Option<R>) -> String {
    let Some(mut reader) = pipe else {
        return String::new();
    };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    while buf.len() < MAX_OUTPUT_BYTES {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    buf.truncate(MAX_OUTPUT_BYTES);
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    #[test]
    fn process_runner_propagates_a_zero_exit_and_stdout() {
        let (status, stdout, _stderr) = ProcessRunner
            .run("echo", &["hi".to_string()])
            .expect("echo runs");
        assert_eq!(status, 0);
        assert_eq!(stdout.trim_end(), "hi");
    }

    #[test]
    fn process_runner_propagates_a_nonzero_exit() {
        let (status, _out, _err) = ProcessRunner.run("false", &[]).expect("false runs");
        assert_ne!(status, 0);
    }

    #[test]
    fn process_runner_captures_stderr_separately() {
        let (_status, stdout, stderr) = ProcessRunner
            .run(
                "sh",
                &["-c".to_string(), "echo out; echo err >&2".to_string()],
            )
            .expect("sh runs");
        assert_eq!(stdout.trim_end(), "out");
        assert_eq!(stderr.trim_end(), "err");
    }

    #[test]
    fn process_runner_reports_a_missing_program_as_an_error_not_a_panic() {
        let err = ProcessRunner
            .run("definitely-not-a-real-program-xyz", &[])
            .unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn process_runner_kills_a_command_that_outlives_the_timeout() {
        let start = std::time::Instant::now();
        let out = ProcessRunner.run("sleep", &["30".to_string()]);
        assert!(out.is_err(), "a timed-out run is an error: {out:?}");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(15),
            "returned promptly rather than waiting out the sleep"
        );
    }

    #[test]
    fn process_runner_truncates_output_beyond_the_cap() {
        // 200 KiB of 'x' — well past MAX_OUTPUT_BYTES.
        let (_status, stdout, _stderr) = ProcessRunner
            .run(
                "sh",
                &["-c".to_string(), "yes x | head -c 204800".to_string()],
            )
            .expect("sh runs");
        assert!(stdout.len() <= MAX_OUTPUT_BYTES, "capped: {}", stdout.len());
    }

    #[test]
    fn process_runner_does_not_hang_when_a_backgrounded_descendant_outlives_the_immediate_child() {
        // The immediate child (`sh`) exits promptly with status 0 -- the
        // try_wait loop returns normally, no timeout fires there -- but it
        // backgrounds `sleep 30`, which inherits the piped stdout/stderr fds
        // and doesn't close them. Without killing the whole process group,
        // the two reader threads' joins block for as long as `sleep` lives
        // (comfortably longer than any patience a test should have).
        let start = std::time::Instant::now();
        let out = ProcessRunner.run("sh", &["-c".to_string(), "sleep 30 & exit 0".to_string()]);
        assert!(out.is_ok(), "the immediate child exited 0: {out:?}");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(15),
            "returned promptly rather than waiting out the backgrounded descendant \
             (elapsed: {:?})",
            start.elapsed()
        );
    }

    #[test]
    fn process_runner_does_not_hang_forever_when_a_descendant_escapes_the_process_group() {
        // `setsid` gives the backgrounded `sleep` its own session AND its own
        // process group (pgid == its own pid), stepping outside the
        // immediate child's group entirely -- so `kill_group`'s
        // `killpg(child.id(), ...)` cannot reach it (see `kill_group`'s
        // doc). Unlike the sibling
        // `..._backgrounded_descendant_outlives_the_immediate_child` test
        // above, this command does NOT redirect the descendant's
        // stdout/stderr away: it inherits the piped fds `sh` itself was
        // given and keeps them open for as long as it runs -- exactly the
        // I1 scenario ("It keeps the inherited stdout/stderr open, so those
        // fallback recv() calls block forever"). `sleep 12` deliberately
        // outlives `EXEC_TIMEOUT` (5s): before the I1 fix, `run` blocks on
        // an unbounded `recv()` until this process exits on its own (~12s);
        // after the fix, `run` returns within `EXEC_TIMEOUT` plus a short
        // output grace period (~5.5s) regardless. The escaped `sleep`
        // process cannot be killed by this test either way (that is the
        // bug/limitation being pinned) and is left to exit on its own after
        // ~12s -- short enough not to linger meaningfully across test runs.
        let start = std::time::Instant::now();
        let out = ProcessRunner.run(
            "sh",
            &["-c".to_string(), "setsid sleep 12 & exit 0".to_string()],
        );
        assert!(out.is_ok(), "the immediate child exited 0: {out:?}");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(8),
            "returned within EXEC_TIMEOUT + a bounded output grace period, \
             not after waiting out the escaped descendant's full lifetime \
             (elapsed: {:?})",
            start.elapsed()
        );
    }

    #[test]
    fn process_runner_gives_a_stdin_reader_eof_rather_than_hanging() {
        let start = std::time::Instant::now();
        let (_status, stdout, _stderr) = ProcessRunner
            .run("sh", &["-c".to_string(), "cat".to_string()])
            .expect("cat runs");
        assert!(stdout.is_empty());
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }
}
