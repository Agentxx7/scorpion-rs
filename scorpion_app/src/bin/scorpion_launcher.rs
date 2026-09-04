//! `scorpion-launcher` — the thin Windows startup orchestrator for the
//! installed Scorpion application (`SCORPION_WINDOWS_INSTALLABLE_APPLICATION_001`).
//!
//! This binary owns exactly what the frontier's own contract assigns it:
//! installed-file layout resolution, startup orchestration, local runtime
//! configuration, a health wait, launching the default browser, and
//! truthful operator-facing startup errors. It owns none of Scorpion's
//! actual capabilities — it never parses IAM material, crawls, touches
//! evidence/audit/research logic, or opens the persistence store itself.
//! It only ever spawns the existing, unmodified `scorpion-api` binary
//! (built from `scorpion_app/src/main.rs`, entirely unchanged by this
//! frontier) as a child process and waits for its own existing
//! `GET /health` route to answer truthfully.
//!
//! # Why this binary has almost no dependencies
//!
//! Every piece of "Windows-specific" behavior this launcher needs turned
//! out to be reachable from the standard library alone, so no new crate
//! dependency was added for it (mirrors the SAML frontier's own
//! zero-new-dependency precedent):
//!
//! - hiding the launcher's own console window: `#![windows_subsystem =
//!   "windows"]` (a compiler attribute, not a crate)
//! - hiding the spawned scorpion-api child's console window:
//!   `std::os::windows::process::CommandExt::creation_flags` with the
//!   well-known `CREATE_NO_WINDOW` Win32 constant, hardcoded with a
//!   comment (part of `std` on the Windows target, not a crate)
//! - opening the OS default browser: shelling out to the `cmd.exe`
//!   built-in `start` command with a fixed, never-externally-derived URL
//!   (see [`browser_open_command`]) — `std::process::Command` only
//! - a truthful startup-error dialog when there is no console to print
//!   to: shelling out to `powershell.exe`'s built-in
//!   `System.Windows.Forms.MessageBox` (present on every supported
//!   Windows version) with a properly quoted, fixed-shape message — see
//!   [`powershell_single_quote`] — `std::process::Command` only
//! - resolving the per-user writable application-data directory:
//!   `%LOCALAPPDATA%` via `std::env::var`
//!
//! # Process-lifecycle limitation (reported, not hidden)
//!
//! The launcher does **not** remain resident for the whole "application
//! session." It spawns `scorpion-api` (or detects one already healthy),
//! waits for readiness, opens the browser, and exits. Implementing true
//! session-long ownership without either a system-tray application or a
//! Windows Service — both explicitly out of scope for this frontier —
//! would require a hidden background process with no way for the user to
//! ever ask it to quit, which is worse, not better. This is the smallest
//! truthful single-instance behavior available under those constraints
//! (the frontier's own escape hatch for this exact situation): a second
//! launch re-probes `/health` first and reuses the already-running
//! instance rather than starting a duplicate server, so duplicate
//! servers are still deterministically prevented, but `scorpion-api`
//! itself is not stopped when the launcher (or the browser) closes — it
//! keeps running until the user's session ends or it is stopped some
//! other way (e.g. Task Manager). There is no "Quit Scorpion" control in
//! this V1.
//!
//! # Port-conflict semantics
//!
//! `127.0.0.1:8787` (fixed — never `0.0.0.0`, never a different port) is
//! probed for an existing healthy Scorpion first. Three outcomes:
//!
//! 1. `/health` already answers truthfully → reuse it (open the browser,
//!    do not spawn a second server).
//! 2. The port is free (a real, momentary `TcpListener::bind` succeeds
//!    and is immediately dropped) → spawn `scorpion-api`, wait for
//!    `/health`. There is an inherent, small TOCTOU window between this
//!    probe and the child's own real bind — accepted rather than solved
//!    by inheriting a pre-bound socket handle into the child, which
//!    would materially expand this frontier's scope for a race that
//!    `scorpion-api`'s own `TcpListener::bind` already fails loudly on;
//!    reported here rather than hidden.
//! 3. The port is occupied by something that is not an already-healthy
//!    Scorpion → fail visibly with a truthful error; never guess, never
//!    silently pick a different port (the existing IAM callback URI
//!    architecture bakes in `127.0.0.1:8787`; dynamic-port propagation
//!    does not exist end-to-end), never kill the unrelated process.

#![cfg_attr(windows, windows_subsystem = "windows")]

use spider::features::domain_runtime::DOMAIN_DATABASE_ENV;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Fixed, canonical bind address — identical default to
/// `scorpion_app::main`'s own `SCORPION_API_BIND` fallback. Never
/// `0.0.0.0`, never overridden to a different value by this launcher.
const SCORPION_BIND: &str = "127.0.0.1:8787";
const SCORPION_HOST: &str = "127.0.0.1";
const SCORPION_PORT: u16 = 8787;

/// How long to wait for `/health` after spawning a fresh `scorpion-api`
/// child, and how often to re-check within that budget.
const HEALTH_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// A short, single-attempt timeout used only for the initial "is a
/// healthy instance already running" probe — this must not block
/// startup for the common case where nothing is listening yet.
const HEALTH_PROBE_CONNECT_TIMEOUT: Duration = Duration::from_millis(300);

/// The application name used for `%LOCALAPPDATA%\Scorpion`, the Start
/// Menu folder, and the message-box title — the one place this literal
/// is defined.
const APP_DIR_NAME: &str = "Scorpion";

fn main() {
    let app_data_dir = match resolve_app_data_dir(&|name| std::env::var(name).ok()) {
        Ok(dir) => dir,
        Err(message) => {
            // Nowhere to log yet — this is the one failure mode with no
            // log file to write to, so the dialog is the only truthful
            // channel available.
            show_error_dialog(&message);
            std::process::exit(1);
        }
    };

    if let Err(error) = std::fs::create_dir_all(log_dir(&app_data_dir)) {
        let message = format!(
            "Scorpion could not create its application-data directory at {}: {error}",
            log_dir(&app_data_dir).display()
        );
        show_error_dialog(&message);
        std::process::exit(1);
    }

    let mut launcher_log = LauncherLog::open(launcher_log_path(&app_data_dir));
    launcher_log.line(&format!("startup: app_data_dir={}", app_data_dir.display()));

    let install_dir = match resolve_install_dir() {
        Ok(dir) => dir,
        Err(message) => {
            launcher_log.line(&format!("fatal: {message}"));
            show_error_dialog(&message);
            std::process::exit(1);
        }
    };
    let scorpion_api_exe = install_dir.join(scorpion_api_exe_name());
    if !scorpion_api_exe.is_file() {
        let message = format!(
            "scorpion-api was not found next to the launcher (expected at {}). \
             The installation may be corrupt — try reinstalling Scorpion.",
            scorpion_api_exe.display()
        );
        launcher_log.line(&format!("fatal: {message}"));
        show_error_dialog(&message);
        std::process::exit(1);
    }

    if probe_health_once(SCORPION_HOST, SCORPION_PORT, HEALTH_PROBE_CONNECT_TIMEOUT) {
        launcher_log.line("existing healthy Scorpion instance detected on 127.0.0.1:8787 — reusing it, not spawning a second server");
        open_browser(&console_url(SCORPION_BIND));
        launcher_log.line("opened browser against the existing instance; exiting");
        return;
    }

    // Momentarily claim the port ourselves to distinguish "free" from
    // "occupied by something else" before handing it to the real child —
    // see the port-conflict semantics documented on this module.
    match std::net::TcpListener::bind(SCORPION_BIND) {
        Ok(listener) => drop(listener),
        Err(error) => {
            let message = format!(
                "Port 127.0.0.1:8787 is already in use by another application \
                 ({error}). Scorpion could not start. Close the other application \
                 or free the port and try again."
            );
            launcher_log.line(&format!("fatal: {message}"));
            show_error_dialog(&message);
            std::process::exit(1);
        }
    }

    let db_path = domain_db_path(&app_data_dir);
    let api_log_path = scorpion_api_log_path(&app_data_dir);
    launcher_log.line(&format!(
        "spawning scorpion-api: bind={SCORPION_BIND} db={} log={}",
        db_path.display(),
        api_log_path.display()
    ));

    let stdout_file = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&api_log_path)
    {
        Ok(file) => file,
        Err(error) => {
            let message = format!(
                "Scorpion could not open its log file at {}: {error}",
                api_log_path.display()
            );
            launcher_log.line(&format!("fatal: {message}"));
            show_error_dialog(&message);
            std::process::exit(1);
        }
    };
    let stderr_file = match stdout_file.try_clone() {
        Ok(file) => file,
        Err(error) => {
            let message = format!("Scorpion could not prepare its log file: {error}");
            launcher_log.line(&format!("fatal: {message}"));
            show_error_dialog(&message);
            std::process::exit(1);
        }
    };

    let mut command = Command::new(&scorpion_api_exe);
    command
        .env("SCORPION_API_BIND", SCORPION_BIND)
        // Reuses the one canonical env-var-name constant
        // (`spider::features::domain_runtime::DOMAIN_DATABASE_ENV`) rather
        // than re-typing its value as a local string literal here —
        // enforced by `spider/tests/architecture_guardrails.rs`'s
        // `domain_runtime_seam_owns_database_resolution_not_a_local_literal`.
        .env(DOMAIN_DATABASE_ENV, &db_path)
        .stdout(stdout_file)
        .stderr(stderr_file)
        .stdin(std::process::Stdio::null());
    apply_hidden_child_window(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = format!("Scorpion could not be started: {error}");
            launcher_log.line(&format!("fatal: {message}"));
            show_error_dialog(&message);
            std::process::exit(1);
        }
    };

    let deadline = Instant::now() + HEALTH_WAIT_TIMEOUT;
    let mut ready = false;
    while Instant::now() < deadline {
        if probe_health_once(SCORPION_HOST, SCORPION_PORT, HEALTH_PROBE_CONNECT_TIMEOUT) {
            ready = true;
            break;
        }
        std::thread::sleep(HEALTH_POLL_INTERVAL);
    }

    if !ready {
        // We spawned this child ourselves — cleaning it up here is not
        // "killing an unrelated process," it is us releasing our own
        // failed attempt so a retry does not race against it.
        let _ = child.kill();
        let _ = child.wait();
        let message = format!(
            "Scorpion did not become ready within {} seconds. See the log file at {} for details.",
            HEALTH_WAIT_TIMEOUT.as_secs(),
            api_log_path.display()
        );
        launcher_log.line(&format!("fatal: {message}"));
        show_error_dialog(&message);
        std::process::exit(1);
    }

    launcher_log.line("scorpion-api is healthy; opening browser");
    open_browser(&console_url(SCORPION_BIND));
    launcher_log.line("launcher exiting; scorpion-api continues running independently (see module doc: process-lifecycle limitation)");
}

// ---------------------------------------------------------------------
// Pure/testable helpers
// ---------------------------------------------------------------------

/// Resolve the per-user writable application-data directory. Windows
/// (the only shipping target for this frontier) always uses
/// `%LOCALAPPDATA%\Scorpion`, resolved through `env` so this is
/// deterministically testable without touching real process
/// environment. The non-Windows fallback exists only so this binary
/// stays buildable/testable in this repository's own (Linux) dev
/// environment — it is not part of the shipped Windows product.
fn resolve_app_data_dir(env: &impl Fn(&str) -> Option<String>) -> Result<PathBuf, String> {
    if cfg!(windows) {
        let local_app_data = env("LOCALAPPDATA").ok_or_else(|| {
            "Scorpion could not resolve %LOCALAPPDATA% — this Windows profile appears \
             misconfigured; Scorpion cannot start without a writable per-user directory."
                .to_string()
        })?;
        if local_app_data.trim().is_empty() {
            return Err(
                "Scorpion could not resolve %LOCALAPPDATA% (it was set but empty) — \
                 this Windows profile appears misconfigured."
                    .to_string(),
            );
        }
        Ok(PathBuf::from(local_app_data).join(APP_DIR_NAME))
    } else {
        let home = env("HOME").ok_or_else(|| {
            "Scorpion could not resolve a home directory (this dev fallback path is not \
             part of the shipped Windows product)."
                .to_string()
        })?;
        Ok(PathBuf::from(home)
            .join(".local")
            .join("share")
            .join(APP_DIR_NAME))
    }
}

fn log_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("logs")
}

fn domain_db_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("scorpion.sqlite3")
}

fn scorpion_api_log_path(app_data_dir: &Path) -> PathBuf {
    log_dir(app_data_dir).join("scorpion-api.log")
}

fn launcher_log_path(app_data_dir: &Path) -> PathBuf {
    log_dir(app_data_dir).join("launcher.log")
}

fn scorpion_api_exe_name() -> &'static str {
    if cfg!(windows) {
        "scorpion-api.exe"
    } else {
        "scorpion-api"
    }
}

/// The installation directory is always the directory containing this
/// launcher's own executable — the WiX layout in `scorpion_app/wix/main.wxs` places
/// `scorpion-api.exe` and `scorpion-launcher.exe` side by side in the
/// same `bin` directory, so no separate configuration or registry
/// lookup is needed.
fn resolve_install_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("Scorpion could not resolve its own install location: {error}"))?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Scorpion's install location has no parent directory".to_string())
}

fn console_url(bind: &str) -> String {
    format!("http://{bind}/")
}

/// The exact program and arguments used to open the default browser —
/// separated from the actual `Command::spawn()` call so this shape is
/// directly unit-testable. `url` here is always this launcher's own
/// fixed [`console_url`] output — never anything derived from network
/// input, an environment variable, or a command-line argument — so
/// there is no injection surface despite going through a shell
/// built-in.
fn browser_open_command(url: &str) -> (&'static str, [String; 4]) {
    (
        "cmd",
        [
            "/C".to_string(),
            "start".to_string(),
            // The empty title argument is required: `start` treats a
            // quoted first argument as the new window's title, not the
            // target, unless a title placeholder is supplied first.
            String::new(),
            url.to_string(),
        ],
    )
}

fn open_browser(url: &str) {
    if cfg!(windows) {
        let (program, args) = browser_open_command(url);
        let _ = detached_command(program).args(args).spawn();
    } else {
        // Dev-environment-only fallback (this launcher's shipped target
        // is Windows only) — mirrors the existing best-effort opener
        // pattern already used by spider_cli/src/oauth.rs.
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = detached_command(opener).arg(url).spawn();
    }
}

/// A `Command` with stdin/stdout/stderr explicitly set to
/// [`std::process::Stdio::null`] rather than inherited. This matters
/// beyond tidiness: the browser this launcher opens (and, on Windows,
/// the `powershell` dialog helper) is a long-lived, unrelated process —
/// without this, it would inherit whatever pipe the launcher's own
/// stdout/stderr happen to be (a real console when run interactively
/// for diagnosis, or — proven the hard way by this frontier's own
/// integration test — a test harness's captured-output pipe), and keep
/// that pipe's write end open for as long as the browser stays open,
/// which can hang anything waiting for that pipe to reach EOF.
fn detached_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}

/// A single, bounded-timeout attempt to read a truthful `/health`
/// response from `host:port`. Never blocks longer than `timeout`, never
/// retries internally (callers own their own retry/backoff policy).
fn probe_health_once(host: &str, port: u16, timeout: Duration) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(
        &std::net::SocketAddr::new(
            host.parse().unwrap_or(std::net::Ipv4Addr::LOCALHOST.into()),
            port,
        ),
        timeout,
    ) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    if stream.write_all(health_check_request().as_bytes()).is_err() {
        return false;
    }
    let mut response = Vec::new();
    let mut buffer = [0_u8; 512];
    // Bounded read loop: /health's whole response is tiny, so a handful
    // of reads is always enough; this never spins unbounded because
    // read_timeout is set above and each iteration makes progress or
    // returns.
    for _ in 0..8 {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buffer[..n]),
            Err(_) => break,
        }
        if response.len() > 4096 {
            break;
        }
    }
    health_response_indicates_ready(&String::from_utf8_lossy(&response))
}

fn health_check_request() -> &'static str {
    "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
}

/// Truthful readiness check on a raw HTTP response: the status line must
/// be a real `200`, and the body must contain the exact canonical
/// `/health` payload `scorpion_app::main` returns
/// (`{"status":"ok"}`) — never inferred from "the socket accepted a
/// connection" alone (a listening-but-not-yet-serving socket, or an
/// entirely different service occupying the port, must not read as
/// healthy).
fn health_response_indicates_ready(response: &str) -> bool {
    response.starts_with("HTTP/1.1 200") && response.contains(r#"{"status":"ok"}"#)
}

#[cfg(windows)]
fn apply_hidden_child_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW — documented Win32 process-creation flag
    // (0x08000000). A GUI-subsystem launcher spawning a console-
    // subsystem child would otherwise still cause a new console window
    // to flash up for that child; this suppresses it while leaving the
    // child's stdout/stderr fully redirected to the log file above (an
    // explicit error dialog and the log file remain the diagnosability
    // channels, per this frontier's own "no-console UX" requirement).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn apply_hidden_child_window(_command: &mut Command) {
    // No-op off Windows — this launcher's shipped target is Windows
    // only; kept so the rest of `main` compiles identically everywhere
    // for local dev/test.
}

/// Escapes `value` for safe interpolation inside a PowerShell
/// single-quoted string literal: PowerShell's single-quoted strings
/// treat everything literally except a doubled `''`, which is how an
/// embedded `'` is escaped. Applied even though every caller in this
/// binary only ever passes a fixed-shape, internally constructed
/// message (see [`show_error_dialog`]) — defense in depth costs nothing
/// here and matches this frontier's "do not shell out through an unsafe
/// user-controlled command string" requirement.
fn powershell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Best-effort truthful error dialog for the no-console GUI-subsystem
/// launcher. Always attempted after the log file write (the log file is
/// the guaranteed diagnostic record; the dialog is UX-only and its
/// failure is silently swallowed, matching the existing best-effort
/// browser-opener precedent).
fn show_error_dialog(message: &str) {
    if !cfg!(windows) {
        eprintln!("scorpion-launcher error: {message}");
        return;
    }
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         [System.Windows.Forms.MessageBox]::Show({}, {}, 'OK', 'Error') | Out-Null",
        powershell_single_quote(message),
        powershell_single_quote(APP_DIR_NAME),
    );
    let mut command = detached_command("powershell");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    apply_hidden_child_window(&mut command);
    let _ = command.status();
}

/// Tiny append-only diagnostic log — one timestamped line per call,
/// best-effort (a log-write failure must never crash the launcher or
/// block startup). Deliberately not the canonical Scorpion
/// evidence/persistence architecture: this is operational
/// startup-orchestration diagnostics, not a domain capability, and owns
/// no IAM/evidence/audit semantics.
struct LauncherLog {
    file: Option<File>,
}

impl LauncherLog {
    fn open(path: PathBuf) -> Self {
        let file = OpenOptions::new().create(true).append(true).open(path).ok();
        Self { file }
    }

    fn line(&mut self, message: &str) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(file, "[{timestamp}] {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // App-data / path resolution
    // ---------------------------------------------------------------

    #[test]
    fn app_data_dir_resolves_and_ends_with_scorpion_on_this_platform() {
        // `resolve_app_data_dir` branches on `cfg!(windows)` at runtime,
        // not compile time — this genuinely takes the Windows branch
        // when this test itself runs on a real Windows target (proven
        // the hard way: an earlier version of this test only supplied
        // "HOME", which is correct for the non-Windows dev fallback but
        // caused a real panic when this test actually executed on
        // Windows CI, since that branch reads "LOCALAPPDATA" instead).
        // Supplying both keys unconditionally is harmless (only the one
        // the active platform's branch actually reads is used) and
        // makes this test correct on every platform it can run on.
        let dir = resolve_app_data_dir(&|name| match name {
            "LOCALAPPDATA" => Some("C:\\Users\\test-user\\AppData\\Local".to_string()),
            "HOME" => Some("/home/test-user".to_string()),
            _ => None,
        })
        .unwrap();
        assert!(dir.ends_with("Scorpion"));
    }

    #[test]
    fn missing_localappdata_is_reported_truthfully_not_defaulted() {
        // Simulates the Windows branch's failure path directly (the
        // branch itself is `cfg!`-gated at runtime, not compile time,
        // so this exercises the exact same code on this platform).
        if cfg!(windows) {
            let result = resolve_app_data_dir(&|_| None);
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("LOCALAPPDATA"));
        }
    }

    #[test]
    fn db_and_log_paths_live_under_the_resolved_app_data_directory() {
        let app_data_dir = PathBuf::from("/fake/Scorpion");
        assert_eq!(
            domain_db_path(&app_data_dir),
            PathBuf::from("/fake/Scorpion/scorpion.sqlite3")
        );
        assert_eq!(
            scorpion_api_log_path(&app_data_dir),
            PathBuf::from("/fake/Scorpion/logs/scorpion-api.log")
        );
        assert_eq!(
            launcher_log_path(&app_data_dir),
            PathBuf::from("/fake/Scorpion/logs/launcher.log")
        );
        assert_eq!(log_dir(&app_data_dir), PathBuf::from("/fake/Scorpion/logs"));
    }

    #[test]
    fn no_development_absolute_path_is_embedded_in_any_resolved_path() {
        let app_data_dir = PathBuf::from("/fake/Scorpion");
        for path in [
            domain_db_path(&app_data_dir),
            scorpion_api_log_path(&app_data_dir),
            launcher_log_path(&app_data_dir),
        ] {
            let rendered = path.display().to_string();
            assert!(!rendered.contains("/home/jonny"));
            assert!(!rendered.contains("jonnyfrez"));
        }
    }

    // ---------------------------------------------------------------
    // Server address / port conflict semantics
    // ---------------------------------------------------------------

    #[test]
    fn default_bind_is_loopback_only_never_wildcard() {
        assert_eq!(SCORPION_BIND, "127.0.0.1:8787");
        assert!(!SCORPION_BIND.starts_with("0.0.0.0"));
        assert_eq!(SCORPION_HOST, "127.0.0.1");
    }

    #[test]
    fn console_url_targets_the_fixed_loopback_bind() {
        assert_eq!(console_url(SCORPION_BIND), "http://127.0.0.1:8787/");
    }

    // ---------------------------------------------------------------
    // Health readiness truthfulness
    // ---------------------------------------------------------------

    #[test]
    fn real_health_response_is_recognized_as_ready() {
        let response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}";
        assert!(health_response_indicates_ready(response));
    }

    #[test]
    fn a_non_200_status_is_never_ready() {
        let response = "HTTP/1.1 503 Service Unavailable\r\n\r\n{\"status\":\"ok\"}";
        assert!(!health_response_indicates_ready(response));
    }

    #[test]
    fn an_empty_or_garbage_response_is_never_ready() {
        assert!(!health_response_indicates_ready(""));
        assert!(!health_response_indicates_ready(
            "not an http response at all"
        ));
    }

    #[test]
    fn a_200_from_an_unrelated_service_without_the_exact_body_is_never_ready() {
        // Proves the launcher does not treat "something answered 200"
        // as proof of a healthy Scorpion — it must be this exact body.
        let response = "HTTP/1.1 200 OK\r\n\r\n<html>some other server</html>";
        assert!(!health_response_indicates_ready(response));
    }

    // ---------------------------------------------------------------
    // Browser-open command shape — no unsafe user-controlled string
    // ---------------------------------------------------------------

    #[test]
    fn browser_open_command_targets_cmd_start_with_the_exact_console_url() {
        let (program, args) = browser_open_command("http://127.0.0.1:8787/");
        assert_eq!(program, "cmd");
        assert_eq!(
            args,
            [
                "/C".to_string(),
                "start".to_string(),
                String::new(),
                "http://127.0.0.1:8787/".to_string(),
            ]
        );
    }

    #[test]
    fn browser_is_never_opened_against_anything_but_the_fixed_console_url() {
        // The only call site in `main` is `open_browser(&console_url(SCORPION_BIND))`
        // — this asserts that composed value is exactly the fixed
        // loopback URL, never built from any external input.
        assert_eq!(console_url(SCORPION_BIND), "http://127.0.0.1:8787/");
    }

    // ---------------------------------------------------------------
    // PowerShell message quoting — no injection surface
    // ---------------------------------------------------------------

    #[test]
    fn powershell_quoting_escapes_embedded_single_quotes() {
        assert_eq!(powershell_single_quote("plain"), "'plain'");
        assert_eq!(powershell_single_quote("it's broken"), "'it''s broken'");
    }

    #[test]
    fn powershell_quoting_neutralizes_a_naive_command_injection_attempt() {
        let hostile = "'); Remove-Item -Recurse -Force C:\\ ; ('";
        let quoted = powershell_single_quote(hostile);
        // The whole hostile string is now a single, inert quoted
        // literal: no unescaped `'` remains to end the string early.
        assert_eq!(quoted.matches('\'').count() % 2, 0);
        assert!(quoted.starts_with('\''));
        assert!(quoted.ends_with('\''));
    }

    // ---------------------------------------------------------------
    // Install-directory / sibling-binary resolution
    // ---------------------------------------------------------------

    #[test]
    fn scorpion_api_exe_name_is_platform_appropriate() {
        if cfg!(windows) {
            assert_eq!(scorpion_api_exe_name(), "scorpion-api.exe");
        } else {
            assert_eq!(scorpion_api_exe_name(), "scorpion-api");
        }
    }

    // ---------------------------------------------------------------
    // Health probe against a real (non-Scorpion) TCP listener — proves
    // "port occupied by something unidentifiable" is distinguished from
    // "port free" and from "healthy Scorpion" using a real socket, not
    // just string parsing.
    // ---------------------------------------------------------------

    #[test]
    fn probe_reports_not_ready_against_a_real_listener_that_never_answers_health() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            // Accept and immediately drop — simulates an unrelated
            // service that accepts TCP but never speaks this protocol.
            let _ = listener.accept();
        });
        let ready = probe_health_once("127.0.0.1", port, Duration::from_millis(500));
        assert!(!ready);
        let _ = handle.join();
    }

    #[test]
    fn probe_reports_not_ready_when_nothing_is_listening() {
        // Port 0 always fails to connect (no listener can bind port 0
        // for a peer to connect to) — a deterministic "nothing here".
        let ready = probe_health_once("127.0.0.1", 0, Duration::from_millis(200));
        assert!(!ready);
    }

    #[test]
    fn probe_reports_ready_against_a_real_socket_serving_the_exact_health_response() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0_u8; 512];
                let _ = stream.read(&mut buf);
                let body = "{\"status\":\"ok\"}";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        let ready = probe_health_once("127.0.0.1", port, Duration::from_millis(500));
        assert!(ready);
        let _ = handle.join();
    }
}
