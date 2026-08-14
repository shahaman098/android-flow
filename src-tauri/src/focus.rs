use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Manager, WebviewWindow};
use tauri_plugin_clipboard_manager::ClipboardExt;

/// True while paste is in progress — dock must not call orderFrontRegardless.
static PASTING: AtomicBool = AtomicBool::new(false);
static LAST_DOCK_PID: AtomicI32 = AtomicI32::new(0);

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

pub fn capture_frontmost_app(app: &AppHandle) -> Result<String, String> {
    let target = frontmost_app()?;
    let state = app.state::<DictationTarget>();
    let mut guard = state
        .app
        .lock()
        .map_err(|_| "Failed to lock target app".to_string())?;

    // If Hub/Dock is focused, keep the previous target app instead of aborting.
    // Blocking here made fn+1 look completely dead while Flow Hub was open.
    if is_flow_app(&target.name) {
        if let Some(prev) = guard.clone() {
            return Ok(prev.name);
        }
        return Err(
            "Flow is frontmost and no target app was captured. Click the text field you want Flow to write into, then try again."
                .into(),
        );
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

pub fn take_selected_text(app: &AppHandle) -> Option<String> {
    await_selection_capture();
    let state = app.state::<DictationTarget>();
    state.selected_text.lock().ok().and_then(|mut g| g.take())
}

/// Set while the ⌘C that runs alongside a starting recording is still in flight.
static CAPTURING_SELECTION: AtomicBool = AtomicBool::new(false);

pub fn mark_selection_capture_started() {
    CAPTURING_SELECTION.store(true, Ordering::SeqCst);
}

pub fn mark_selection_capture_finished() {
    CAPTURING_SELECTION.store(false, Ordering::SeqCst);
}

/// Block until the in-flight selection capture lands.
///
/// The capture runs off the main thread so it cannot delay opening the microphone, which
/// means a very short dictation could otherwise reach processing first and see no selection
/// — silently turning a spoken edit into an insert. Reading is seconds behind the capture in
/// practice, so this almost never actually waits.
fn await_selection_capture() {
    let deadline = std::time::Instant::now() + Duration::from_millis(800);
    while CAPTURING_SELECTION.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
}

/// Copy the current selection only (no select-all). Empty / unchanged clipboard means no range.
pub fn capture_current_selection(app: &AppHandle) -> Option<String> {
    let previous = app.clipboard().read_text().ok();

    #[cfg(target_os = "macos")]
    {
        let status = Command::new("osascript")
            .args([
                "-e",
                r#"tell application "System Events" to keystroke "c" using command down"#,
            ])
            .status();
        if !status.map(|s| s.success()).unwrap_or(false) {
            return None;
        }
        thread::sleep(Duration::from_millis(80));
    }

    let copied = app.clipboard().read_text().ok().unwrap_or_default();
    if let Some(previous) = previous.as_deref() {
        let _ = app.clipboard().write_text(previous);
    }

    let selected = copied.trim().to_string();
    if selected.is_empty() || previous.as_deref().is_some_and(|prev| selected == prev.trim()) {
        clear_selected_text(app);
        return None;
    }

    if let Ok(mut guard) = app.state::<DictationTarget>().selected_text.lock() {
        *guard = Some(selected.clone());
    }
    Some(selected)
}

/// Text-transform shortcuts: prefer an existing selection (so fn+1 does not Cmd+A a
/// whole Cursor thread), otherwise select the entire focused field (⌘A) and copy it.
pub fn capture_field_text_for_transform(app: &AppHandle) -> Result<String, String> {
    if let Some(selected) = capture_current_selection(app) {
        let selected = selected.trim().to_string();
        if !selected.is_empty() {
            let state = app.state::<DictationTarget>();
            *state
                .selected_text
                .lock()
                .map_err(|_| "Failed to lock selected text".to_string())? = Some(selected.clone());
            return Ok(selected);
        }
    }
    select_all_and_capture(app)
}

/// Text-transform shortcuts: select the entire focused field/document (⌘A) then copy it.
pub fn select_all_and_capture(app: &AppHandle) -> Result<String, String> {
    // Read only so the clipboard can be put back afterwards. `read_text` errors whenever the
    // pasteboard holds something that is not text — an image, a file, a fresh login — so
    // failing the whole transform here meant fn+1 died before it touched the field, with
    // nothing pasted, purely because of what was copied last. Nothing to restore is fine.
    let previous = app.clipboard().read_text().ok();

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
            return Err("Could not select/copy the prompt. Grant Accessibility permission.".into());
        }
        thread::sleep(Duration::from_millis(100));
    }

    let selected = app
        .clipboard()
        .read_text()
        .map_err(|e| format!("Clipboard read failed: {e}"))?;

    if let Some(previous) = previous {
        app.clipboard()
            .write_text(previous)
            .map_err(|e| format!("Clipboard restore failed: {e}"))?;
    }

    let selected = selected.trim().to_string();
    if selected.is_empty() {
        return Err(
            "No text found after select-all. Focus the field you want Flow to transform, then press the shortcut again."
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

/// How the text reached the target app — decides how long to wait before the clipboard
/// can be put back.
enum PasteRoute {
    /// Inserted directly via AX; the clipboard was never consumed.
    Direct,
    /// Cmd+V was dispatched; the target still needs a moment to consume it.
    Clipboard,
}

pub fn paste_into_target_app(app: &AppHandle, text: &str) -> Result<(), String> {
    set_pasting(true);
    let result = (|| {
        // Snapshot first: the paste route below clobbers the clipboard.
        let previous = app.clipboard().read_text().ok();

        let route = paste_route(app, text);
        // The clipboard is scratch space for Cmd+V, never a place to leave the result:
        // restore it on success and failure alike.
        let settle = match route {
            Ok(PasteRoute::Clipboard) => Duration::from_millis(900),
            Ok(PasteRoute::Direct) => Duration::from_millis(80),
            Err(_) => Duration::from_millis(0),
        };
        thread::sleep(settle);
        if let Some(previous) = previous {
            let _ = app.clipboard().write_text(previous);
        }
        route.map(|_| ())
    })();
    set_pasting(false);
    result
}

fn paste_route(app: &AppHandle, text: &str) -> Result<PasteRoute, String> {
    app.clipboard()
        .write_text(text.to_string())
        .map_err(|e| format!("Clipboard write failed: {e}"))?;

    #[cfg(target_os = "macos")]
    {
        let stored = target_app(app).filter(|target| !is_flow_app(&target.name));
        let Some(target) = stored else {
            return Err(
                "No target app captured. Click the text field you want Flow to write into, then try again."
                    .into(),
            );
        };

        activate_target_pid(target.pid)?;
        thread::sleep(Duration::from_millis(120));

        // One insertion path per app, chosen by which one that app actually honours.
        if prefers_paste_first(&target.name) {
            paste_into_pid(target.pid)?;
            return Ok(PasteRoute::Clipboard);
        }
        crate::macos_text::insert_text_into_focused_element(target.pid, text)?;
        Ok(PasteRoute::Direct)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, text);
        Err("Pasting is only implemented on macOS.".into())
    }
}

/// Periodic keepalive: cheap show when the frontmost app is unchanged, full
/// AX reposition only when the user switches apps.
pub fn keep_bubble_visible(app: &AppHandle) {
    if is_pasting() {
        return;
    }
    let pid = frontmost_app()
        .ok()
        .filter(|target| !is_flow_app(&target.name))
        .map(|target| target.pid)
        .unwrap_or(0);
    let last = LAST_DOCK_PID.swap(pid, Ordering::Relaxed);
    if pid != 0 && pid == last {
        if let Some(window) = app.get_webview_window("bubble") {
            let _ = app.run_on_main_thread(move || {
                let _ = window.show();
            });
        }
        return;
    }
    show_bubble(app);
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
                "listening" => (64.0, 232.0, true),
                _ => (50.0, 100.0, false),
            };
            let _ = window.set_focusable(false);
            let _ = window.set_ignore_cursor_events(!interactive);
            let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize { width, height }));
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
    let Ok(ns_window) = window.ns_window() else {
        return;
    };
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
        if !ns_window.isVisible() {
            ns_window.setIsVisible(true);
        }
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
    let monitors = window.available_monitors().ok().unwrap_or_default();
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
    let mut width = 50.0;
    let mut height = 100.0;
    if let Ok(outer) = window.outer_size() {
        width = outer.width as f64 / scale;
        height = outer.height as f64 / scale;
    }
    if width < 20.0 || height < 20.0 {
        width = 50.0;
        height = 100.0;
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

pub fn frontmost_app() -> Result<TargetApp, String> {
    // NSWorkspace only — fast, no AppleScript, and works before Accessibility is granted
    // for System Events.
    frontmost_app_nsworkspace()
        .ok_or_else(|| "Could not read the frontmost app from NSWorkspace.".to_string())
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
        return Err(format!(
            "Could not reactivate target app by pid {pid}. {stderr}"
        ));
    }
    Ok(())
}

/// Soft-refocus Electron composers (Cursor chat, WhatsApp) by clicking near the
/// bottom-center of the front window, then paste with Cmd+V.
///
/// Do not hard-fail on AX focused-field checks. Cursor / WhatsApp / Claude often
/// report no focused input after a long LLM round-trip (fn+1 / fn+2) even when the
/// composer is visible — that was making correction look completely broken.
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

    // Cursor/WhatsApp lose the text-field focus during LLM processing.
    // Click the lower composer area, then send Cmd+V regardless of AX focus.
    let _ = crate::macos_text::click_composer_area(pid);
    thread::sleep(Duration::from_millis(140));

    if crate::macos_text::has_focused_input_target(pid).is_err() {
        // Second chance click a bit higher (some layouts put the input above the dock).
        let _ = crate::macos_text::click_composer_area_offset(pid, 0.5, 0.82);
        thread::sleep(Duration::from_millis(120));
    }

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
        "edge", "chrome", "safari", "firefox", "arc", "claude", "chatgpt", "whatsapp", "slack",
        "discord", "teams", "notion", "cursor", "code",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}
