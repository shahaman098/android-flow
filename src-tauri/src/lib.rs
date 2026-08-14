mod cloud_api;
mod config;
mod dictate;
mod dictation_post;
mod focus;
#[cfg(target_os = "macos")]
mod macos_fn;
#[cfg(target_os = "macos")]
mod macos_text;
mod meeting;
mod store;
mod training;
mod stt_gcp;
mod stt_groq;
mod stt_local;
mod stt_whisper;
mod stt_whisper_server;
#[cfg(target_os = "macos")]
mod system_audio;
mod vibe_context;

use focus::{hide_bubble, show_bubble, DictationTarget, RecordingFlag};
use serde::Serialize;
#[cfg(target_os = "macos")]
use std::fs::File;
#[cfg(target_os = "macos")]
use std::os::fd::AsRawFd;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    ActivationPolicy, Emitter, Manager, WindowEvent,
};
use tauri_plugin_clipboard_manager::ClipboardExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "macos")]
    let _instance_lock = match acquire_instance_lock() {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            // Launch Services normally reopens the existing app. Direct executable
            // launches do not, so avoid creating a second hotkey/mic engine.
            return;
        }
        Err(err) => {
            eprintln!("Flow instance lock failed: {err}");
            return;
        }
    };

    let app = tauri::Builder::default()
        .manage(DictationTarget::default())
        .manage(RecordingFlag::default())
        .manage(meeting::MeetingSession::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            dictate::get_config,
            dictate::get_ui_prefs,
            dictate::save_user_config,
            dictate::get_readiness,
            dictate::verify_stt_connection,
            dictate::verify_llm_connection,
            dictate::import_deepseek_from_gcloud,
            dictate::process_dictation,
            dictate::process_transcript,
            dictate::hide_bubble_window,
            dictate::transcribe_partial,
            flow_bar_stop,
            flow_bar_cancel,
            reset_recording_session,
            set_bubble_expanded,
            set_bubble_layout,
            copy_text_to_clipboard,
            log_dictation_error,
            hide_hub_window,
            open_accessibility_settings,
            open_microphone_settings,
            check_accessibility_trusted,
            get_accessibility_status,
            store::get_store,
            store::add_dictionary_word,
            store::remove_dictionary_word,
            store::add_snippet,
            store::remove_snippet,
            store::upsert_style,
            store::clear_history,
            store::remove_meeting_transcript,
            training::get_training_stats,
            training::open_training_folder,
            store::clear_meetings,
            meeting::meeting_get_status,
            meeting::meeting_confirm_notified,
            meeting::meeting_start_capture,
            meeting::meeting_stop_capture,
            meeting::meeting_submit_mic_chunk,
            meeting::check_screen_recording_trusted,
            meeting::get_screen_recording_status,
            meeting::open_screen_recording_settings
        ])
        .setup(|app| {
            // Accessory: no Dock icon; the tray + Hub window are the UI.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            let show = MenuItem::with_id(app, "show", "Open Hub", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .tooltip("Flow — fn raw · fn+1 prompt · fn+2 correct · fn+3 answer")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "show" => show_hub_window(app),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_hub_window(tray.app_handle());
                    }
                })
                .build(app)?;

            // Install the macOS fn listener. If Accessibility is missing, surface a
            // diagnostic in the Hub instead of bouncing the user into Settings on every launch.
            #[cfg(target_os = "macos")]
            {
                if let Err(e) = macos_fn::install(app.handle()) {
                    let _ = app.emit("dictation-error", e);
                }
            }

            prewarm_local_processing();

            if let Some(window) = app.get_webview_window("main") {
                let window_clone = window.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        focus::show_bubble(window_clone.app_handle());
                        let _ = window_clone.hide();
                    }
                });
                show_hub_window(app.handle());
            }

            focus::set_bubble_expanded(app.handle(), false);

            #[cfg(target_os = "macos")]
            {
                let dock_app = app.handle().clone();
                std::thread::spawn(move || {
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        // Skip while pasting so the dock cannot steal Cmd+V focus.
                        if focus::is_pasting() {
                            continue;
                        }
                        focus::keep_bubble_visible(&dock_app);
                    }
                });
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Flow");

    app.run(|handle, event| {
        // The resident Whisper server holds ~500MB; do not leave it orphaned.
        if let tauri::RunEvent::Exit = event {
            stt_whisper_server::shutdown();
        }
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            show_hub_window(handle);
            focus::show_bubble(handle);
        }
    });
}

#[cfg(target_os = "macos")]
fn acquire_instance_lock() -> Result<Option<File>, String> {
    let path = std::env::temp_dir().join(format!("com.efi.voiceflow.{}.lock", unsafe {
        libc::getuid()
    }));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        Ok(Some(file))
    } else if std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock {
        Ok(None)
    } else {
        Err(format!(
            "Could not lock {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ))
    }
}

fn prewarm_local_processing() {
    let cfg = config::load_config();
    if cloud_api::uses_cloud(&cfg) {
        return;
    }
    // Load the Whisper model during startup rather than on the user's first press.
    if let Some(model) = stt_whisper::prewarm_model_path(&cfg) {
        stt_whisper_server::prewarm(model, stt_whisper::WHISPER_THREADS);
    }
    std::thread::spawn(|| {
        let _ = stt_gcp::adc_access_token();
    });
}

/// Start mic session for raw `dictate`.
pub fn begin_recording_session(app: &tauri::AppHandle, mode: &str) -> Result<(), String> {
    // Capture the typing target BEFORE the Flow Bar can become frontmost.
    // NSWorkspace path is fast (~ms); do not open Hub here — that stole focus and
    // made paste land on clipboard-only.
    // No target app means nothing can receive the result, so refuse the session rather than
    // recording speech that has nowhere to land.
    let target = focus::capture_frontmost_app(app)?;
    let _ = app.emit("dictation-status", format!("recording → {target}"));

    hide_hub_for_work(app);
    show_bubble(app);

    // Open the mic before anything that blocks. Capturing the selection first cost ~300ms of
    // `osascript` on the main thread, and every one of those milliseconds was speech the
    // recorder never heard — that is what chopped the first word off dictations.
    app.emit("recording-start", mode)
        .map_err(|e| format!("Failed to start recording UI: {e}"))?;

    // Copy any already-selected range for spoken edit. No select-all — empty means insert.
    // Only read at process time (after STT), so it has seconds to finish off-thread. Clear
    // it first so a slow capture can never let the previous session's selection be reused.
    focus::clear_selected_text(app);
    focus::mark_selection_capture_started();
    let selection_handle = app.clone();
    std::thread::spawn(move || {
        let _ = focus::capture_current_selection(&selection_handle);
        focus::mark_selection_capture_finished();
    });

    Ok(())
}

pub fn stop_recording_session(app: &tauri::AppHandle, mode: &str) {
    let _ = app.emit("recording-stop", mode);
}

/// fn+1: no mic — select rough text, extract context/, create a Vibe Coding prompt, paste.
pub fn handle_vibe_prompt(app: &tauri::AppHandle) {
    let flag = app.state::<RecordingFlag>();
    if let Ok(active) = flag.active.lock() {
        if *active {
            let _ = app.emit(
                "dictation-error",
                "Finish or cancel the fn recording before using fn+1 prompt.",
            );
            return;
        }
    }

    show_bubble(app);
    hide_hub_for_work(app);
    let _ = app.emit("session-mode", "vibe_text");
    let _ = app.emit("dictation-status", "fn+1 prompt");

    run_text_transform(app, dictate::prepare_vibe_text, |app| {
        Box::pin(dictate::process_vibe_text(app))
    });
}

/// fn+2: no mic — select text, grammar-correct it, paste.
pub fn handle_correct_text(app: &tauri::AppHandle) {
    let flag = app.state::<RecordingFlag>();
    if let Ok(active) = flag.active.lock() {
        if *active {
            let _ = app.emit(
                "dictation-error",
                "Finish or cancel the fn recording before using fn+2 correction.",
            );
            return;
        }
    }

    show_bubble(app);
    hide_hub_for_work(app);
    let _ = app.emit("session-mode", "correct_text");
    let _ = app.emit("dictation-status", "correcting");

    run_text_transform(app, dictate::prepare_correct_text, |app| {
        Box::pin(dictate::process_correct_text(app))
    });
}

/// Shared body of the fn+1 / fn+2 text chords: capture the focused field, then transform it.
///
/// `prepare` drives ⌘A/⌘C through `osascript` and sleeps ~150ms. Running that on the main
/// thread stalled the very event loop the fn tap dispatches through, so a chord pressed
/// while a previous one was still capturing was dropped entirely.
fn run_text_transform<P, S>(app: &tauri::AppHandle, prepare: P, start: S)
where
    P: FnOnce(&tauri::AppHandle) -> Result<String, String> + Send + 'static,
    S: FnOnce(
            tauri::AppHandle,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>
        + Send
        + 'static,
{
    let app_handle = app.clone();
    std::thread::spawn(move || match prepare(&app_handle) {
        Ok(text) => {
            let _ = app_handle.emit("partial-transcript", &text);
            let run_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = start(run_handle.clone()).await {
                    hide_bubble(&run_handle);
                    let _ = run_handle.emit("dictation-error", err);
                }
            });
        }
        Err(err) => {
            hide_bubble(&app_handle);
            let _ = app_handle.emit("dictation-error", err);
        }
    });
}

/// fn+3: no mic — answer the latest question detected in the live conversation transcript.
pub fn handle_meeting_answer(app: &tauri::AppHandle) {
    let flag = app.state::<RecordingFlag>();
    if let Ok(active) = flag.active.lock() {
        if *active {
            let _ = app.emit(
                "dictation-error",
                "Finish or cancel the fn recording before using fn+3 live answer.",
            );
            return;
        }
    }

    show_bubble(app);
    hide_hub_for_work(app);
    let _ = app.emit("session-mode", "meeting_answer");
    let _ = app.emit("dictation-status", "answering");

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = dictate::process_meeting_answer(app_handle.clone()).await {
            hide_bubble(&app_handle);
            let _ = app_handle.emit("dictation-error", err);
        }
    });
}

fn show_hub_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn hide_hub_for_work(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

#[tauri::command]
fn hide_hub_window(app: tauri::AppHandle) {
    focus::show_bubble(&app);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

/// Flow Bar Stop — finish capture and process (same as releasing fn).
#[tauri::command]
fn flow_bar_stop(app: tauri::AppHandle, mode: String) -> Result<(), String> {
    let flag = app.state::<RecordingFlag>();
    let should_stop = flag
        .active
        .lock()
        .map(|mut active| {
            let was_active = *active;
            *active = false;
            was_active
        })
        .unwrap_or(false);
    if !should_stop {
        return Ok(());
    }
    let session_mode = if mode.is_empty() {
        "dictate".to_string()
    } else {
        mode
    };
    stop_recording_session(&app, &session_mode);
    Ok(())
}

/// Flow Bar Cancel — discard the in-progress recording with no paste.
#[tauri::command]
fn flow_bar_cancel(app: tauri::AppHandle) -> Result<(), String> {
    let flag = app.state::<RecordingFlag>();
    let should_cancel = flag
        .active
        .lock()
        .map(|mut active| {
            let was_active = *active;
            *active = false;
            was_active
        })
        .unwrap_or(false);
    if !should_cancel {
        return Ok(());
    }
    let _ = app.emit("recording-cancel", "bar");
    Ok(())
}

#[tauri::command]
fn reset_recording_session(app: tauri::AppHandle) -> Result<(), String> {
    let flag = app.state::<RecordingFlag>();
    let mut active = flag
        .active
        .lock()
        .map_err(|_| "Internal error: recording lock poisoned. Restart Flow.".to_string())?;
    *active = false;
    Ok(())
}

#[tauri::command]
fn set_bubble_expanded(app: tauri::AppHandle, expanded: bool) -> Result<(), String> {
    focus::set_bubble_expanded(&app, expanded);
    Ok(())
}

#[tauri::command]
fn set_bubble_layout(app: tauri::AppHandle, layout: String) -> Result<(), String> {
    focus::set_bubble_layout(&app, &layout);
    Ok(())
}

#[tauri::command]
fn copy_text_to_clipboard(app: tauri::AppHandle, text: String) -> Result<(), String> {
    app.clipboard()
        .write_text(text)
        .map_err(|e| format!("Clipboard write failed: {e}"))
}

#[tauri::command]
fn log_dictation_error(message: String) -> Result<(), String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = crate::config::data_dir()?.join("last-error.log");
    crate::config::atomic_write_private(&path, &format!("{timestamp} {message}\n"))
}

#[tauri::command]
fn open_accessibility_settings() -> Result<(), String> {
    std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn()
        .map_err(|e| format!("Could not open Accessibility settings: {e}"))?;
    Ok(())
}

#[tauri::command]
fn open_microphone_settings() -> Result<(), String> {
    std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
        .spawn()
        .map_err(|e| format!("Could not open Microphone settings: {e}"))?;
    Ok(())
}

#[tauri::command]
fn check_accessibility_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXIsProcessTrusted() -> bool;
        }
        unsafe { AXIsProcessTrusted() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[derive(Debug, Serialize)]
struct AccessibilityStatus {
    trusted: bool,
    executable_path: String,
    app_bundle_path: Option<String>,
    guidance: Option<String>,
}

#[tauri::command]
fn get_accessibility_status() -> AccessibilityStatus {
    let executable_path = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "[unavailable]".into());
    let app_bundle_path = std::env::current_exe()
        .ok()
        .and_then(|path| current_app_bundle_path(&path))
        .map(|p| p.display().to_string());
    let trusted = check_accessibility_trusted();

    let guidance = if trusted {
        None
    } else {
        let running_target = app_bundle_path.as_deref().unwrap_or(&executable_path);
        Some(format!(
            "macOS stores Accessibility approval per app binary/path. This session is running from {}. If you previously enabled a different Flow.app, that approval will not apply here. Approve this exact app, or launch one stable bundled copy and keep using that same bundle.",
            running_target
        ))
    };

    AccessibilityStatus {
        trusted,
        executable_path,
        app_bundle_path,
        guidance,
    }
}

fn current_app_bundle_path(path: &std::path::Path) -> Option<std::path::PathBuf> {
    for ancestor in path.ancestors() {
        if ancestor
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
        {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}
