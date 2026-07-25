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
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Wall-clock bound on one child process. Deliberately under Extism's 10 s
/// plugin timeout so a hung command surfaces to the guest as a renderable
/// failure result rather than killing the whole plugin render (invariant N2).
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
        let mut child = Command::new(program)
            .args(args)
            // A child that reads stdin gets EOF immediately instead of
            // blocking until the timeout.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn {program}: {e}"))?;

        // Drain both pipes on their own threads, started *before* the wait
        // loop below runs at all: a child that fills a pipe buffer would
        // otherwise block forever while we're stuck waiting for it to exit,
        // since we'd never be reading the other end to relieve the pressure.
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let out_handle = std::thread::spawn(move || read_capped(stdout_pipe));
        let err_handle = std::thread::spawn(move || read_capped(stderr_pipe));

        let deadline = Instant::now() + EXEC_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    kill_and_reap(&mut child, out_handle, err_handle);
                    return Err(format!(
                        "{program} exceeded the {}s exec timeout",
                        EXEC_TIMEOUT.as_secs()
                    ));
                }
                Err(e) => {
                    kill_and_reap(&mut child, out_handle, err_handle);
                    return Err(format!("failed to wait on {program}: {e}"));
                }
            }
        };

        let stdout = out_handle.join().unwrap_or_default();
        let stderr = err_handle.join().unwrap_or_default();
        // `status.code()` is `None` when the process was killed by a signal;
        // there's no exit code to report in that case, so map it to `-1`
        // (this is the only path that ever has a real `ExitStatus` — the
        // timeout/wait-failure paths above never reach here at all).
        Ok((status.code().unwrap_or(-1), stdout, stderr))
    }
}

/// Kill a still-running child, reap it (`wait` after `kill`, so it never
/// lingers as a zombie), and join its two reader threads. Once `wait`
/// confirms the child has actually exited, the kernel has closed its pipe
/// file descriptors, so both readers see EOF and return promptly — nothing is
/// leaked. Best-effort: a `kill`/`wait` failure or a poisoned reader thread is
/// ignored, since the caller is already on its way to returning an error.
fn kill_and_reap(child: &mut Child, out: JoinHandle<String>, err: JoinHandle<String>) {
    let _ = child.kill();
    let _ = child.wait();
    let _ = out.join();
    let _ = err.join();
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
    fn process_runner_gives_a_stdin_reader_eof_rather_than_hanging() {
        let start = std::time::Instant::now();
        let (_status, stdout, _stderr) = ProcessRunner
            .run("sh", &["-c".to_string(), "cat".to_string()])
            .expect("cat runs");
        assert!(stdout.is_empty());
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }
}
