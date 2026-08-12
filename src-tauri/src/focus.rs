use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use tauri_plugin_clipboard_manager::ClipboardExt;

/// True while paste is in progress — dock must not call orderFrontRegardless.
static PASTING: AtomicBool = AtomicBool::new(false);

pub fn set_pasting(active: bool) {
    PASTING.store(active, Ordering::SeqCst);
}

pub fn is_pasting() -> bool {
    PASTING.load(Ordering::SeqCst)
}

#[derive(Clone, Debug)]
pub struct TargetApp {
    pub name: String,
    pub pid: i32,
}

/// App that should receive the dictated text (captured when hotkey is pressed).
pub struct DictationTarget {
    pub app: Mutex<Option<TargetApp>>,
    pub selected_text: Mutex<Option<String>>,
}

impl Default for DictationTarget {
    fn default() -> Self {
        Self {
            app: Mutex::new(None),
            selected_text: Mutex::new(None),
        }
    }
}

pub struct RecordingFlag {
    pub active: Mutex<bool>,
}

impl Default for RecordingFlag {
    fn default() -> Self {
        Self {
            active: Mutex::new(false),
        }
    }
}

#[derive(Clone, Serialize)]
struct DictationFallbackPayload {
    text: String,
    message: String,
}

pub fn capture_frontmost_app(app: &AppHandle) -> Result<String, String> {
    let target = frontmost_app()?;
    let state = app.state::<DictationTarget>();
    let mut guard = state
        .app
        .lock()
        .map_err(|_| "Failed to lock target app".to_string())?;

    // If Hub/Dock is focused, keep the previous target app instead of aborting.
    // Blocking here made Control+1 look completely dead while Flow Hub was open.
    if is_flow_app(&target.name) {
        if let Some(prev) = guard.clone() {
            return Ok(prev.name);
        }
        // No prior target yet — still allow recording; paste will fall back to clipboard.
        return Ok(target.name);
    }

    *guard = Some(target.clone());
    Ok(target.name)
}

pub fn target_app_name(app: &AppHandle) -> Option<String> {
    let state = app.state::<DictationTarget>();
    state
        .app
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|t| t.name.clone()))
}

pub fn target_app(app: &AppHandle) -> Option<TargetApp> {
    let state = app.state::<DictationTarget>();
    state.app.lock().ok().and_then(|g| g.clone())
}

pub fn clear_selected_text(app: &AppHandle) {
    if let Ok(mut guard) = app.state::<DictationTarget>().selected_text.lock() {
        *guard = None;
    }
}

pub fn try_capture_selected_text(app: &AppHandle) -> Result<Option<String>, String> {
    clear_selected_text(app);
    match capture_selected_text(app) {
        Ok(text) => Ok(Some(text)),
        Err(err) => {
            // No selection is fine for prompt mode. Accessibility failures are not.
            if err.to_lowercase().contains("no text selected") {
                Ok(None)
            } else {
                Err(err)
            }
        }
    }
}

pub fn capture_selected_text(app: &AppHandle) -> Result<String, String> {
    let previous = app
        .clipboard()
        .read_text()
        .map_err(|e| format!("Clipboard read failed before copy: {e}"))?;

    #[cfg(target_os = "macos")]
    {
        let status = Command::new("osascript")
            .args([
                "-e",
                r#"tell application "System Events" to keystroke "c" using command down"#,
            ])
            .status()
            .map_err(|e| format!("Copy selection failed: {e}"))?;
        if !status.success() {
            return Err("Could not copy selection. Grant Accessibility permission.".into());
        }
        thread::sleep(Duration::from_millis(80));
    }

    let selected = app
        .clipboard()
        .read_text()
        .map_err(|e| format!("Clipboard read failed: {e}"))?;

    app.clipboard()
        .write_text(previous)
        .map_err(|e| format!("Clipboard restore failed: {e}"))?;

    let selected = selected.trim().to_string();
    if selected.is_empty() {
        return Err("No text selected. Highlight text first for command mode.".into());
    }

    let state = app.state::<DictationTarget>();
    *state
        .selected_text
        .lock()
        .map_err(|_| "Failed to lock selected text".to_string())? = Some(selected.clone());

    Ok(selected)
}

pub fn take_selected_text(app: &AppHandle) -> Option<String> {
    let state = app.state::<DictationTarget>();
    state
        .selected_text
        .lock()
        .ok()
        .and_then(|mut g| g.take())
}

/// Control+2: select the entire document/prompt (⌘A) then copy it.
pub fn select_all_and_capture(app: &AppHandle) -> Result<String, String> {
    let previous = app
        .clipboard()
        .read_text()
        .map_err(|e| format!("Clipboard read failed before select-all: {e}"))?;

    #[cfg(target_os = "macos")]
    {
        let status = Command::new("osascript")
            .args([
                "-e",
                r#"tell application "System Events" to keystroke "a" using command down"#,
                "-e",
                "delay 0.05",
                "-e",
                r#"tell application "System Events" to keystroke "c" using command down"#,
            ])
            .status()
            .map_err(|e| format!("Select-all/copy failed: {e}"))?;
        if !status.success() {
            return Err(
                "Could not select/copy the prompt. Grant Accessibility permission.".into(),
            );
        }
        thread::sleep(Duration::from_millis(100));
    }

    let selected = app
        .clipboard()
        .read_text()
        .map_err(|e| format!("Clipboard read failed: {e}"))?;

    app.clipboard()
        .write_text(previous)
        .map_err(|e| format!("Clipboard restore failed: {e}"))?;

    let selected = selected.trim().to_string();
    if selected.is_empty() {
        return Err(
            "No prompt text found after select-all. Generate a prompt with fn+1 and paste it first, then press fn+2 (or Control+2)."
                .into(),
        );
    }

    let state = app.state::<DictationTarget>();
    *state
        .selected_text
        .lock()
        .map_err(|_| "Failed to lock selected text".to_string())? = Some(selected.clone());

    Ok(selected)
}

/// How the text reached the target app — decides when, or whether, the clipboard
/// can be put back.
enum PasteRoute {
    /// Inserted directly (AX or synthesized typing); the clipboard was never consumed.
    Direct,
    /// Cmd+V was dispatched; the target still needs a moment to consume it.
    Clipboard,
    /// Nothing was inserted — the result stays on the clipboard for the user to paste.
    Retain,
}

pub fn paste_into_target_app(app: &AppHandle, text: &str) -> Result<(), String> {
    set_pasting(true);
    let result = (|| {
        // Snapshot first: the paste routes below clobber the clipboard.
        let previous = app.clipboard().read_text().ok();

        let route = match paste_route(app, text) {
            Ok(route) => route,
            Err(err) => {
                emit_copy_fallback(app, text, &err);
                return Err(err);
            }
        };
        match route {
            PasteRoute::Retain => {
                let err = "Couldn't insert into the text field — result is on the clipboard (⌘V). Click the field in Cursor/WhatsApp first, and grant Flow Accessibility."
                    .to_string();
                emit_copy_fallback(app, text, &err);
                return Err(err);
            }
            // Electron apps are unreliable — keep the transcript on the clipboard as
            // backup instead of restoring the previous pasteboard.
            PasteRoute::Clipboard => {
                thread::sleep(Duration::from_millis(900));
                return Ok(());
            }
            PasteRoute::Direct => thread::sleep(Duration::from_millis(80)),
        }

        if let Some(previous) = previous {
            let _ = app.clipboard().write_text(previous);
        }
        Ok(())
    })();
    set_pasting(false);
    result
}

fn emit_copy_fallback(app: &AppHandle, text: &str, message: &str) {
    let _ = app.clipboard().write_text(text.to_string());
    let _ = app.emit(
        "dictation-fallback",
        DictationFallbackPayload {
            text: text.to_string(),
            message: message.to_string(),
        },
    );
}

fn paste_route(app: &AppHandle, text: &str) -> Result<PasteRoute, String> {
    app.clipboard()
        .write_text(text.to_string())
        .map_err(|e| format!("Clipboard write failed: {e}"))?;

    #[cfg(target_os = "macos")]
    {
        let stored = target_app(app).filter(|target| !is_flow_app(&target.name));
        let Some(target) = stored else {
            // No target app — keep text on clipboard; caller turns this into a visible error.
            return Ok(PasteRoute::Retain);
        };

        // Prefer pid activation — more reliable than app name for Electron (Cursor, WhatsApp).
        let activation_error = activate_target_pid(target.pid)
            .or_else(|_| activate_target_by_name(&target.name))
            .err();
        thread::sleep(Duration::from_millis(120));

        let prefers_paste = prefers_paste_first(&target.name);
        let mut ax_error = None;
        let mut paste_error = None;

        if prefers_paste {
            match paste_into_pid(target.pid) {
                Ok(()) => return Ok(PasteRoute::Clipboard),
                Err(err) => paste_error = Some(err),
            }
            match activate_and_paste(&target.name) {
                Ok(()) => return Ok(PasteRoute::Clipboard),
                Err(err) => paste_error = paste_error.or(Some(err)),
            }
            match crate::macos_text::insert_text_into_focused_element(target.pid, text) {
                Ok(()) => return Ok(PasteRoute::Direct),
                Err(err) => ax_error = Some(err),
            }
        } else {
            match crate::macos_text::insert_text_into_focused_element(target.pid, text) {
                Ok(()) => return Ok(PasteRoute::Direct),
                Err(err) => ax_error = Some(err),
            }
            match paste_into_pid(target.pid) {
                Ok(()) => return Ok(PasteRoute::Clipboard),
                Err(err) => paste_error = Some(err),
            }
            match activate_and_paste(&target.name) {
                Ok(()) => return Ok(PasteRoute::Clipboard),
                Err(err) => paste_error = paste_error.or(Some(err)),
            }
        }

        match activate_and_type(&target.name, text) {
            Ok(()) => {
                let _ = app.emit(
                    "dictation-warning",
                    format!(
                        "Used typed fallback for {}. This app did not accept the primary insertion path.",
                        target.name
                    ),
                );
                Ok(PasteRoute::Direct)
            }
            Err(type_err) => {
                let activation_note = activation_error
                    .map(|msg| format!(" Target activation also failed: {msg}"))
                    .unwrap_or_default();
                let ax_note = ax_error
                    .map(|msg| format!(" AX insertion failed: {msg}"))
                    .unwrap_or_default();
                let paste_note = paste_error
                    .map(|msg| format!(" Paste fallback failed: {msg}"))
                    .unwrap_or_default();
                Err(format!(
                    "Couldn't insert into '{}'. Text is on the clipboard (⌘V). {type_err}{activation_note}{ax_note}{paste_note}",
                    target.name,
                ))
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(PasteRoute::Retain)
    }
}


/// Keep the side dock visible above every app without activating Flow.
pub fn show_bubble(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("bubble") {
        let _ = app.run_on_main_thread(move || {
            let _ = window.set_focusable(false);
            // Click-through is owned by Bubble.tsx (idle = through, expanded = interactive).
            // Do not force ignore_cursor_events here — the 2s keepalive would steal Stop/Cancel.
            position_dock_to_cursor_screen(&window);
            let _ = window.show();
            #[cfg(target_os = "macos")]
            {
                configure_dock_overlay(&window);
            }
        });
    }
}

/// Change the bar between its passive idle capsule and interactive recording controls.
/// Native code owns the entire window layout so React never races AppKit.
pub fn set_bubble_expanded(app: &AppHandle, expanded: bool) {
    set_bubble_layout(app, if expanded { "listening" } else { "idle" });
}

/// Keep the dock as small as possible unless the user needs interactive controls.
pub fn set_bubble_layout(app: &AppHandle, layout: &str) {
    if let Some(window) = app.get_webview_window("bubble") {
        let layout = layout.to_string();
        let _ = app.run_on_main_thread(move || {
            let (width, height, interactive) = match layout.as_str() {
                "listening" => (76.0, 304.0, true),
                "fallback" => (112.0, 44.0, true),
                _ => (60.0, 120.0, false),
            };
            let _ = window.set_focusable(false);
            let _ = window.set_ignore_cursor_events(!interactive);
            let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize {
                width,
                height,
            }));
            position_dock_to_cursor_screen(&window);
            let _ = window.show();
            #[cfg(target_os = "macos")]
            configure_dock_overlay(&window);
        });
    }
}

#[cfg(target_os = "macos")]
pub fn configure_dock_overlay(window: &WebviewWindow) {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
    let Ok(ns_window) = window.ns_window() else { return; };
    unsafe {
        let ns_window: &NSWindow = &*ns_window.cast();
        let behavior = (ns_window.collectionBehavior()
            | NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::CanJoinAllApplications
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle)
            & !NSWindowCollectionBehavior::Stationary;
        ns_window.setCollectionBehavior(behavior);
        ns_window.setHidesOnDeactivate(false);
        ns_window.setCanHide(false);
        ns_window.setAlphaValue(1.0);
        ns_window.setLevel(101);
        if !ns_window.isVisible() { ns_window.setIsVisible(true); }
        // Never steal focus during paste — orderFrontRegardless was racing Cmd+V.
        if !is_pasting() {
            ns_window.orderFrontRegardless();
        }
    }
}


/// Dock stays on screen while Flow is running (idle outline). Visibility is not toggled off.
pub fn hide_bubble(app: &AppHandle) {
    // Ensure the dock remains shown after a session ends.
    show_bubble(app);
}

/// Place the dock on the right edge of the monitor containing the active work
/// window. Falls back to cursor/current/primary monitor when AX bounds are absent.
pub fn position_dock_to_cursor_screen(window: &WebviewWindow) {
    let monitors = window
        .available_monitors()
        .ok()
        .unwrap_or_default();
    let frontmost_bounds = frontmost_app()
        .ok()
        .filter(|target| !is_flow_app(&target.name))
        .and_then(|target| front_window_bounds_for_pid(target.pid).ok());
    let cursor = cursor_screen_point();

    let chosen = frontmost_bounds
        .as_ref()
        .and_then(|bounds| monitor_containing_point(&monitors, bounds.center_x, bounds.center_y))
        .or_else(|| cursor.and_then(|(cx, cy)| monitor_containing_point(&monitors, cx, cy)))
        .cloned()
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());

    let Some(monitor) = chosen else {
        return;
    };

    let size = monitor.size();
    let origin = monitor.position();
    let scale = monitor.scale_factor();
    // Prefer the window's current logical size (Bubble expands/collapses).
    let mut width = 60.0;
    let mut height = 120.0;
    if let Ok(outer) = window.outer_size() {
        width = outer.width as f64 / scale;
        height = outer.height as f64 / scale;
    }
    if width < 20.0 || height < 20.0 {
        width = 60.0;
        height = 120.0;
    }
    let x = origin.x as f64 / scale + size.width as f64 / scale - width - 28.0;
    let y = origin.y as f64 / scale + (size.height as f64 / scale - height) / 2.0;
    // Do not reset size here — Bubble owns idle vs expanded dimensions.
    let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
    #[cfg(target_os = "macos")]
    configure_dock_overlay(window);
}

fn monitor_containing_point<'a>(
    monitors: &'a [tauri::Monitor],
    x: f64,
    y: f64,
) -> Option<&'a tauri::Monitor> {
    monitors.iter().find(|m| {
        let pos = m.position();
        let size = m.size();
        let left = pos.x as f64;
        let top = pos.y as f64;
        let right = left + size.width as f64;
        let bottom = top + size.height as f64;
        x >= left && x < right && y >= top && y < bottom
    })
}

#[derive(Clone, Copy)]
struct FrontWindowBounds {
    center_x: f64,
    center_y: f64,
}

fn front_window_bounds_for_pid(pid: i32) -> Result<FrontWindowBounds, String> {
    #[cfg(target_os = "macos")]
    {
        let (x, y, w, h) = crate::macos_text::front_window_bounds(pid)?;
        return Ok(FrontWindowBounds {
            center_x: x + w / 2.0,
            center_y: y + h / 2.0,
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err("Front window bounds are only available on macOS.".into())
    }
}

/// Initial right-edge placement — follows the active work window when possible.
pub fn position_dock_default(window: &WebviewWindow) {
    position_dock_to_cursor_screen(window);
}

/// Global mouse location in top-left-origin physical pixels (Tauri monitor space).
fn cursor_screen_point() -> Option<(f64, f64)> {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSEvent;
        // NSEvent::mouseLocation is bottom-left Cocoa coords across the virtual desktop.
        let cocoa = NSEvent::mouseLocation();
        // NSScreen::screens requires the AppKit main-thread marker (objc2-app-kit 0.3).
        let mtm = MainThreadMarker::new()?;
        let screens = objc2_app_kit::NSScreen::screens(mtm);
        let mut max_height = 0.0_f64;
        for screen in screens.iter() {
            let frame = screen.frame();
            let top = frame.origin.y + frame.size.height;
            if top > max_height {
                max_height = top;
            }
        }
        // Convert Cocoa (bottom-left) → top-left for matching Tauri PhysicalPosition.
        let x = cocoa.x;
        let y = max_height - cocoa.y;
        return Some((x, y));
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn is_flow_app(name: &str) -> bool {
    is_flow_app_name(name)
}

pub fn is_flow_app_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("Flow") || name.eq_ignore_ascii_case("flow-app")
}

pub fn frontmost_app_name() -> Result<String, String> {
    Ok(frontmost_app()?.name)
}

pub fn frontmost_app() -> Result<TargetApp, String> {
    // Prefer NSWorkspace — fast, no AppleScript, works before Accessibility is granted
    // for System Events (still need AX for paste, but capture itself is reliable).
    if let Some(target) = frontmost_app_nsworkspace() {
        return Ok(target);
    }

    let output = Command::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to tell first application process whose frontmost is true to return name & linefeed & (unix id as text)"#,
        ])
        .output()
        .map_err(|e| format!("Could not read frontmost app: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Could not read frontmost app. Grant Accessibility permission to Flow. {stderr}"
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let name = lines.next().unwrap_or("").trim().to_string();
    let pid_raw = lines.next().unwrap_or("").trim();
    if name.is_empty() {
        return Err("Frontmost app name was empty.".into());
    }
    let pid = pid_raw
        .parse::<i32>()
        .map_err(|_| format!("Frontmost app pid was invalid: {pid_raw}"))?;
    Ok(TargetApp { name, pid })
}

#[cfg(target_os = "macos")]
fn frontmost_app_nsworkspace() -> Option<TargetApp> {
    use objc2_app_kit::NSWorkspace;
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    let name = app.localizedName()?.to_string();
    let pid = app.processIdentifier();
    if name.is_empty() || pid <= 0 {
        return None;
    }
    Some(TargetApp {
        name,
        pid: pid as i32,
    })
}

#[cfg(not(target_os = "macos"))]
fn frontmost_app_nsworkspace() -> Option<TargetApp> {
    None
}

fn activate_target_pid(pid: i32) -> Result<(), String> {
    let script = format!(
        r#"tell application "System Events" to set frontmost of (first application process whose unix id is {pid}) to true"#
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("Could not reactivate target app: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Could not reactivate target app by pid {pid}. {stderr}"));
    }
    Ok(())
}

fn activate_target_by_name(app_name: &str) -> Result<(), String> {
    let escaped = escape_applescript_string(app_name);
    let script = format!(r#"tell application "{escaped}" to activate"#);
    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("Could not activate '{app_name}': {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Could not activate '{app_name}'. {stderr}"));
    }
    Ok(())
}

/// Soft-refocus Electron composers (Cursor chat, WhatsApp) by clicking near the
/// bottom-center of the front window, then paste with Cmd+V.
/// Hard AX focus gates are skipped — Cursor often reports no focused field after
/// a long STT round-trip even when the chat input is visible.
fn paste_into_pid(pid: i32) -> Result<(), String> {
    let script = format!(
        r#"
tell application "System Events"
  set frontmost of (first application process whose unix id is {pid}) to true
end tell
delay 0.12
"#
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("Paste by pid failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Could not focus pid {pid}. Grant Accessibility permission to Flow. {stderr}"
        ));
    }

    // Cursor/WhatsApp lose the text-field focus during cloud processing.
    // Click the lower composer area, then send Cmd+V.
    let _ = crate::macos_text::click_composer_area(pid);
    thread::sleep(Duration::from_millis(140));

    if crate::macos_text::has_focused_input_target(pid).is_err() {
        // Second chance click a bit higher (some layouts put the input above the dock).
        let _ = crate::macos_text::click_composer_area_offset(pid, 0.5, 0.82);
        thread::sleep(Duration::from_millis(120));
    }
    crate::macos_text::has_focused_input_target(pid)?;

    if let Err(err) = crate::macos_text::post_command_v() {
        // Fall back to System Events if CGEvent is blocked.
        let paste = Command::new("osascript")
            .args([
                "-e",
                r#"tell application "System Events" to keystroke "v" using command down"#,
            ])
            .output()
            .map_err(|e| format!("Paste keystroke failed: {e}"))?;
        if !paste.status.success() {
            let stderr = String::from_utf8_lossy(&paste.stderr);
            return Err(format!(
                "Paste into pid {pid} failed ({err}). Grant Accessibility. {stderr}"
            ));
        }
    }
    Ok(())
}

fn prefers_paste_first(app_name: &str) -> bool {
    let name = app_name.to_lowercase();
    // Browser chat surfaces and Electron-style chat apps usually accept paste
    // more reliably than AXValue replacement on focused contenteditable fields.
    [
        "edge",
        "chrome",
        "safari",
        "firefox",
        "arc",
        "claude",
        "chatgpt",
        "whatsapp",
        "slack",
        "discord",
        "teams",
        "notion",
        "cursor",
        "code",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

fn activate_and_paste(app_name: &str) -> Result<(), String> {
    let escaped = escape_applescript_string(app_name);
    let script = format!(
        r#"
tell application "{escaped}" to activate
delay 0.12
"#
    );

    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("Paste failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Activate '{app_name}' failed. Grant Accessibility permission to Flow. {stderr}"
        ));
    }

    if let Ok(target) = frontmost_app() {
        if !is_flow_app(&target.name) {
            let _ = crate::macos_text::click_composer_area(target.pid);
            thread::sleep(Duration::from_millis(120));
        }
    }

    if crate::macos_text::post_command_v().is_err() {
        let paste = Command::new("osascript")
            .args([
                "-e",
                r#"tell application "System Events" to keystroke "v" using command down"#,
            ])
            .output()
            .map_err(|e| format!("Paste keystroke failed: {e}"))?;
        if !paste.status.success() {
            let stderr = String::from_utf8_lossy(&paste.stderr);
            return Err(format!(
                "Paste into '{app_name}' failed. Text is on the clipboard (⌘V). {stderr}"
            ));
        }
    }

    Ok(())
}

fn activate_and_type(app_name: &str, text: &str) -> Result<(), String> {
    let escaped = escape_applescript_string(app_name);
    let chunks = typing_chunks(text, 40);
    let mut script = format!("tell application \"{escaped}\" to activate\ndelay 0.08\n");
    script.push_str("tell application \"System Events\"\n");
    for chunk in chunks {
        match chunk {
            TypingChunk::Text(value) => {
                let value = escape_applescript_string(&value);
                script.push_str(&format!("keystroke \"{value}\"\n"));
            }
            TypingChunk::Return => script.push_str("key code 36\n"),
            TypingChunk::Tab => script.push_str("key code 48\n"),
        }
    }
    script.push_str("end tell\n");

    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("Typed fallback failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Typed fallback failed for '{app_name}'. {stderr}"));
    }

    Ok(())
}

enum TypingChunk {
    Text(String),
    Return,
    Tab,
}

fn typing_chunks(text: &str, max_chunk_len: usize) -> Vec<TypingChunk> {
    let mut out = Vec::new();
    let mut current = String::new();

    let flush = |out: &mut Vec<TypingChunk>, current: &mut String| {
        if !current.is_empty() {
            out.push(TypingChunk::Text(std::mem::take(current)));
        }
    };

    for ch in text.chars() {
        match ch {
            '\n' => {
                flush(&mut out, &mut current);
                out.push(TypingChunk::Return);
            }
            '\t' => {
                flush(&mut out, &mut current);
                out.push(TypingChunk::Tab);
            }
            _ => {
                current.push(ch);
                if current.chars().count() >= max_chunk_len {
                    flush(&mut out, &mut current);
                }
            }
        }
    }
    flush(&mut out, &mut current);
    out
}

fn escape_applescript_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
