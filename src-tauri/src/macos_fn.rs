//! Global `fn` (Globe) key via CGEventTap (reliable). Carbon RegisterEventHotKey for bare
//! keys is flaky on modern macOS and often never fires.
//!
//! `fn` has no keyDown/keyUp of its own — macOS reports it as a `flagsChanged` event with
//! `kCGEventFlagMaskSecondaryFn` set/cleared, so state is tracked as an edge on that bit
//! rather than a key code.
//!
//! - Hold `fn` → activate (dictate)
//! - Hold `fn` + `1` → auto-prompt (vibe)
//! - Hold `fn` + `2` → refine

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::thread;

use tauri::{AppHandle, Emitter, Manager};

use crate::focus::RecordingFlag;
use crate::{begin_recording_session, handle_vibe_refine, stop_recording_session};

const KEY_1: i64 = 0x12;
const KEY_2: i64 = 0x13;

const MODE_DICTATE: u8 = 0;
const MODE_VIBE: u8 = 1;

/// kCGEventFlagMaskSecondaryFn — set while the `fn`/Globe key is held.
const FN_MASK: u64 = 0x0080_0000;

static FN_DOWN: AtomicBool = AtomicBool::new(false);
static KEY1_DOWN: AtomicBool = AtomicBool::new(false);
static KEY2_DOWN: AtomicBool = AtomicBool::new(false);
/// One refine per fn hold — `2` can arrive before or after the fn edge, and both paths
/// funnel into `start_refine`.
static REFINE_FIRED: AtomicBool = AtomicBool::new(false);
static SESSION_MODE: AtomicU8 = AtomicU8::new(MODE_DICTATE);
static APP: OnceLock<AppHandle> = OnceLock::new();
static TAP_PORT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

type CGEventRef = *mut c_void;
type CGEventTapProxy = *mut c_void;
type CFMachPortRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFStringRef = *const c_void;

const K_CG_SESSION_EVENT_TAP: u32 = 1;
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0; // active filter — we can swallow fn
const K_CG_EVENT_KEY_DOWN: u32 = 10;
const K_CG_EVENT_KEY_UP: u32 = 11;
const K_CG_EVENT_FLAGS_CHANGED: u32 = 12;
const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
const K_CF_RUN_LOOP_COMMON_MODES: &str = "kCFRunLoopCommonModes";

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: Option<
            unsafe extern "C" fn(
                CGEventTapProxy,
                u32,
                CGEventRef,
                *mut c_void,
            ) -> CGEventRef,
        >,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: CFMachPortRef,
        order: i64,
    ) -> CFRunLoopSourceRef;
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRun();
    fn CFRetain(cf: *const c_void) -> *const c_void;
    static kCFRunLoopCommonModes: CFStringRef;
}

fn mode_label() -> &'static str {
    if SESSION_MODE.load(Ordering::SeqCst) == MODE_VIBE {
        "vibe"
    } else {
        "dictate"
    }
}

fn log_debug(msg: &str) {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/flow-section.log")
        .and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "{} {}", chrono_like_now(), msg)
        });
}

fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

fn on_fn_down(app: &AppHandle) {
    log_debug("fn DOWN");
    if FN_DOWN.swap(true, Ordering::SeqCst) {
        return; // already down (shouldn't repeat for flagsChanged, but stay idempotent)
    }
    REFINE_FIRED.store(false, Ordering::SeqCst);

    // `2` already held when fn went down → refine immediately, no mic session.
    if KEY2_DOWN.load(Ordering::SeqCst) || unsafe { CGEventSourceKeyState(1, KEY_2 as u16) } {
        start_refine(app);
        return;
    }

    let vibe = KEY1_DOWN.load(Ordering::SeqCst) || unsafe { CGEventSourceKeyState(1, KEY_1 as u16) };
    SESSION_MODE.store(
        if vibe { MODE_VIBE } else { MODE_DICTATE },
        Ordering::SeqCst,
    );
    let mode = mode_label();
    let _ = app.emit(
        "dictation-status",
        if vibe {
            "fn+1 auto-prompt — speak, release fn"
        } else {
            "fn activate — speak (press 1 for auto-prompt), release fn"
        },
    );

    let already_active = app
        .state::<RecordingFlag>()
        .active
        .lock()
        .map(|g| *g)
        .unwrap_or(true);
    if already_active {
        return;
    }

    if let Err(err) = begin_recording_session(app, mode) {
        let _ = app.emit("dictation-error", format!("fn start failed: {err}"));
        FN_DOWN.store(false, Ordering::SeqCst);
        return;
    }
    set_recording_active(app, true);
}

/// fn+2, pressed in either order.
///
/// Holding `2` first is handled on the fn edge, but pressing `2` *after* fn is the natural
/// way to type the chord — and that edge has already opened a mic session for plain
/// activate, since it could not know `2` was coming. Tear that session down before
/// refining: otherwise the recorder keeps running and the fn release transcribes it on top
/// of the refined prompt.
fn start_refine(app: &AppHandle) {
    if REFINE_FIRED.swap(true, Ordering::SeqCst) {
        return;
    }

    let was_active = app
        .state::<RecordingFlag>()
        .active
        .lock()
        .map(|g| *g)
        .unwrap_or(false);
    if was_active {
        // Clear the flag before refining — `handle_vibe_refine` refuses to run while a
        // recording is active.
        set_recording_active(app, false);
        // Discard the audio instead of going through `recording-stop`, which would
        // transcribe and paste it.
        let _ = app.emit("recording-cancel", "refine");
    }

    let _ = app.emit("dictation-status", "fn+2 refine");
    handle_vibe_refine(app);

    // The chord is spent; releasing fn must not also fire a stop.
    FN_DOWN.store(false, Ordering::SeqCst);
}

fn set_recording_active(app: &AppHandle, value: bool) {
    let state = app.state::<RecordingFlag>();
    let Ok(mut guard) = state.active.lock() else {
        return;
    };
    *guard = value;
}

fn on_fn_up(app: &AppHandle) {
    log_debug("fn UP");
    if !FN_DOWN.swap(false, Ordering::SeqCst) {
        return;
    }
    if KEY1_DOWN.load(Ordering::SeqCst) {
        SESSION_MODE.store(MODE_VIBE, Ordering::SeqCst);
    }
    let mode = mode_label().to_string();
    let was_active = app
        .state::<RecordingFlag>()
        .active
        .lock()
        .map(|g| *g)
        .unwrap_or(false);
    if was_active {
        set_recording_active(app, false);
        let _ = app.emit(
            "dictation-status",
            format!("fn released → processing ({mode})"),
        );
        stop_recording_session(app, &mode);
    }
}

unsafe extern "C" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    _info: *mut c_void,
) -> CGEventRef {
    // If the tap is disabled by timeout/user input, re-enable it.
    if event_type == 0xFFFFFFFE || event_type == 0xFFFFFFFF {
        let tap = TAP_PORT.load(Ordering::SeqCst);
        if !tap.is_null() {
            CGEventTapEnable(tap, true);
        }
        return event;
    }

    if event_type == K_CG_EVENT_FLAGS_CHANGED {
        let flags = CGEventGetFlags(event);
        let now_fn = (flags & FN_MASK) != 0;
        if now_fn == FN_DOWN.load(Ordering::SeqCst) {
            return event; // not an fn edge (some other modifier changed) — pass through
        }

        log_debug(&format!("raw fn {}", if now_fn { "down" } else { "up" }));

        let Some(app) = APP.get() else {
            return event;
        };

        // Must hop to the main thread — mic/UI/osascript from the tap thread silently fails.
        if now_fn {
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || on_fn_down(&handle));
        } else {
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || on_fn_up(&handle));
        }

        // Swallow the fn transition so it doesn't trigger macOS's own configured fn
        // action (e.g. Start Dictation / Emoji & Symbols).
        return std::ptr::null_mut();
    }

    if event_type != K_CG_EVENT_KEY_DOWN && event_type != K_CG_EVENT_KEY_UP {
        return event;
    }

    let code = CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE);
    let is_down = event_type == K_CG_EVENT_KEY_DOWN;

    // Read once: `start_refine` clears FN_DOWN, and the swallow decision below must match
    // the dispatch decision above it.
    let fn_held = FN_DOWN.load(Ordering::SeqCst);

    // While fn is held the digit belongs to Flow's chord, so it is swallowed — letting it
    // through typed a stray "1"/"2" into the target app immediately before the prompt
    // landed there (and fn+2's select-all then swept that digit into the refine input).
    // Digits typed without fn are never touched.
    if code == KEY_1 {
        KEY1_DOWN.store(is_down, Ordering::SeqCst);
        if is_down && fn_held {
            SESSION_MODE.store(MODE_VIBE, Ordering::SeqCst);
            if let Some(app) = APP.get() {
                let _ = app.emit("dictation-status", "fn+1 auto-prompt armed");
                let _ = app.emit("session-mode", "vibe");
            }
        }
        return if fn_held { std::ptr::null_mut() } else { event };
    }
    if code == KEY_2 {
        KEY2_DOWN.store(is_down, Ordering::SeqCst);
        if is_down && fn_held {
            if let Some(app) = APP.get() {
                // Mic, osascript, and UI work all fail silently off the main thread.
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || start_refine(&handle));
            }
        }
        return if fn_held { std::ptr::null_mut() } else { event };
    }

    event
}

fn event_mask() -> u64 {
    (1u64 << K_CG_EVENT_KEY_DOWN) | (1u64 << K_CG_EVENT_KEY_UP) | (1u64 << K_CG_EVENT_FLAGS_CHANGED)
}

/// Start a background CFRunLoop thread with a session event tap for `fn`.
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

pub fn install(app: &AppHandle) -> Result<(), String> {
    APP.set(app.clone())
        .map_err(|_| "fn listener already installed".to_string())?;

    let trusted = unsafe { AXIsProcessTrusted() };
    log_debug(&format!("install AXIsProcessTrusted={trusted}"));
    if !trusted {
        let _ = app.emit(
            "dictation-error",
            "Accessibility is OFF for the currently running Flow binary. Open Accessibility settings and verify the exact Flow app you launched is enabled. fn cannot work without this.",
        );
    }

    let app_for_err = app.clone();
    thread::Builder::new()
        .name("flow-fn-tap".into())
        .spawn(move || unsafe {
            // Prefer HID tap; fall back to session tap.
            let mut tap = CGEventTapCreate(
                0, // kCGHIDEventTap
                K_CG_HEAD_INSERT_EVENT_TAP,
                K_CG_EVENT_TAP_OPTION_DEFAULT,
                event_mask(),
                Some(tap_callback),
                std::ptr::null_mut(),
            );
            if tap.is_null() {
                tap = CGEventTapCreate(
                    K_CG_SESSION_EVENT_TAP,
                    K_CG_HEAD_INSERT_EVENT_TAP,
                    K_CG_EVENT_TAP_OPTION_DEFAULT,
                    event_mask(),
                    Some(tap_callback),
                    std::ptr::null_mut(),
                );
            }
            if tap.is_null() {
                log_debug("CGEventTapCreate returned null");
                let _ = app_for_err.emit(
                    "dictation-error",
                    "fn hotkey failed: System Settings → Privacy → Accessibility → enable Flow, then restart Flow. Quit commercial Wispr Flow.",
                );
                return;
            }
            log_debug("CGEventTapCreate OK");
            TAP_PORT.store(tap, Ordering::SeqCst);

            let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
            if source.is_null() {
                let _ = app_for_err.emit("dictation-error", "fn hotkey: runloop source failed");
                return;
            }

            CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
            CGEventTapEnable(tap, true);

            let _ = app_for_err.emit(
                "dictation-warning",
                "fn listener ON — hold fn to activate, fn+1 auto-prompt, fn+2 refine",
            );

            let _ = K_CF_RUN_LOOP_COMMON_MODES;
            let _ = CFRetain;

            CFRunLoopRun();
        })
        .map_err(|e| format!("Failed to start fn listener thread: {e}"))?;

    // Commercial Wispr Flow steals mic + hotkeys — quit it automatically.
    if wispr_flow_running() {
        let _ = std::process::Command::new("pkill")
            .args(["-f", "/Applications/Wispr Flow.app"])
            .status();
        let _ = app.emit(
            "dictation-warning",
            "Quit commercial Wispr Flow automatically so our Flow can use the mic/hotkeys.",
        );
    }

    Ok(())
}

fn wispr_flow_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-f", "/Applications/Wispr Flow.app/Contents/MacOS/Wispr Flow"])
        .stdout(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
