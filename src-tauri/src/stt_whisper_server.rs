//! Resident `whisper-server` so local STT stops paying the model load on every request.
//!
//! `whisper-cli` maps the ~500MB ggml model, transcribes, and exits. Measured on an M1 Pro
//! with `ggml-small.en.bin`, that load is ~0.6s of pure overhead — 0.4s of audio costs the
//! same as 7s of it — and multiple seconds whenever the file has fallen out of the page
//! cache. The progressive-transcript loop fires a request every 2s while dictating, so the
//! reloads dominated local dictation and periodically stalled it for seconds at a time.
//!
//! One long-lived server holds the model resident and answers over HTTP instead. It is the
//! only local engine — there is no CLI path behind it, so a server that will not start is a
//! hard error the user has to fix (missing `whisper-cpp`/`ffmpeg`, or a missing model).

use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Model load for `small.en` measured ~10s cold. Give it headroom, but never block a
/// dictation on it — `endpoint` only waits when a caller explicitly asks it to.
const READY_TIMEOUT: Duration = Duration::from_secs(45);
const PROBE_INTERVAL: Duration = Duration::from_millis(100);

struct Server {
    child: Child,
    port: u16,
    /// Respawn when the user switches models in Settings.
    model: PathBuf,
}

fn slot() -> &'static Mutex<Option<Server>> {
    static SLOT: OnceLock<Mutex<Option<Server>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Base URL of a running server for `model`, starting one if needed.
///
/// `wait` is false on the dictation path: if the model is still loading the caller should
/// use the CLI for this request rather than stall behind a cold start.
pub fn endpoint(model: &Path, threads: u32, wait: bool) -> Option<String> {
    let mut guard = slot().lock().ok()?;

    if let Some(server) = guard.as_mut() {
        let same_model = server.model == model;
        let alive = matches!(server.child.try_wait(), Ok(None));
        if same_model && alive {
            let port = server.port;
            drop(guard);
            return ready(port, wait).then(|| format!("http://127.0.0.1:{port}"));
        }
        // Stale: wrong model, or the server died. Reap it and start clean.
        let _ = server.child.kill();
        let _ = server.child.wait();
        *guard = None;
    }

    let server = spawn(model, threads)?;
    let port = server.port;
    *guard = Some(server);
    drop(guard);

    ready(port, wait).then(|| format!("http://127.0.0.1:{port}"))
}

/// Where the running server's pid is recorded so the next Flow can reap it.
fn pid_file() -> PathBuf {
    // macOS gives each user their own temp dir, so this needs no uid in the name.
    std::env::temp_dir().join("flow-whisper-server.pid")
}

/// Kill a server left behind by a previous Flow process.
///
/// `shutdown` covers a clean quit, but `RunEvent::Exit` never fires on SIGTERM or SIGKILL —
/// a force-quit, a crash, or the install script's launch smoke test all orphan the child,
/// each one holding ~500MB resident forever. Reaping on the next start bounds that to one.
fn reap_orphan() {
    let path = pid_file();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return;
    };
    let _ = std::fs::remove_file(&path);
    let Ok(pid) = raw.trim().parse::<i32>() else {
        return;
    };
    // Pids get reused, so confirm what we are about to signal really is our server.
    if is_whisper_server(pid) {
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
}

fn is_whisper_server(pid: i32) -> bool {
    Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("whisper-server"))
        .unwrap_or(false)
}

fn spawn(model: &Path, threads: u32) -> Option<Server> {
    reap_orphan();

    let binary = crate::stt_whisper::resolve_executable("whisper-server")?;
    let ffmpeg = crate::stt_whisper::resolve_executable("ffmpeg")?;
    let port = free_port()?;

    // A bundle launched from Finder inherits a bare PATH and `/` as its working directory.
    // `--convert` shells out to `ffmpeg` by name, and `--tmp-dir` defaults to `.` — left
    // alone, the server would come up fine and then fail every request, in the bundle only.
    // `resolve_executable` may hand back a bare name when it found the binary on PATH
    // already; there is nothing to prepend in that case.
    let ffmpeg_dir = ffmpeg
        .parent()
        .and_then(Path::to_str)
        .filter(|dir| !dir.is_empty())
        .unwrap_or_default()
        .to_string();
    let path = match (ffmpeg_dir.as_str(), std::env::var("PATH")) {
        ("", Ok(existing)) => existing,
        (dir, Ok(existing)) => format!("{dir}:{existing}"),
        (dir, Err(_)) => dir.to_string(),
    };
    let tmp_dir = std::env::temp_dir();

    // `--convert` lets the server accept the browser's webm directly, so the caller does not
    // have to transcode first.
    //
    // Decoding knobs mirror the CLI path exactly, so which engine served a request can
    // never change what the user gets back — only how long it took.
    let child = Command::new(binary)
        .args([
            "-m",
            model.to_str()?,
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "-t",
            &threads.to_string(),
            "-bo",
            "8",
            "-bs",
            "8",
            "-nth",
            "0.7",
            "--convert",
            "--tmp-dir",
            tmp_dir.to_str()?,
        ])
        .env("PATH", path)
        .current_dir(&tmp_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let _ = std::fs::write(pid_file(), child.id().to_string());

    Some(Server {
        child,
        port,
        model: model.to_path_buf(),
    })
}

/// Bind :0 to have the OS pick a port, then release it for the server. The gap is racy in
/// principle; in practice nothing else on the machine is grabbing ephemeral ports in that
/// window, and a lost race just means the spawn fails and callers use the CLI.
fn free_port() -> Option<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    drop(listener);
    Some(port)
}

fn ready(port: u16, wait: bool) -> bool {
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    let deadline = Instant::now() + if wait { READY_TIMEOUT } else { Duration::ZERO };
    loop {
        if TcpStream::connect_timeout(&addr.into(), Duration::from_millis(200)).is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(PROBE_INTERVAL);
    }
}

/// Start the server ahead of the first dictation, so its model load overlaps app startup
/// instead of the user's first press.
pub fn prewarm(model: PathBuf, threads: u32) {
    std::thread::spawn(move || {
        let _ = endpoint(&model, threads, true);
    });
}

/// Kill the child on app exit — it holds ~500MB resident and would otherwise be orphaned.
pub fn shutdown() {
    let Ok(mut guard) = slot().lock() else {
        return;
    };
    if let Some(server) = guard.as_mut() {
        let _ = server.child.kill();
        let _ = server.child.wait();
    }
    *guard = None;
    let _ = std::fs::remove_file(pid_file());
}
