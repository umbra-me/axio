//! The notification-area icon, its native menu, and the HTML flyout it opens.
//!
//! Two of these three are native and cannot be otherwise. The icon is a bitmap because
//! `Shell_NotifyIconW` takes one; the right-click menu is an OS menu because the shell
//! owns it. Only the flyout — a borderless webview positioned against the icon — is HTML,
//! and it is the piece worth having: it is the faithful port of the popover the macOS
//! original shows, which a list of menu strings only approximates.

use std::sync::Arc;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, LogicalPosition, Manager, PhysicalPosition};

use super::icon::{self, Severity};
use super::state::AppState;
use super::{FLYOUT_WINDOW, MAIN_WINDOW};

const TRAY_ID: &str = "quota";
const FLYOUT_WIDTH: f64 = 340.0;
const FLYOUT_HEIGHT: f64 = 460.0;

/// Rendered larger than 16px: Windows scales a tray icon down for the notification area
/// and up for the overflow flyout, and downscaling a 32px glyph reads better than
/// upscaling a 16px one.
const ICON_SIZE: u32 = 32;

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open axio quota", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &refresh, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        // Left click belongs to the flyout, so the menu must not steal it.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => open_main(app),
            "refresh" => super::state::refresh(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                toggle_flyout(tray.app_handle(), rect.position.to_physical(1.0));
            }
        })
        .build(app)?;

    Ok(())
}

pub fn open_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    hide_flyout(app);
}

/// Re-renders the icon from current results and pushes it to the shell.
pub fn update_icon(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    let snapshots: Vec<_> = {
        let results = state.results.lock().unwrap_or_else(|err| err.into_inner());
        results
            .iter()
            .filter_map(|(id, outcome)| outcome.as_ref().ok().map(|s| (*id, s.clone())))
            .collect()
    };

    let focus = crate::focus::tray_focus(&snapshots).map(|(_, snapshot)| snapshot);
    let (text, severity) = label(focus);
    let tooltip = match focus.and_then(|snapshot| snapshot.headline().map(|w| (snapshot, w))) {
        Some((snapshot, window)) => format!(
            "{} — {} {:.0}% used",
            snapshot.provider.display_name(),
            window.label,
            window.used_percent
        ),
        None => "axio quota — no data".to_string(),
    };

    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    if let Some(rendered) = icon::render(&text, ICON_SIZE, severity) {
        let image = Image::new_owned(rendered.pixels, rendered.size, rendered.size);
        let _ = tray.set_icon(Some(image));
    }
    let _ = tray.set_tooltip(Some(&tooltip));
}

/// What the icon shows.
///
/// Two characters is the practical ceiling, so 100% becomes "!!" rather than a number too
/// small to read. A provider that could not report is "--", which must look different from
/// 0% — those mean opposite things.
fn label(focus: Option<&crate::UsageSnapshot>) -> (String, Severity) {
    let Some(window) = focus.and_then(crate::UsageSnapshot::headline) else {
        return ("--".to_string(), Severity::Unknown);
    };
    let used = window.used_percent;
    let severity = Severity::from_used_percent(used);
    if used >= 99.5 {
        return ("!!".to_string(), severity);
    }
    (format!("{}", used.round() as i64), severity)
}

fn toggle_flyout(app: &AppHandle, at: PhysicalPosition<f64>) {
    let Some(window) = app.get_webview_window(FLYOUT_WINDOW) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }

    // Anchored above the icon and nudged left so the panel does not hang off the right
    // edge of the screen. The taskbar sits at the bottom on nearly every install, so a
    // panel placed below the icon would be underneath it.
    let scale = window.scale_factor().unwrap_or(1.0);
    let logical = at.to_logical::<f64>(scale);
    let position = LogicalPosition::new(
        logical.x - FLYOUT_WIDTH + 20.0,
        logical.y - FLYOUT_HEIGHT - 12.0,
    );
    let _ = window.set_position(tauri::Position::Logical(position));
    let _ = window.show();
    let _ = window.set_focus();
    super::state::refresh(app);
}

pub fn hide_flyout(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(FLYOUT_WINDOW) {
        let _ = window.hide();
    }
}
