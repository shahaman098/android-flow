mod cloud_api;
mod config;
mod dictate;
mod focus;
#[cfg(target_os = "macos")]
mod macos_text;
#[cfg(target_os = "macos")]
mod macos_fn;
mod meeting;
mod store;
mod stt_gcp;
#[cfg(target_os = "macos")]
mod system_audio;
mod vibe_context;

use serde::Serialize;
use focus::{
    clear_selected_text, hide_bubble, show_bubble, DictationTarget, RecordingFlag,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    ActivationPolicy, Emitter, Manager, WindowEvent,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(DictationTarget::default())
        .manage(RecordingFlag::default())
        .manage(meeting::MeetingSession::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(
            // Keep Control+1/2 as optional fallbacks; primary hotkey is fn (see macos_fn).
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    let vibe = Shortcut::new(Some(Modifiers::CONTROL), Code::Digit1);
                    let refine = Shortcut::new(Some(Modifiers::CONTROL), Code::Digit2);
                    if shortcut == &vibe {
                        // `hands_free` = tap to start, tap again to stop. Off = hold.
                        // This is the only path the setting applies to; the primary fn
                        // hotkey is inherently hold-style (see macos_fn).
                        let hands_free = config::load_config().hands_free;
                        handle_toggle_shortcut(app, event.state, hands_free, "vibe");
                    } else if shortcut == &refine {
                        if event.state == ShortcutState::Pressed {
                            handle_vibe_refine(app);
                        }
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            dictate::get_config,
            dictate::save_user_config,
            dictate::get_readiness,
            dictate::verify_stt_connection,
            dictate::verify_llm_connection,
            dictate::import_deepseek_from_gcloud,
            dictate::process_dictation,
            dictate::hide_bubble_window,
            dictate::transcribe_partial,
            hub_vibe_press,
            hub_vibe_release,
            hide_hub_window,
            open_accessibility_settings,
            open_microphone_settings,
            quit_wispr_flow_conflict,
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
            // Regular (not Accessory) so Flow appears in the Dock / Cmd+Tab with a real Hub window.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            let show = MenuItem::with_id(app, "show", "Open Hub", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .tooltip("Flow — hold fn activate · fn+1 auto-prompt · fn+2 refine")
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

            let _ = app
                .global_shortcut()
                .register(Shortcut::new(Some(Modifiers::CONTROL), Code::Digit1));
            let _ = app
                .global_shortcut()
                .register(Shortcut::new(Some(Modifiers::CONTROL), Code::Digit2));

            // Install the macOS fn listener. If Accessibility is missing, surface a
            // diagnostic in the Hub instead of bouncing the user into Settings on every launch.
            #[cfg(target_os = "macos")]
            {
                if let Err(e) = macos_fn::install(app.handle()) {
                    let _ = app.emit("dictation-error", e);
                }
            }

            // Background poll for known call apps (Zoom, Teams, Slack, Discord, WhatsApp,
            // FaceTime) — only ever gates the Hub's meeting-transcription banner, never starts
            // capture on its own.
            #[cfg(target_os = "macos")]
            meeting::start_detection_loop(app.handle().clone());

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

            focus::show_bubble(app.handle());

            #[cfg(target_os = "macos")]
            {
                let dock_app = app.handle().clone();
                std::thread::spawn(move || {
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        focus::show_bubble(&dock_app);
                    }
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Flow");
}

/// Start mic session for `dictate` (activate) or `vibe` (auto-prompt).
pub fn begin_recording_session(app: &tauri::AppHandle, mode: &str) -> Result<(), String> {
    clear_selected_text(app);
    show_bubble(app);

    // Open the mic before anything slow. Resolving the frontmost app is an osascript
    // round-trip (100-300ms) and used to run ahead of this emit — on the main thread,
    // from the fn key-down handler — so every session lost the first word or two. The
    // target app is not needed until paste time, seconds later, so it resolves below
    // while the recorder is already warming up.
    app.emit("recording-start", mode)
        .map_err(|e| format!("Failed to start recording UI: {e}"))?;

    let handle = app.clone();
    std::thread::spawn(move || {
        let (message, show_hub) = match focus::capture_frontmost_app(&handle) {
            Ok(name) if focus::is_flow_app_name(&name) => (
                Some("Recording from Flow Hub — result stays on clipboard unless you clicked another app first.".to_string()),
                true,
            ),
            Ok(name) => {
                let _ = handle.emit("dictation-status", format!("recording → {name}"));
                (None, false)
            }
            Err(err) => (
                Some(format!("{err} Recording anyway — grant Accessibility if paste fails.")),
                true,
            ),
        };

        if let Some(message) = message {
            let _ = handle.emit("dictation-warning", message);
        }
        if show_hub {
            let hub = handle.clone();
            let _ = handle.run_on_main_thread(move || show_hub_window(&hub));
        }
    });

    Ok(())
}

pub fn stop_recording_session(app: &tauri::AppHandle, mode: &str) {
    let _ = app.emit("recording-stop", mode);
}

fn begin_vibe_session(app: &tauri::AppHandle) -> Result<(), String> {
    begin_recording_session(app, "vibe")
}

fn handle_toggle_shortcut(
    app: &tauri::AppHandle,
    state: ShortcutState,
    hands_free: bool,
    mode: &str,
) {
    let flag = app.state::<RecordingFlag>();
    let mut active = match flag.active.lock() {
        Ok(g) => g,
        Err(_) => {
            let _ = app.emit(
                "dictation-error",
                "Internal error: recording lock poisoned. Restart Flow.",
            );
            return;
        }
    };

    match state {
        ShortcutState::Pressed => {
            if hands_free {
                if *active {
                    *active = false;
                    let _ = app.emit("recording-stop", mode);
                } else if let Err(err) = begin_vibe_session(app) {
                    *active = false;
                    hide_bubble(app);
                    let _ = app.emit("dictation-error", err);
                } else {
                    *active = true;
                }
            } else if !*active {
                if let Err(err) = begin_vibe_session(app) {
                    *active = false;
                    hide_bubble(app);
                    let _ = app.emit("dictation-error", err);
                } else {
                    *active = true;
                }
            }
        }
        ShortcutState::Released => {
            if !hands_free && *active {
                *active = false;
                let _ = app.emit("recording-stop", mode);
            }
        }
    }
}

/// fn+2 / Control+2: no mic — select entire prompt, extract context/, refine, paste.
pub fn handle_vibe_refine(app: &tauri::AppHandle) {
    let flag = app.state::<RecordingFlag>();
    if let Ok(active) = flag.active.lock() {
        if *active {
            let _ = app.emit(
                "dictation-error",
                "Finish or cancel the fn recording before using fn+2 refine.",
            );
            return;
        }
    }

    show_bubble(app);
    let _ = app.emit("session-mode", "vibe_refine");
    let _ = app.emit("dictation-status", "correcting");

    match dictate::prepare_vibe_refine(app) {
        Ok(text) => {
            let _ = app.emit("partial-transcript", &text);
            let app_handle = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = dictate::process_vibe_refine(app_handle.clone()).await {
                    hide_bubble(&app_handle);
                    let _ = app_handle.emit("dictation-error", err);
                }
            });
        }
        Err(err) => {
            hide_bubble(app);
            let _ = app.emit("dictation-error", err);
        }
    }
}

fn show_hub_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
fn hide_hub_window(app: tauri::AppHandle) {
    focus::show_bubble(&app);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

/// Hub "Hold to record" button — same path as Control+1 press.
#[tauri::command]
fn hub_vibe_press(app: tauri::AppHandle) -> Result<(), String> {
    handle_toggle_shortcut(&app, ShortcutState::Pressed, false, "vibe");
    Ok(())
}

/// Hub "Hold to record" button — same path as Control+1 release.
#[tauri::command]
fn hub_vibe_release(app: tauri::AppHandle) -> Result<(), String> {
    handle_toggle_shortcut(&app, ShortcutState::Released, false, "vibe");
    Ok(())
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

/// Kill commercial Wispr Flow which steals mic/hotkeys from our Flow.app.
#[tauri::command]
fn quit_wispr_flow_conflict() -> Result<String, String> {
    let status = std::process::Command::new("pkill")
        .args(["-f", "/Applications/Wispr Flow.app"])
        .status()
        .map_err(|e| format!("pkill failed: {e}"))?;
    if status.success() {
        Ok("Quit commercial Wispr Flow. Try hold fn again.".into())
    } else {
        Ok("Commercial Wispr Flow was not running.".into())
    }
}
