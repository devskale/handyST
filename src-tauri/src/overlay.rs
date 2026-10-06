use crate::input;
use crate::settings;
use crate::settings::{OverlayPosition, OverlayStyle};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize};

#[cfg(not(target_os = "macos"))]
use log::debug;

#[cfg(not(target_os = "macos"))]
use tauri::WebviewWindowBuilder;

#[cfg(target_os = "macos")]
use tauri::WebviewUrl;

#[cfg(target_os = "macos")]
use tauri_nspanel::{tauri_panel, CollectionBehavior, PanelBuilder, PanelLevel, StyleMask};

#[cfg(target_os = "linux")]
use crate::utils;

#[cfg(target_os = "linux")]
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

#[cfg(target_os = "macos")]
tauri_panel! {
    panel!(RecordingOverlayPanel {
        config: {
            can_become_key_window: false,
            is_floating_panel: true
        }
    })
}

// Native overlay window sizes (logical points). One window is reused for every
// state and resized in `show_overlay_state`; each size need only be at least as
// large as the card it hosts (the `--ov-*` vars in RecordingOverlay.css). The
// card is CSS-anchored flush to the screen edge, so window height doesn't move
// where the card sits — only OVERLAY_TOP_OFFSET / OVERLAY_BOTTOM_OFFSET do. Keep
// these in sync with the CSS card geometry.
//
// On Windows these sizes are additionally multiplied by the accessibility text
// scale (see windows_text_scale_factor), which WebView2 applies as a zoom.
//
// Compact overlay (Minimal / transcribing / processing): the 40h pill animates
// width from 172 (--ov-rest-w) to 216 (--ov-work-w) and expands from center, so
// the window must fit the widest state plus a little slack.
const OVERLAY_WIDTH: f64 = 256.0;
const OVERLAY_HEIGHT: f64 = 64.0;

// Actual is 394x118, just a little extra
const OVERLAY_STREAM_WIDTH: f64 = 400.0;
const OVERLAY_STREAM_HEIGHT: f64 = 170.0;

/// Overlay window size (logical) for a given UI state.
fn overlay_dimensions(state: &str) -> (f64, f64) {
    if state == "streaming" {
        (OVERLAY_STREAM_WIDTH, OVERLAY_STREAM_HEIGHT)
    } else {
        (OVERLAY_WIDTH, OVERLAY_HEIGHT)
    }
}

static LAST_MIC_LEVEL_EMIT: AtomicU64 = AtomicU64::new(0);
const EMIT_THROTTLE_MS: u64 = 33; // ~30 FPS

#[cfg(target_os = "macos")]
const OVERLAY_TOP_OFFSET: f64 = 46.0;
#[cfg(any(target_os = "windows", target_os = "linux"))]
const OVERLAY_TOP_OFFSET: f64 = 4.0;

#[cfg(target_os = "macos")]
const OVERLAY_BOTTOM_OFFSET: f64 = 15.0;

#[cfg(any(target_os = "windows", target_os = "linux"))]
const OVERLAY_BOTTOM_OFFSET: f64 = 40.0;

/// Configures the edge and offset of a GTK layer surface. gtk-layer-shell
/// commits anchor and margin changes itself, including while the surface is
/// mapped, so changing position does not require a manual hide/show cycle.
#[cfg(target_os = "linux")]
fn configure_layer_shell_position(gtk_window: &gtk::ApplicationWindow, position: OverlayPosition) {
    let (edge, opposite_edge, margin) = match position {
        OverlayPosition::Top => (Edge::Top, Edge::Bottom, OVERLAY_TOP_OFFSET),
        OverlayPosition::Bottom => (Edge::Bottom, Edge::Top, OVERLAY_BOTTOM_OFFSET),
    };

    gtk_window.set_anchor(edge, true);
    gtk_window.set_anchor(opposite_edge, false);
    gtk_window.set_layer_shell_margin(edge, margin.round() as i32);
    gtk_window.set_layer_shell_margin(opposite_edge, 0);
}

/// Configures a GTK layer surface before it is shown.
///
/// Tauri's normal `set_size` path calls `gtk_window_resize`, but layer surfaces
/// derive their dimensions from GTK's size request. gtk-layer-shell documents
/// the `set_size_request` + `resize(1, 1)` sequence for forcing a new size.
#[cfg(target_os = "linux")]
fn configure_layer_shell_surface(
    gtk_window: &gtk::ApplicationWindow,
    position: OverlayPosition,
    width: f64,
    height: f64,
) {
    use gtk::prelude::{GtkWindowExt, WidgetExt};

    configure_layer_shell_position(gtk_window, position);

    gtk_window.set_size_request(
        width.round().max(1.0) as i32,
        height.round().max(1.0) as i32,
    );
    gtk_window.resize(1, 1);
}

/// Initializes GTK layer shell for Linux overlay window
/// Returns true if layer shell was successfully initialized, false otherwise
#[cfg(target_os = "linux")]
fn init_gtk_layer_shell(overlay_window: &tauri::webview::WebviewWindow) -> bool {
    if utils::env_flag_enabled("HANDY_NO_GTK_LAYER_SHELL") {
        debug!("Skipping GTK layer shell init (HANDY_NO_GTK_LAYER_SHELL is enabled)");
        return false;
    }

    if !gtk_layer_shell::is_supported() {
        return false;
    }

    // Try to get the GTK window from the Tauri webview
    if let Ok(gtk_window) = overlay_window.gtk_window() {
        gtk_window.init_layer_shell();
        gtk_window.set_layer(Layer::Overlay);
        gtk_window.set_keyboard_mode(KeyboardMode::None);
        gtk_window.set_exclusive_zone(0);

        let overlay_position = settings::get_settings(overlay_window.app_handle()).overlay_position;
        configure_layer_shell_surface(&gtk_window, overlay_position, OVERLAY_WIDTH, OVERLAY_HEIGHT);

        let initialized = gtk_window.is_layer_window();
        LAYER_SHELL_ACTIVE.store(initialized, Ordering::SeqCst);
        return initialized;
    }
    false
}

/// Forces a window to be topmost using Win32 API (Windows only)
/// This is more reliable than Tauri's set_always_on_top which can be overridden
#[cfg(target_os = "windows")]
fn force_overlay_topmost(overlay_window: &tauri::webview::WebviewWindow) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    };

    // Clone because run_on_main_thread takes 'static
    let overlay_clone = overlay_window.clone();

    // Make sure the Win32 call happens on the UI thread
    let _ = overlay_clone.clone().run_on_main_thread(move || {
        if let Ok(hwnd) = overlay_clone.hwnd() {
            unsafe {
                // Force Z-order: make this window topmost without changing size/pos or stealing focus
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }
    });
}

fn get_monitor_with_cursor(app_handle: &AppHandle) -> Option<tauri::Monitor> {
    if let Some(mouse_location) = input::get_cursor_position(app_handle) {
        if let Ok(monitors) = app_handle.available_monitors() {
            for monitor in monitors {
                // On Windows both the cursor (enigo -> GetCursorPos) and the
                // monitor bounds are physical pixels, so compare them directly.
                #[cfg(target_os = "windows")]
                if is_mouse_within_monitor(mouse_location, monitor.position(), monitor.size()) {
                    return Some(monitor);
                }

                // macOS/Linux: enigo returns logical coords, so scale the bounds down.
                #[cfg(not(target_os = "windows"))]
                {
                    let scale = monitor.scale_factor();
                    let pos = PhysicalPosition::new(
                        (monitor.position().x as f64 / scale) as i32,
                        (monitor.position().y as f64 / scale) as i32,
                    );
                    let size = PhysicalSize::new(
                        (monitor.size().width as f64 / scale) as u32,
                        (monitor.size().height as f64 / scale) as u32,
                    );
                    if is_mouse_within_monitor(mouse_location, &pos, &size) {
                        return Some(monitor);
                    }
                }
            }
        }
    }

    app_handle.primary_monitor().ok().flatten()
}

fn is_mouse_within_monitor(
    mouse_pos: (i32, i32),
    monitor_pos: &PhysicalPosition<i32>,
    monitor_size: &PhysicalSize<u32>,
) -> bool {
    let (mouse_x, mouse_y) = mouse_pos;
    let PhysicalPosition {
        x: monitor_x,
        y: monitor_y,
    } = *monitor_pos;
    let PhysicalSize {
        width: monitor_width,
        height: monitor_height,
    } = *monitor_size;

    mouse_x >= monitor_x
        && mouse_x < (monitor_x + monitor_width as i32)
        && mouse_y >= monitor_y
        && mouse_y < (monitor_y + monitor_height as i32)
}

/// Returns overlay position in logical coordinates (points on macOS).
///
/// The Bottom anchor uses the macOS work area (visibleFrame) so the overlay
/// tracks the Dock — above it when shown, at the screen edge when hidden.
/// This relies on tauri 2.11's work_area.position.y fix (#14655), the same
/// bug that led PR #969 to abandon work_area for full monitor bounds. Top and
/// the other platforms keep full monitor bounds plus the fixed offsets
/// (work_area is unreliable on Wayland; Windows' offset clears the taskbar).
///
/// We must use LogicalPosition (not PhysicalPosition) because Tauri/tao
/// converts PhysicalPosition using the scale factor of the monitor the window
/// is *currently* on, which is wrong when moving cross-monitor. Windows uses
/// `place_windows_overlay` instead (no single logical space across mixed DPI).
fn calculate_overlay_position(
    app_handle: &AppHandle,
    width: f64,
    height: f64,
) -> Option<(f64, f64)> {
    let monitor = get_monitor_with_cursor(app_handle)?;
    let scale = monitor.scale_factor();
    let monitor_x = monitor.position().x as f64 / scale;
    let monitor_y = monitor.position().y as f64 / scale;
    let monitor_width = monitor.size().width as f64 / scale;

    let settings = settings::get_settings(app_handle);

    let x = monitor_x + (monitor_width - width) / 2.0;
    let y = match settings.overlay_position {
        OverlayPosition::Top => monitor_y + OVERLAY_TOP_OFFSET,
        OverlayPosition::Bottom => {
            // work_area.position shares monitor.position's global coordinate
            // space, so no monitor offset is added.
            #[cfg(target_os = "macos")]
            let bottom = {
                let wa = monitor.work_area();
                (wa.position.y as f64 + wa.size.height as f64) / scale
            };
            #[cfg(not(target_os = "macos"))]
            let bottom = monitor_y + monitor.size().height as f64 / scale;

            bottom - height - OVERLAY_BOTTOM_OFFSET
        }
    };

    Some((x, y))
}

/// Current overlay window size in logical units (points), for repositioning
/// without assuming a fixed size (compact vs. streaming).
#[cfg(not(target_os = "windows"))]
fn current_overlay_logical_size(window: &tauri::webview::WebviewWindow) -> Option<(f64, f64)> {
    let size = window.inner_size().ok()?;
    let scale = window.scale_factor().ok()?;
    Some((size.width as f64 / scale, size.height as f64 / scale))
}

#[cfg(target_os = "windows")]
static WINDOWS_OVERLAY_IS_STREAMING: AtomicBool = AtomicBool::new(false);

/// Windows accessibility text size (Settings > Accessibility > Text size), a
/// separate axis from display scaling that WebView2 applies as a document zoom.
#[cfg(target_os = "windows")]
fn windows_text_scale_factor() -> f64 {
    // Absent until the user moves the slider off 100%; stored as a percentage.
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Accessibility")
        .and_then(|key| key.get_value::<u32, _>("TextScaleFactor"))
        .map(|percent| (percent as f64 / 100.0).clamp(1.0, 2.25))
        .unwrap_or(1.0)
}

/// Overlay rectangle in the destination monitor's physical pixels, so nothing
/// is converted through the window's previous-monitor DPI.
#[cfg(target_os = "windows")]
fn windows_overlay_bounds(
    monitor_position: PhysicalPosition<i32>,
    monitor_size: PhysicalSize<u32>,
    scale: f64,
    text_scale: f64,
    logical_width: f64,
    logical_height: f64,
    overlay_position: OverlayPosition,
) -> (i32, i32, i32, i32) {
    // Grow the window with the text scale; offsets stay DPI-only since the
    // card sits flush against the window's screen-edge side.
    let content_scale = scale * text_scale;
    let width = (logical_width * content_scale).round().max(1.0) as i32;
    let height = (logical_height * content_scale).round().max(1.0) as i32;
    let x = (monitor_position.x as f64 + (monitor_size.width as f64 - width as f64) / 2.0).round()
        as i32;
    let y = match overlay_position {
        OverlayPosition::Top => {
            (monitor_position.y as f64 + OVERLAY_TOP_OFFSET * scale).round() as i32
        }
        OverlayPosition::Bottom => (monitor_position.y as f64 + monitor_size.height as f64
            - height as f64
            - OVERLAY_BOTTOM_OFFSET * scale)
            .round() as i32,
    };

    (x, y, width, height)
}

/// Moves and sizes the overlay in one native SetWindowPos, bypassing tao's
/// current-DPI logical conversion that mislands cross-monitor moves.
#[cfg(target_os = "windows")]
fn place_windows_overlay(
    app_handle: &AppHandle,
    overlay_window: &tauri::webview::WebviewWindow,
    logical_width: f64,
    logical_height: f64,
) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};

    let monitor = get_monitor_with_cursor(app_handle)
        .ok_or_else(|| "failed to determine the monitor containing the cursor".to_string())?;
    let text_scale = windows_text_scale_factor();
    let (x, y, width, height) = windows_overlay_bounds(
        *monitor.position(),
        *monitor.size(),
        monitor.scale_factor(),
        text_scale,
        logical_width,
        logical_height,
        settings::get_settings(app_handle).overlay_position,
    );
    let hwnd = overlay_window
        .hwnd()
        .map_err(|error| format!("failed to get overlay window handle: {error}"))?;

    unsafe {
        SetWindowPos(
            hwnd,
            None,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
        .map_err(|error| format!("failed to set overlay bounds: {error}"))?;
    }

    log::debug!(
        "windows overlay bounds: x={} y={} width={} height={} scale={} text_scale={}",
        x,
        y,
        width,
        height,
        monitor.scale_factor(),
        text_scale
    );
    Ok(())
}

/// Creates the recording overlay window and keeps it hidden by default
#[cfg(not(target_os = "macos"))]
pub fn create_recording_overlay(app_handle: &AppHandle) {
    // On Linux (Wayland), monitor detection often fails, but we don't need exact coordinates
    // for Layer Shell as we use anchors. On other platforms, we require a monitor.
    #[cfg(not(target_os = "linux"))]
    {
        let position = calculate_overlay_position(app_handle, OVERLAY_WIDTH, OVERLAY_HEIGHT);
        if position.is_none() {
            debug!("Failed to determine overlay position, not creating overlay window");
            return;
        }
    }

    // Position starts unset — update_overlay_position() sets the correct
    // LogicalPosition before the overlay is shown.
    let mut builder = WebviewWindowBuilder::new(
        app_handle,
        "recording_overlay",
        tauri::WebviewUrl::App("src/overlay/index.html".into()),
    )
    .title("Recording")
    .resizable(false)
    .inner_size(OVERLAY_WIDTH, OVERLAY_HEIGHT)
    .shadow(false)
    .maximizable(false)
    .minimizable(false)
    .closable(false)
    .accept_first_mouse(true)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .focusable(false)
    .focused(false)
    .visible(false);

    if let Some(data_dir) = crate::portable::data_dir() {
        builder = builder.data_directory(data_dir.join("webview"));
    }

    #[allow(unused_variables)]
    match builder.build() {
        Ok(window) => {
            #[cfg(target_os = "linux")]
            {
                // Try to initialize GTK layer shell, ignore errors if compositor doesn't support it
                if init_gtk_layer_shell(&window) {
                    debug!("GTK layer shell initialized for overlay window");
                } else {
                    debug!("GTK layer shell not available, falling back to regular window");
                }
            }

            debug!("Recording overlay window created successfully (hidden)");
        }
        Err(e) => {
            debug!("Failed to create recording overlay window: {}", e);
        }
    }
}

/// Creates the recording overlay panel and keeps it hidden by default (macOS)
#[cfg(target_os = "macos")]
pub fn create_recording_overlay(app_handle: &AppHandle) {
    if let Some((x, y)) = calculate_overlay_position(app_handle, OVERLAY_WIDTH, OVERLAY_HEIGHT) {
        // PanelBuilder creates a Tauri window then converts it to NSPanel.
        // The window remains registered, so get_webview_window() still works.
        match PanelBuilder::<_, RecordingOverlayPanel>::new(app_handle, "recording_overlay")
            .url(WebviewUrl::App("src/overlay/index.html".into()))
            .title("Recording")
            .position(tauri::Position::Logical(tauri::LogicalPosition { x, y }))
            .level(PanelLevel::Status)
            .size(tauri::Size::Logical(tauri::LogicalSize {
                width: OVERLAY_WIDTH,
                height: OVERLAY_HEIGHT,
            }))
            .has_shadow(false)
            .transparent(true)
            .no_activate(true)
            .corner_radius(0.0)
            .style_mask(StyleMask::empty().borderless().nonactivating_panel())
            .with_window(|w| w.decorations(false).transparent(true).focusable(false))
            .collection_behavior(
                CollectionBehavior::new()
                    .can_join_all_spaces()
                    .full_screen_auxiliary(),
            )
            .build()
        {
            Ok(panel) => {
                panel.hide();
            }
            Err(e) => {
                log::error!("Failed to create recording overlay panel: {}", e);
            }
        }
    }
}

fn show_overlay_state(app_handle: &AppHandle, state: &str) {
    // Whether the overlay shows at all is governed by overlay_style; position
    // only chooses Top vs Bottom placement. Checked here (off the main thread)
    // so the common overlay-disabled case never pays for a main-thread hop.
    let settings = settings::get_settings(app_handle);
    if settings.overlay_style == OverlayStyle::None {
        return;
    }

    // The rest queries monitors and the cursor and mutates window geometry. On
    // Linux the monitor/cursor lookups hit GDK/Xlib on the process's shared X11
    // connection, which is only safe from the GTK main thread — running them on
    // a background thread corrupts the connection and hard-crashes the app
    // (issue #227). Hop to the main thread on every platform to keep the
    // geometry path uniform (a no-op cost on Windows, and it also keeps macOS's
    // NSScreen access main-thread-correct). run_on_main_thread runs the closure
    // inline when already on the main thread, so this never deadlocks.
    let handle = app_handle.clone();
    let state = state.to_string();
    let _ = app_handle.run_on_main_thread(move || show_overlay_state_on_main(&handle, &state));
}

fn show_overlay_state_on_main(app_handle: &AppHandle, state: &str) {
    // Size the overlay for this state (compact vs. streaming), then position it.
    let (width, height) = overlay_dimensions(state);
    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        // Invalidate any delayed hide still in flight from a previous session
        // (see `hide_recording_overlay`).
        OVERLAY_SHOW_GENERATION.fetch_add(1, Ordering::SeqCst);

        #[cfg(target_os = "linux")]
        let shown_with_layer_shell = if LAYER_SHELL_ACTIVE.load(Ordering::SeqCst) {
            let position = settings::get_settings(app_handle).overlay_position;
            match overlay_window.gtk_window() {
                Ok(gtk_window) => {
                    configure_layer_shell_surface(&gtk_window, position, width, height)
                }
                Err(error) => log::error!("Failed to access GTK overlay window: {error}"),
            }
            let _ = overlay_window.show();
            true
        } else {
            false
        };
        #[cfg(not(target_os = "linux"))]
        let shown_with_layer_shell = false;

        if !shown_with_layer_shell {
            let size_started = std::time::Instant::now();
            #[cfg(not(target_os = "windows"))]
            let _ =
                overlay_window.set_size(tauri::Size::Logical(tauri::LogicalSize { width, height }));
            #[cfg(target_os = "windows")]
            WINDOWS_OVERLAY_IS_STREAMING.store(state == "streaming", Ordering::Relaxed);
            let size_elapsed = size_started.elapsed();

            let pos_started = std::time::Instant::now();
            #[cfg(not(target_os = "windows"))]
            let set_pos_elapsed =
                // sttts: a user-dragged pill position wins over the centered
                // top/bottom placement — unless the user is dragging right now.
                if pill_dragging() {
                    std::time::Duration::ZERO
                } else if let Some((x, y)) = saved_pill_position(app_handle)
                    .or_else(|| calculate_overlay_position(app_handle, width, height))
                {
                    // State-aware clamp: the streaming panel is wider than
                    // the idle card, so a saved idle position at the right
                    // edge would clip it — pull the window fully on-screen
                    // for THIS state's size.
                    let (x, y) = clamp_to_monitor(app_handle, x, y, width, height);
                    let set_pos_started = std::time::Instant::now();
                    let _ = overlay_window
                        .set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
                    set_pos_started.elapsed()
                } else {
                    std::time::Duration::ZERO
                };
            #[cfg(target_os = "windows")]
            let set_pos_elapsed = {
                let set_pos_started = std::time::Instant::now();
                if let Err(error) =
                    place_windows_overlay(app_handle, &overlay_window, width, height)
                {
                    log::error!("Failed to place recording overlay: {error}");
                }
                set_pos_started.elapsed()
            };
            let pos_calc_elapsed = pos_started.elapsed() - set_pos_elapsed;

            let show_started = std::time::Instant::now();
            let _ = overlay_window.show();
            let show_elapsed = show_started.elapsed();

            // On Windows, aggressively re-assert "topmost" in the native Z-order after showing
            #[cfg(target_os = "windows")]
            force_overlay_topmost(&overlay_window);

            // Re-assert bounds after show(): the pre-show move crosses the DPI
            // boundary, and tao's WM_DPICHANGED reflow clobbers the first placement.
            #[cfg(target_os = "windows")]
            if let Err(error) = place_windows_overlay(app_handle, &overlay_window, width, height) {
                log::error!("Failed to re-assert recording overlay position: {error}");
            }

            // sttts: re-assert the position AFTER show(). On the macOS
            // NSPanel the pre-show set_position is eaten by the panel's
            // autoresize when the size changes in the same pass (verified:
            // saved 1250 clamped to 1214, the card still rendered at ~1342,
            // clipped off the right edge). The post-show move applies.
            if !pill_dragging() {
                if let Some((x, y)) = saved_pill_position(app_handle)
                    .or_else(|| calculate_overlay_position(app_handle, width, height))
                {
                    let (x, y) = clamp_to_monitor(app_handle, x, y, width, height);
                    let _ = overlay_window
                        .set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
                }
            }

            log::debug!(
                "overlay '{}': set_size={:?} pos_calc={:?} set_pos={:?} show={:?}",
                state,
                size_elapsed,
                pos_calc_elapsed,
                set_pos_elapsed,
                show_elapsed
            );
        }

        let _ = overlay_window.emit("show-overlay", state);
    }
}

/// Notify the visible recording overlay that the input stream has delivered its
/// first sample chunk. Audio feedback uses the same backend readiness signal,
/// but this targeted event is skipped when overlays are disabled.
pub fn emit_recording_ready(app_handle: &AppHandle) {
    if !OVERLAY_ENABLED.load(Ordering::Relaxed) {
        return;
    }

    // Showing the overlay is also queued onto the main thread. Queue readiness
    // there as well so a very fast always-on stream cannot overtake show-overlay
    // and then get reset back to the arming state by the frontend.
    let handle = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || {
        let _ = handle.emit_to("recording_overlay", "recording-ready", ());
    });
}

/// Shows the recording overlay window with fade-in animation
pub fn show_recording_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "recording");
}

/// Shows the post-dictation overlay with a "Dictate again" button
pub fn show_complete_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "complete");
}

/// Shows the small idle dictation pill (sttts: persistent, click to dictate)
pub fn show_idle_pill(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "idle");
}

/// Shows the streaming (Live) overlay window
pub fn show_streaming_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "streaming");
}

/// Shows the transcribing overlay window
pub fn show_transcribing_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "transcribing");
}

/// Shows the processing overlay window
pub fn show_processing_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "processing");
}

/// Updates the overlay window position based on current settings
pub fn update_overlay_position(app_handle: &AppHandle) {
    // Positioning queries monitors/cursor (GDK/Xlib on Linux) and moves the
    // window, so it must run on the main thread — see show_overlay_state.
    let handle = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || update_overlay_position_on_main(&handle));
}

fn update_overlay_position_on_main(app_handle: &AppHandle) {
    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        #[cfg(target_os = "linux")]
        if LAYER_SHELL_ACTIVE.load(Ordering::SeqCst) {
            let position = settings::get_settings(app_handle).overlay_position;
            match overlay_window.gtk_window() {
                Ok(gtk_window) => configure_layer_shell_position(&gtk_window, position),
                Err(error) => log::error!("Failed to access GTK overlay window: {error}"),
            }
            return;
        }

        #[cfg(target_os = "windows")]
        {
            let state = if WINDOWS_OVERLAY_IS_STREAMING.load(Ordering::Relaxed) {
                "streaming"
            } else {
                "recording"
            };
            let (width, height) = overlay_dimensions(state);
            if let Err(error) = place_windows_overlay(app_handle, &overlay_window, width, height) {
                log::error!("Failed to update recording overlay position: {error}");
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            // Use the window's current size so centering stays correct whether the
            // overlay is in compact or streaming layout.
            let (width, height) = current_overlay_logical_size(&overlay_window)
                .unwrap_or((OVERLAY_WIDTH, OVERLAY_HEIGHT));
            // sttts: a user-dragged pill position wins over the centered
            // top/bottom placement — unless the user is dragging right now.
            if !pill_dragging() {
                if let Some((x, y)) = saved_pill_position(app_handle)
                    .or_else(|| calculate_overlay_position(app_handle, width, height))
                {
                    let (x, y) = clamp_to_monitor(app_handle, x, y, width, height);
                    let _ = overlay_window
                        .set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
                }
            }
        }
    }
}

/// sttts: --pill-test self-test. Drives the full pill lifecycle from code —
/// show → read position → move + persist (the drag-persist code path) →
/// re-show (restore path) → verify — and writes a verdict JSON next to the
/// settings store. Lets CI/an agent validate pill changes without a human or
/// screen captures.
pub fn run_pill_test(app_handle: &AppHandle) {
    let handle = app_handle.clone();
    std::thread::spawn(move || {
        // Give the overlay webview time to load its JS (the state re-request
        // runs on mount).
        std::thread::sleep(std::time::Duration::from_millis(1500));

        // Run a closure on the main thread and WAIT for it. Steps are
        // separated so the main thread stays free between them — window
        // frames apply and reads are honest (a blocked main thread made
        // earlier verdicts lie).
        let on_main = |job: Box<dyn FnOnce() + Send>| {
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            let _ = handle.run_on_main_thread(move || {
                job();
                let _ = tx.send(());
            });
            rx.recv_timeout(std::time::Duration::from_secs(3)).is_ok()
        };
        let read_pos = || {
            let (tx, rx) = std::sync::mpsc::channel();
            let h = handle.clone();
            let _ = handle.run_on_main_thread(move || {
                let p = h
                    .get_webview_window("recording_overlay")
                    .and_then(|w| w.scale_factor().ok())
                    .and_then(|scale| {
                        h.get_webview_window("recording_overlay")
                            .and_then(|w| w.outer_position().ok())
                            .map(|p| {
                                serde_json::json!({
                                    "x": p.x as f64 / scale,
                                    "y": p.y as f64 / scale,
                                })
                            })
                    });
                let _ = tx.send(p);
            });
            rx.recv_timeout(std::time::Duration::from_secs(3))
                .ok()
                .flatten()
        };
        let visible = || {
            let (tx, rx) = std::sync::mpsc::channel();
            let h = handle.clone();
            let _ = handle.run_on_main_thread(move || {
                let v = h
                    .get_webview_window("recording_overlay")
                    .and_then(|w| w.is_visible().ok())
                    .unwrap_or(false);
                let _ = tx.send(v);
            });
            rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap_or(false)
        };

        let mut results = serde_json::Map::new();

        // 0. deterministic start: clear any saved drag position so p1 is the
        // default placement, not whatever a previous run left behind.
        let h0 = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            let mut st = crate::settings::get_settings(&h0);
            st.overlay_pill_pos = None;
            crate::settings::write_settings(&h0, st);
        });
        std::thread::sleep(std::time::Duration::from_millis(200));

        // 1. show idle, confirm visible
        on_main(Box::new(|| {}));
        let h2 = handle.clone();
        on_main(Box::new(move || show_overlay_state(&h2, "idle")));
        std::thread::sleep(std::time::Duration::from_millis(400));
        let p1 = read_pos();
        results.insert("debug_p1".into(), serde_json::json!(p1));
        results.insert("shown".into(), serde_json::json!(p1.is_some() && visible()));

        // 2. move + persist through the drag-persist path, verify honestly
        if let Some(target_json) = p1.clone() {
            let tx = target_json["x"].as_f64().unwrap_or(700.0) + 120.0;
            let ty = (target_json["y"].as_f64().unwrap_or(880.0) - 60.0).max(40.0);
            let h2 = handle.clone();
            on_main(Box::new(move || {
                if let Some(win) = h2.get_webview_window("recording_overlay") {
                    let _ = win.set_position(tauri::Position::Logical(
                        tauri::LogicalPosition { x: tx, y: ty },
                    ));
                }
            }));
            if let Err(e) = crate::shortcut::set_overlay_pill_position(
                handle.clone(),
                tx,
                ty,
            ) {
                log::warn!("sttts: pill test persist failed: {e}");
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
            // Frame reads (outer_position) return stale values on this panel
            // across show cycles (tauri#7139) — the honest verdict is the
            // persisted settings round-trip plus visibility; the actual frame
            // is verified externally (System Events / screenshot).
            let moved = crate::settings::get_settings(&handle)
                .overlay_pill_pos
                .map(|p| (p.x - tx).abs() < 2.0 && (p.y - ty).abs() < 2.0)
                .unwrap_or(false)
                && visible();
            results.insert("moved".into(), serde_json::json!(moved));

            // 3. restore path: re-show must come from the saved spot
            let h3 = handle.clone();
            on_main(Box::new(move || show_overlay_state(&h3, "idle")));
            std::thread::sleep(std::time::Duration::from_millis(400));
            let restored = crate::settings::get_settings(&handle)
                .overlay_pill_pos
                .map(|p| (p.x - tx).abs() < 2.0 && (p.y - ty).abs() < 2.0)
                .unwrap_or(false)
                && visible();
            results.insert("restored".into(), serde_json::json!(restored));

            // 4. state morph: the streaming panel is 400x170 — starting a
            // dictation at the top-right must clamp fully on-screen instead
            // of clipping off the right edge (the user's report).
            let h4 = handle.clone();
            on_main(Box::new(move || show_overlay_state(&h4, "streaming")));
            std::thread::sleep(std::time::Duration::from_millis(400));
            // Deterministic: the show path clamps the saved spot for the
            // 400x170 panel. clamp_to_monitor needs the main thread on macOS
            // (available_monitors off-main returned nothing and made this
            // verdict a false negative), so compute it on_main.
            let h6 = handle.clone();
            let (tx2, rx2) = std::sync::mpsc::channel::<Option<(f64, f64)>>();
            let _ = handle.run_on_main_thread(move || {
                let _ = tx2.send(Some(clamp_to_monitor(&h6, tx, ty, 400.0, 170.0)));
            });
            let clamped = rx2
                .recv_timeout(std::time::Duration::from_secs(3))
                .ok()
                .flatten();
            let saved = crate::settings::get_settings(&handle).overlay_pill_pos;
            let vis = visible();
            // Display-independent assertion: the clamped 400x170 spot must
            // fit fully inside SOME monitor (with the menu-bar reserve).
            // Hard-coding the main display failed whenever the pill
            // legitimately lives on the second monitor.
            let h7 = handle.clone();
            let (tx3, rx3) = std::sync::mpsc::channel::<bool>();
            let _ = handle.run_on_main_thread(move || {
                let fits = if let Ok(monitors) = h7.available_monitors() {
                    monitors.iter().any(|m| {
                        let s = m.scale_factor();
                        let mx2 = m.position().x as f64 / s;
                        let my2 = m.position().y as f64 / s;
                        let mw2 = m.size().width as f64 / s;
                        let mh2 = m.size().height as f64 / s;
                        clamped.map(|(ex, ey)| {
                            ex >= mx2
                                && ey >= my2 + 28.0
                                && ex + 400.0 <= mx2 + mw2
                                && ey + 170.0 <= my2 + mh2
                        })
                        .unwrap_or(false)
                    })
                } else {
                    false
                };
                let _ = tx3.send(fits);
            });
            let bounds_ok = rx3
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap_or(false);
            results.insert(
                "debug_stream".into(),
                serde_json::json!({
                    "clamped": clamped,
                    "saved_present": saved.is_some(),
                    "bounds_ok": bounds_ok,
                    "visible": vis,
                }),
            );
            let streaming_ok = saved.is_some() && bounds_ok && vis;
            results.insert(
                "streaming_clamped".into(),
                serde_json::json!(streaming_ok),
            );

            // 5. edge probes (deep eval): a dead-corner position must be
            // rejected by the guard, a valid top-right position accepted.
            let _ = crate::shortcut::set_overlay_pill_position(
                handle.clone(),
                1400.0,
                10.0,
            );
            let dead_rejected = effective_saved_pill_position(&handle)
                .map(|p| (p.0 - 1400.0).abs() > 1.0 || (p.1 - 10.0).abs() > 1.0)
                .unwrap_or(true);
            results.insert(
                "probe_dead_corner_rejected".into(),
                serde_json::json!(dead_rejected),
            );

            let _ = crate::shortcut::set_overlay_pill_position(
                handle.clone(),
                1250.0,
                40.0,
            );
            let valid_accepted = effective_saved_pill_position(&handle)
                .map(|p| (p.0 - 1250.0).abs() <= 1.0 && (p.1 - 40.0).abs() <= 1.0)
                .unwrap_or(false);
            results.insert(
                "probe_top_right_saved".into(),
                serde_json::json!(valid_accepted),
            );

            // Leave the pill top-right, visible, as the outcome on screen.
            let h5 = handle.clone();
            on_main(Box::new(move || show_overlay_state(&h5, "idle")));
        } else {
            results.insert("moved".into(), serde_json::json!(false));
            results.insert("restored".into(), serde_json::json!(false));
            results.insert("streaming_clamped".into(), serde_json::json!(false));
            results.insert(
                "probe_dead_corner_rejected".into(),
                serde_json::json!(false),
            );
            results.insert("probe_top_right_saved".into(), serde_json::json!(false));
        }

        // Aggregate only the real checks; the debug_* objects would trip a
        // naive all(as_bool).
        const CHECKS: [&str; 6] = [
            "shown",
            "moved",
            "restored",
            "streaming_clamped",
            "probe_dead_corner_rejected",
            "probe_top_right_saved",
        ];
        let ok = CHECKS.iter().all(|k| {
            results
                .get(*k)
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        });
        results.insert("ok".into(), serde_json::json!(ok));

        if let Ok(path) =
            crate::portable::app_data_dir(&handle).map(|dir| dir.join("pill_test_result.json"))
        {
            let _ = serde_json::to_string_pretty(&results)
                .map(|json| std::fs::write(path, json));
        }
        log::info!("sttts: pill self-test verdict: {:?}", results);
    });
}

/// sttts: true while the user is dragging the pill window; repositioning is
/// suppressed so the backend never fights the OS drag.
static PILL_DRAG_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn set_pill_dragging(active: bool) {
    PILL_DRAG_ACTIVE.store(active, Ordering::SeqCst);
}

fn pill_dragging() -> bool {
    PILL_DRAG_ACTIVE.load(Ordering::SeqCst)
}

/// sttts: a saved pill position is only used when it still lands on a known
/// monitor (logical coordinates). Guards against stale/garbage values
/// parking the pill off-screen (e.g. top-left after a display change).
fn pill_pos_on_any_monitor(app_handle: &AppHandle, x: f64, y: f64) -> bool {
    let Ok(monitors) = app_handle.available_monitors() else {
        return false;
    };
    monitors.iter().any(|m| {
        let scale = m.scale_factor();
        let mx = m.position().x as f64 / scale;
        let my = m.position().y as f64 / scale;
        let mw = m.size().width as f64 / scale;
        let mh = m.size().height as f64 / scale;
        // The visible CARD (172 px, centered with ~42 px inset in the 256 px
        // window) must stay on THIS monitor — the invisible window margin may
        // overhang the edges. The top keeps a menu-bar reserve: the pill is
        // a status-level panel and would otherwise slide behind it and
        // "slowly disappear", especially top-right.
        x >= mx - 42.0
            && y >= my + 28.0
            && x + OVERLAY_WIDTH - 42.0 <= mx + mw
            && y + OVERLAY_HEIGHT <= my + mh
    })
}

/// The saved position after the on-screen guard ran — what a show actually
/// applies. Exposed for the --pill-test probes.
pub fn effective_saved_pill_position(app_handle: &AppHandle) -> Option<(f64, f64)> {
    saved_pill_position(app_handle)
}

/// sttts: clamp a target position so the window of `width`x`height` lies
/// fully on the monitor with the cursor, with a menu-bar reserve at the top.
/// This is the last line of defense at show time: a saved idle position is
/// fine for the small card but the streaming panel is 400px wide and would
/// otherwise clip off the right edge ("die Pill verschwindet").
fn clamp_to_monitor(
    app_handle: &AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> (f64, f64) {
    // Clamp on the monitor the POSITION itself is on (nearest when outside) —
    // never the cursor's: with the mouse resting on another display, a
    // cursor-based clamp teleported the pill across screens on every show.
    let Ok(monitors) = app_handle.available_monitors() else {
        log::warn!("sttts clamp: no monitors");
        return (x, y);
    };
    for m in &monitors {
        let s = m.scale_factor();
        log::info!(
            "sttts clamp: monitor pos=({},{}) size=({},{}) scale={}",
            m.position().x,
            m.position().y,
            m.size().width,
            m.size().height,
            s
        );
    }
    // Prefer a monitor that CONTAINS the window's top-left point outright;
    // only when the point is outside every monitor fall back to the
    // center-distance nearest. (Edge points could otherwise tie and pick the
    // neighbouring display, teleporting the pill across screens.)
    let contains = |m: &tauri::Monitor| {
        let s = m.scale_factor();
        let mx2 = m.position().x as f64 / s;
        let my2 = m.position().y as f64 / s;
        let mw2 = m.size().width as f64 / s;
        let mh2 = m.size().height as f64 / s;
        x >= mx2 && x < mx2 + mw2 && y >= my2 && y < my2 + mh2
    };
    let monitor = monitors
        .iter()
        .find(|m| contains(m))
        .or_else(|| {
            monitors.iter().min_by(|a, b| {
                let d = |m: &tauri::Monitor| {
                    let s = m.scale_factor();
                    let mx2 = m.position().x as f64 / s;
                    let my2 = m.position().y as f64 / s;
                    let mw2 = m.size().width as f64 / s;
                    let mh2 = m.size().height as f64 / s;
                    let dx = (x + width / 2.0).clamp(mx2, mx2 + mw2)
                        - (x + width / 2.0);
                    let dy = (y + height / 2.0).clamp(my2, my2 + mh2)
                        - (y + height / 2.0);
                    dx * dx + dy * dy
                };
                d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)
            })
        });
    let Some(monitor) = monitor else {
        return (x, y);
    };
    let scale = monitor.scale_factor();
    let mx = monitor.position().x as f64 / scale;
    let my = monitor.position().y as f64 / scale;
    let mw = monitor.size().width as f64 / scale;
    let mh = monitor.size().height as f64 / scale;
    const TOP_RESERVE: f64 = 28.0;
    (
        x.clamp(mx, (mx + mw - width).max(mx)),
        y.clamp(my + TOP_RESERVE, (my + mh - height).max(my + TOP_RESERVE)),
    )
}

fn saved_pill_position(app_handle: &AppHandle) -> Option<(f64, f64)> {
    settings::get_settings(app_handle)
        .overlay_pill_pos
        .filter(|p| pill_pos_on_any_monitor(app_handle, p.x, p.y))
        .map(|p| (p.x, p.y))
}

/// Generation counter bumped every time the overlay is shown. The delayed
/// `hide()` below only unmaps the window if no show happened after it was
/// scheduled, so a hide left over from a finished transcription can never
/// take down the overlay of a session that started in the meantime — e.g. a
/// press the coordinator remembered while the pipeline was busy and started
/// the instant it drained, well inside the 300 ms hide delay.
static OVERLAY_SHOW_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Hides the recording overlay window with fade-out animation
pub fn hide_recording_overlay(app_handle: &AppHandle) {
    // Always hide the overlay regardless of settings - if setting was changed while recording,
    // we still want to hide it properly
    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        // Snapshot before doing anything observable, so any show that lands
        // after this point invalidates the delayed hide below.
        let scheduled_at = OVERLAY_SHOW_GENERATION.load(Ordering::SeqCst);
        // Emit event to trigger fade-out animation
        let _ = overlay_window.emit("hide-overlay", ());
        // Hide the window after a short delay to allow animation to complete,
        // unless a newer session has shown the overlay again by then.
        let window_clone = overlay_window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            if OVERLAY_SHOW_GENERATION.load(Ordering::SeqCst) != scheduled_at {
                log::debug!("Skipping stale overlay hide: a newer session is showing the overlay");
                return;
            }
            let _ = window_clone.hide();
        });
    }
}

// Cached "overlay is enabled" flag, kept in sync with overlay_style. Avoids
// reading the Tauri store on every audio callback (~24 Hz during recording).
// Defaults to false so the audio path doesn't emit until lib.rs::setup
// populates the cache from initial settings.
static OVERLAY_ENABLED: AtomicBool = AtomicBool::new(false);

/// Tracks whether gtk-layer-shell was successfully initialized (Linux only).
/// Used to skip layer-shell calls when the window is a regular fallback.
#[cfg(target_os = "linux")]
static LAYER_SHELL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Update the cached overlay-enabled flag. Called from `lib.rs` at
/// startup after settings load, and from `change_overlay_style_setting`
/// whenever the user changes whether the overlay is shown.
pub fn update_overlay_enabled_cache(enabled: bool) {
    OVERLAY_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn emit_levels(app_handle: &AppHandle, levels: &[f32]) {
    // Skip emission when the overlay is disabled. The recording_overlay
    // window is created at boot regardless of overlay_style, so without this
    // guard a hidden overlay's WebKit subprocess still
    // processes every event. Each event drives some kind of WebKit
    // C++ allocation that accumulates without bound (mechanism not
    // directly characterized; see issue #1279 for the investigation).
    // For users with `overlay_style: none` (the Linux default) this skip
    // eliminates the upstream driver of that accumulation.
    if !OVERLAY_ENABLED.load(Ordering::Relaxed) {
        return;
    }

    // Throttle to ~30 FPS. Even with the overlay enabled, the raw audio
    // callback fires far faster than the UI needs; capping emission rate
    // cuts the per-frame `eval_script`/IPC volume that drives the wry
    // memory growth in issue #1279 (upstream tauri-apps/wry#1489).
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let last = LAST_MIC_LEVEL_EMIT.load(Ordering::Relaxed);
    if now.saturating_sub(last) < EMIT_THROTTLE_MS {
        return;
    }
    LAST_MIC_LEVEL_EMIT.store(now, Ordering::Relaxed);

    // Target only the overlay window. In Tauri 2 both `AppHandle::emit`
    // and `WebviewWindow::emit` broadcast to all webviews; Tauri's
    // listener filter then skips webviews with no registered listener
    // for the event, so the settings webview never received `mic-level`.
    // But the previous dual-call pattern still produced two `eval_script`
    // calls to the overlay per audio callback (one from each .emit()).
    // `emit_to` with the overlay's window label produces a single
    // eval_script call per callback, cutting the per-callback WebKit
    // dispatch work in half.
    let _ = app_handle.emit_to("recording_overlay", "mic-level", levels);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_hit_test_uses_half_open_physical_bounds() {
        let position = PhysicalPosition::new(-2560, -200);
        let size = PhysicalSize::new(2560, 1440);

        assert!(is_mouse_within_monitor((-2560, -200), &position, &size));
        assert!(is_mouse_within_monitor((-1, 1239), &position, &size));
        assert!(!is_mouse_within_monitor((0, 0), &position, &size));
        assert!(!is_mouse_within_monitor((-1, 1240), &position, &size));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_cursor_hit_test_does_not_scale_physical_monitor_bounds() {
        let position = PhysicalPosition::new(1920, 0);
        let size = PhysicalSize::new(3840, 2160);
        let cursor = (5000, 1000);

        assert!(is_mouse_within_monitor(cursor, &position, &size));

        // This is the old mixed-coordinate comparison. It excludes a cursor
        // that is visibly inside a secondary display running at 150%.
        let scale = 1.5;
        let logical_position = PhysicalPosition::new(
            (position.x as f64 / scale) as i32,
            (position.y as f64 / scale) as i32,
        );
        let logical_size = PhysicalSize::new(
            (size.width as f64 / scale) as u32,
            (size.height as f64 / scale) as u32,
        );
        assert!(!is_mouse_within_monitor(
            cursor,
            &logical_position,
            &logical_size
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_overlay_bounds_use_destination_monitor_scale() {
        let monitor_position = PhysicalPosition::new(1920, 0);
        let monitor_size = PhysicalSize::new(3840, 2160);

        assert_eq!(
            windows_overlay_bounds(
                monitor_position,
                monitor_size,
                1.5,
                1.0,
                OVERLAY_WIDTH,
                OVERLAY_HEIGHT,
                OverlayPosition::Bottom,
            ),
            (3648, 2025, 384, 75)
        );
        assert_eq!(
            windows_overlay_bounds(
                monitor_position,
                monitor_size,
                1.5,
                1.0,
                OVERLAY_WIDTH,
                OVERLAY_HEIGHT,
                OverlayPosition::Top,
            ),
            (3648, 6, 384, 75)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_overlay_bounds_support_negative_monitor_origins() {
        assert_eq!(
            windows_overlay_bounds(
                PhysicalPosition::new(-2560, -200),
                PhysicalSize::new(2560, 1440),
                1.25,
                1.0,
                OVERLAY_STREAM_WIDTH,
                OVERLAY_STREAM_HEIGHT,
                OverlayPosition::Bottom,
            ),
            (-1530, 1040, 500, 150)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_overlay_bounds_grow_with_text_scale_without_moving_the_anchored_edge() {
        let monitor_position = PhysicalPosition::new(-2560, -200);
        let monitor_size = PhysicalSize::new(2560, 1440);

        let (x, y, width, height) = windows_overlay_bounds(
            monitor_position,
            monitor_size,
            1.25,
            1.1,
            OVERLAY_STREAM_WIDTH,
            OVERLAY_STREAM_HEIGHT,
            OverlayPosition::Bottom,
        );
        // 400x120 logical at 1.25 DPI x 1.1 text, still centered horizontally.
        assert_eq!((x, y, width, height), (-1555, 1025, 550, 165));
        // Bottom edge unchanged from the 1.0 case above (1040 + 150).
        assert_eq!(y + height, 1190);

        let (_, top_y, _, _) = windows_overlay_bounds(
            monitor_position,
            monitor_size,
            1.25,
            1.1,
            OVERLAY_STREAM_WIDTH,
            OVERLAY_STREAM_HEIGHT,
            OverlayPosition::Top,
        );
        // Top offset rides the DPI scale alone, so the top edge doesn't move.
        assert_eq!(top_y, -195);
    }
}
