//! Global `fn` (Globe) key via CGEventTap (reliable). Carbon RegisterEventHotKey for bare
//! keys is flaky on modern macOS and often never fires.
//!
//! `fn` has no keyDown/keyUp of its own — macOS reports it as a `flagsChanged` event with
//! `kCGEventFlagMaskSecondaryFn` set/cleared, so state is tracked as an edge on that bit
//! rather than a key code.
//!
//! - Hold `fn` → activate (dictate)
//! - Press `fn` + `1` → prompt from current text (vibe)
//! - Press `fn` + `2` → grammar-correct current text
//! - Press `fn` + `3` → answer latest live-conversation question

#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::focus::RecordingFlag;
use crate::{
    begin_recording_session, handle_correct_text, handle_meeting_answer, handle_vibe_prompt,
    stop_recording_session,
};

const KEY_1: i64 = 0x12;
const KEY_2: i64 = 0x13;
const KEY_3: i64 = 0x14;

const MODE_DICTATE: u8 = 0;
const DICTATION_CHORD_GRACE_MS: u64 = 180;

/// kCGEventFlagMaskSecondaryFn — set while the `fn`/Globe key is held.
const FN_MASK: u64 = 0x0080_0000;

static FN_DOWN: AtomicBool = AtomicBool::new(false);
static KEY1_DOWN: AtomicBool = AtomicBool::new(false);
static KEY2_DOWN: AtomicBool = AtomicBool::new(false);
static KEY3_DOWN: AtomicBool = AtomicBool::new(false);
/// One correction per fn hold — `2` can arrive before or after the fn edge.
static REFINE_FIRED: AtomicBool = AtomicBool::new(false);
/// One prompt transform per fn hold — `1` can arrive before or after the fn edge.
static PROMPT_FIRED: AtomicBool = AtomicBool::new(false);
/// One live answer per fn hold — `3` can arrive before or after the fn edge.
static ANSWER_FIRED: AtomicBool = AtomicBool::new(false);
static SESSION_MODE: AtomicU8 = AtomicU8::new(MODE_DICTATE);
static FN_GENERATION: AtomicU64 = AtomicU64::new(0);
static APP: OnceLock<AppHandle> = OnceLock::new();
static TAP_PORT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

type CGEventRef = *mut c_void;
type CGEventTapProxy = *mut c_void;
type CFMachPortRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFStringRef = *const c_void;

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
            unsafe extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef,
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

fn log_debug(msg: &str) {
    if std::env::var_os("FLOW_DEBUG_FN").is_none() {
        return;
    }
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

/// Take the fn-down edge, on the event-tap thread.
///
/// The chord state MUST settle here rather than in `on_fn_down`: the digit branch of the same
/// tap callback reads `FN_DOWN` synchronously, and `on_fn_down` is dispatched to the main
/// thread, which this app routinely blocks for hundreds of milliseconds in `osascript`. While
/// it was blocked the `2` of a fn+2 arrived with `FN_DOWN` still false — so the digit was
/// neither swallowed (a stray "2" landed in the user's field) nor treated as a chord, and the
/// press fell through to plain dictation.
///
/// Returns true when this call was the actual transition.
fn note_fn_down_edge(app: &AppHandle) -> bool {
    if FN_DOWN.swap(true, Ordering::SeqCst) {
        return false; // already down — stay idempotent
    }
    let generation = FN_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    REFINE_FIRED.store(false, Ordering::SeqCst);
    PROMPT_FIRED.store(false, Ordering::SeqCst);
    ANSWER_FIRED.store(false, Ordering::SeqCst);

    // Mic/UI/osascript work silently fails off the main thread, so only the side effects hop.
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || on_fn_down(&handle, generation));
    true
}

/// Release the fn-down edge, on the event-tap thread. Mirrors `note_fn_down_edge`.
fn note_fn_up_edge(app: &AppHandle) {
    if !FN_DOWN.swap(false, Ordering::SeqCst) {
        return;
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || on_fn_up(&handle));
}

fn on_fn_down(app: &AppHandle, generation: u64) {
    log_debug("fn DOWN");
    if FN_GENERATION.load(Ordering::SeqCst) != generation {
        return; // fn was already released and pressed again before this hop ran
    }

    // `2` already held when fn went down → correct current text immediately, no mic session.
    if KEY2_DOWN.load(Ordering::SeqCst) || unsafe { CGEventSourceKeyState(1, KEY_2 as u16) } {
        start_correct_text(app);
        return;
    }

    // `3` already held when fn went down → answer latest live question immediately.
    if KEY3_DOWN.load(Ordering::SeqCst) || unsafe { CGEventSourceKeyState(1, KEY_3 as u16) } {
        start_meeting_answer(app);
        return;
    }

    // `1` already held when fn went down → prompt from current text immediately, no mic session.
    if KEY1_DOWN.load(Ordering::SeqCst) || unsafe { CGEventSourceKeyState(1, KEY_1 as u16) } {
        start_prompt(app);
        return;
    }

    let _ = app.emit("dictation-status", "fn held");

    // Give digit chords a short window to arrive before opening the microphone. Without
    // this, fn+1 first starts raw dictation and then races a cancellation against prompt
    // capture, which makes the chord feel dead on fast presses.
    let app_handle = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(DICTATION_CHORD_GRACE_MS));
        let handle_for_callback = app_handle.clone();
        let _ = app_handle
            .run_on_main_thread(move || start_dictation_if_plain(&handle_for_callback, generation));
    });
}

fn start_dictation_if_plain(app: &AppHandle, generation: u64) {
    if !FN_DOWN.load(Ordering::SeqCst) {
        return;
    }
    if FN_GENERATION.load(Ordering::SeqCst) != generation {
        return;
    }
    if PROMPT_FIRED.load(Ordering::SeqCst)
        || REFINE_FIRED.load(Ordering::SeqCst)
        || ANSWER_FIRED.load(Ordering::SeqCst)
    {
        return;
    }

    SESSION_MODE.store(MODE_DICTATE, Ordering::SeqCst);
    let _ = app.emit("dictation-status", "fn raw dictation — speak, release fn");

    let already_active = app
        .state::<RecordingFlag>()
        .active
        .lock()
        .map(|g| *g)
        .unwrap_or(true);
    if already_active {
        return;
    }

    if let Err(err) = begin_recording_session(app, "dictate") {
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
/// correcting: otherwise the recorder keeps running and the fn release transcribes it on top
/// of the corrected text.
fn start_correct_text(app: &AppHandle) {
    if REFINE_FIRED.swap(true, Ordering::SeqCst) {
        log_debug("fn+2 ignored (already fired this hold)");
        return;
    }
    log_debug("fn+2 START correct");

    let was_active = app
        .state::<RecordingFlag>()
        .active
        .lock()
        .map(|g| *g)
        .unwrap_or(false);
    if was_active {
        // Clear the flag before correcting — `handle_correct_text` refuses to run while a
        // recording is active.
        set_recording_active(app, false);
        // Discard the audio instead of going through `recording-stop`, which would
        // transcribe and paste it.
        let _ = app.emit("recording-cancel", "correct_text");
    }

    let _ = app.emit("dictation-status", "fn+2 correcting text");
    handle_correct_text(app);

    // The chord is spent; releasing fn must not also fire a stop.
}

/// fn+3, pressed in either order. Cancels any speculative raw dictation session and
/// answers the latest question from the live conversation transcript instead.
fn start_meeting_answer(app: &AppHandle) {
    if ANSWER_FIRED.swap(true, Ordering::SeqCst) {
        log_debug("fn+3 ignored (already fired this hold)");
        return;
    }
    log_debug("fn+3 START answer");

    let was_active = app
        .state::<RecordingFlag>()
        .active
        .lock()
        .map(|g| *g)
        .unwrap_or(false);
    if was_active {
        set_recording_active(app, false);
        let _ = app.emit("recording-cancel", "meeting_answer");
    }

    let _ = app.emit("dictation-status", "fn+3 answering live question");
    handle_meeting_answer(app);

    // The chord is spent; releasing fn must not also fire a stop.
}

/// fn+1, pressed in either order.
///
/// If fn was pressed first, a raw dictation session may already be opening. Cancel it and
/// transform the text currently in the focused field instead.
fn start_prompt(app: &AppHandle) {
    if PROMPT_FIRED.swap(true, Ordering::SeqCst) {
        log_debug("fn+1 ignored (already fired this hold)");
        return;
    }
    log_debug("fn+1 START prompt");

    let was_active = app
        .state::<RecordingFlag>()
        .active
        .lock()
        .map(|g| *g)
        .unwrap_or(false);
    if was_active {
        set_recording_active(app, false);
        let _ = app.emit("recording-cancel", "vibe_text");
    }

    let _ = app.emit("dictation-status", "fn+1 prompt from text");
    handle_vibe_prompt(app);

    // The chord is spent; releasing fn must not also fire a stop.
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
    let mode = "dictate".to_string();
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

        if now_fn {
            note_fn_down_edge(app);
        } else {
            note_fn_up_edge(app);
        }

        // Swallow the fn transition so it doesn't trigger macOS's own configured fn
        // action (e.g. Start Dictation / Emoji & Symbols).
        return std::ptr::null_mut();
    }

    if event_type != K_CG_EVENT_KEY_DOWN && event_type != K_CG_EVENT_KEY_UP {
        return event;
    }

    let code = CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE);
    if code != KEY_1 && code != KEY_2 && code != KEY_3 {
        return event;
    }

    let is_down = event_type == K_CG_EVENT_KEY_DOWN;
    let Some(app) = APP.get() else {
        return event;
    };

    // The digit event carries its own modifier snapshot, so ask the hardware rather than
    // trusting bookkeeping: this is true the instant fn is physically down, even if the
    // flagsChanged edge for it is still queued. Adopt that edge here when it is, so the
    // per-hold "already fired" flags are reset before any chord runs.
    let fn_flagged = (CGEventGetFlags(event) & FN_MASK) != 0;

    // Publish the digit before adopting the edge, so an `on_fn_down` that starts on the main
    // thread while this callback is still running sees it as already held.
    match code {
        KEY_1 => KEY1_DOWN.store(is_down, Ordering::SeqCst),
        KEY_2 => KEY2_DOWN.store(is_down, Ordering::SeqCst),
        _ => KEY3_DOWN.store(is_down, Ordering::SeqCst),
    }

    if fn_flagged {
        note_fn_down_edge(app);
    }
    // Read once: chord handlers clear FN_DOWN, and the swallow decision below must match
    // the dispatch decision above it.
    let fn_held = fn_flagged || FN_DOWN.load(Ordering::SeqCst);

    // While fn is held the digit belongs to Flow's chord, so it is swallowed — letting it
    // through typed a stray "1"/"2"/"3" into the target app immediately before the output
    // landed there.
    // Digits typed without fn are never touched.
    if is_down {
        let digit = match code {
            KEY_1 => 1,
            KEY_2 => 2,
            _ => 3,
        };
        log_debug(&format!(
            "digit {digit} down (fn_flagged={fn_flagged} fn_tracked={} → chord={fn_held})",
            FN_DOWN.load(Ordering::SeqCst)
        ));
    }

    if is_down && fn_held {
        let handle = app.clone();
        // Mic, osascript, and UI work all fail silently off the main thread.
        let _ = app.run_on_main_thread(move || match code {
            KEY_1 => start_prompt(&handle),
            KEY_2 => start_correct_text(&handle),
            _ => start_meeting_answer(&handle),
        });
    }

    if fn_held {
        std::ptr::null_mut()
    } else {
        event
    }
}

fn event_mask() -> u64 {
    (1u64 << K_CG_EVENT_KEY_DOWN) | (1u64 << K_CG_EVENT_KEY_UP) | (1u64 << K_CG_EVENT_FLAGS_CHANGED)
}

// Start a background CFRunLoop thread with a session event tap for `fn`.
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
            // HID tap only — it is the one that sees the fn flag before anything else can
            // swallow it.
            let tap = CGEventTapCreate(
                0, // kCGHIDEventTap
                K_CG_HEAD_INSERT_EVENT_TAP,
                K_CG_EVENT_TAP_OPTION_DEFAULT,
                event_mask(),
                Some(tap_callback),
                std::ptr::null_mut(),
            );
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
                "fn listener ON — hold fn raw, fn+1 prompt, fn+2 correct, fn+3 answer",
            );

            let _ = K_CF_RUN_LOOP_COMMON_MODES;
            let _ = CFRetain;

            CFRunLoopRun();
        })
        .map_err(|e| format!("Failed to start fn listener thread: {e}"))?;

    if wispr_flow_running() {
        let _ = app.emit(
            "dictation-warning",
            "Commercial Wispr Flow is running and will compete for the mic and fn key. Quit it for Flow to work.",
        );
    }

    Ok(())
}

fn wispr_flow_running() -> bool {
    std::process::Command::new("pgrep")
        .args([
            "-f",
            "/Applications/Wispr Flow.app/Contents/MacOS/Wispr Flow",
        ])
        .stdout(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
