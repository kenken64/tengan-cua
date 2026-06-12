use anyhow::{Result, anyhow};

/// A visible top-level window. Coordinates are absolute desktop coordinates,
/// matching the desktop_origin/desktop_size reported for monitors.
#[derive(Clone, Debug)]
pub(crate) struct WindowInfo {
    pub(crate) app: String,
    pub(crate) title: String,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) focused: bool,
    /// Window-manager id used to activate the window on Linux.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) id: Option<String>,
}

/// Desktop furniture that shows up in the window list but is never a useful
/// automation target.
const SYSTEM_APP_DENYLIST: &[&str] = &[
    "Window Server",
    "Dock",
    "Control Center",
    "Control Centre",
    "Notification Center",
    "Notification Centre",
    "UserNotificationCenter",
    "Spotlight",
    "Wallpaper",
    "SystemUIServer",
    "Screenshot",
];

pub(crate) fn list_windows() -> Result<Vec<WindowInfo>> {
    let mut windows = Vec::new();
    let mut z_orders = Vec::new();

    for window in xcap::Window::all()? {
        if window.is_minimized().unwrap_or(false) {
            continue;
        }

        let app = window.app_name().unwrap_or_default();
        let title = window.title().unwrap_or_default();
        if app.is_empty() && title.is_empty() {
            continue;
        }
        if SYSTEM_APP_DENYLIST
            .iter()
            .any(|denied| app.eq_ignore_ascii_case(denied))
        {
            continue;
        }

        let width = window.width().unwrap_or(0) as i32;
        let height = window.height().unwrap_or(0) as i32;
        // Menu bar items, status overlays, and other desktop furniture show up
        // as tiny windows; they are never useful click targets.
        if width < 50 || height < 50 {
            continue;
        }

        windows.push(WindowInfo {
            app,
            title,
            x: window.x().unwrap_or(0),
            y: window.y().unwrap_or(0),
            width,
            height,
            focused: window.is_focused().unwrap_or(false),
            id: window.id().ok().map(|id| id.to_string()),
        });
        z_orders.push(window.z().unwrap_or(i32::MIN));
    }

    // On macOS the focus flag is app-level, so every window of the frontmost
    // app reports focused. Keep it only on the topmost of those windows.
    let top_focused_index = windows
        .iter()
        .enumerate()
        .filter(|(_, window)| window.focused)
        .max_by_key(|(index, _)| z_orders[*index])
        .map(|(index, _)| index);
    if let Some(top_focused_index) = top_focused_index {
        for (index, window) in windows.iter_mut().enumerate() {
            window.focused = index == top_focused_index;
        }
    }

    Ok(windows)
}

/// Find the visible window best matching `query` (title or app name,
/// case-insensitive) and bring it to the foreground.
pub(crate) fn focus_window(query: &str) -> Result<WindowInfo> {
    let windows = list_windows()?;
    let matched = best_match(&windows, query)
        .ok_or_else(|| {
            anyhow!(
                "no visible window matches {query:?}; visible windows: {}",
                summarize(&windows)
            )
        })?
        .clone();

    activate_window(&matched)?;
    Ok(matched)
}

fn activate_window(window: &WindowInfo) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::activate_window(window)
    }
    #[cfg(target_os = "linux")]
    {
        linux::activate_window(window)
    }
    #[cfg(target_os = "windows")]
    {
        win::activate_window(window)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = window;
        anyhow::bail!("window activation is not supported on this platform")
    }
}

/// Exact title beats exact app name, which beats title substring, which beats
/// app substring. Ties keep the first listed window.
pub(crate) fn best_match<'a>(windows: &'a [WindowInfo], query: &str) -> Option<&'a WindowInfo> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }

    let mut best: Option<(u32, &WindowInfo)> = None;
    for window in windows {
        let score = match_score(window, &needle);
        if score > 0 && best.is_none_or(|(best_score, _)| score > best_score) {
            best = Some((score, window));
        }
    }

    best.map(|(_, window)| window)
}

fn match_score(window: &WindowInfo, needle: &str) -> u32 {
    let title = window.title.to_lowercase();
    let app = window.app.to_lowercase();

    if title == needle {
        4
    } else if app == needle {
        3
    } else if title.contains(needle) {
        2
    } else if app.contains(needle) {
        1
    } else {
        0
    }
}

fn summarize(windows: &[WindowInfo]) -> String {
    if windows.is_empty() {
        return "none".to_string();
    }

    windows
        .iter()
        .map(|window| format!("{:?} ({})", window.title, window.app))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(target_os = "macos")]
mod macos {
    use std::process::Command;

    use anyhow::{Context, Result, bail};

    use super::WindowInfo;

    // Activating the app needs no special permission; raising one specific
    // window goes through System Events and silently degrades to app
    // activation when the Accessibility permission is missing or slow.
    const FOCUS_WINDOW_SCRIPT: &str = r#"
on run argv
    set appName to item 1 of argv
    set winTitle to item 2 of argv
    tell application appName to activate
    if winTitle is not "" then
        try
            with timeout of 5 seconds
                tell application "System Events" to tell process appName
                    perform action "AXRaise" of (first window whose name is winTitle)
                end tell
            end timeout
        end try
    end if
end run
"#;

    pub(super) fn activate_window(window: &WindowInfo) -> Result<()> {
        let output = Command::new("osascript")
            .arg("-e")
            .arg(FOCUS_WINDOW_SCRIPT)
            .arg(&window.app)
            .arg(&window.title)
            .output()
            .context("failed to run osascript")?;

        if !output.status.success() {
            bail!(
                "osascript exited with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::process::Command;

    use anyhow::{Context, Result, bail};

    use super::WindowInfo;

    pub(super) fn activate_window(window: &WindowInfo) -> Result<()> {
        let mut command = Command::new("wmctrl");
        match &window.id {
            Some(id) => command.arg("-i").arg("-a").arg(id),
            None => command.arg("-a").arg(&window.title),
        };

        let status = command
            .status()
            .context("failed to run wmctrl; install wmctrl for window activation")?;
        if !status.success() {
            bail!("wmctrl exited with status {status}");
        }

        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod win {
    use std::process::Command;

    use anyhow::{Context, Result, bail};

    use super::WindowInfo;

    pub(super) fn activate_window(window: &WindowInfo) -> Result<()> {
        let target = if window.title.is_empty() {
            &window.app
        } else {
            &window.title
        };
        let script = format!(
            "$shell = New-Object -ComObject WScript.Shell; \
if (-not $shell.AppActivate('{}')) {{ exit 1 }}",
            target.replace('\'', "''")
        );

        let status = Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(&script)
            .status()
            .context("failed to run powershell")?;

        if !status.success() {
            bail!("AppActivate could not focus window {target:?}");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(app: &str, title: &str) -> WindowInfo {
        WindowInfo {
            app: app.to_string(),
            title: title.to_string(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            focused: false,
            id: None,
        }
    }

    #[test]
    fn best_match_prefers_exact_title_over_substring() {
        let windows = vec![
            window("Google Chrome", "Stake.com - Blackjack"),
            window("Terminal", "Stake"),
        ];

        let matched = best_match(&windows, "stake").unwrap();
        assert_eq!(matched.app, "Terminal");

        let matched = best_match(&windows, "stake.com").unwrap();
        assert_eq!(matched.app, "Google Chrome");
    }

    #[test]
    fn best_match_falls_back_to_app_name() {
        let windows = vec![
            window("Google Chrome", "Stake.com - Blackjack"),
            window("iTerm2", "~/Projects/tengan-cua"),
        ];

        let matched = best_match(&windows, "chrome").unwrap();
        assert_eq!(matched.app, "Google Chrome");

        let matched = best_match(&windows, "iterm").unwrap();
        assert_eq!(matched.app, "iTerm2");
    }

    #[test]
    fn best_match_rejects_empty_and_unmatched_queries() {
        let windows = vec![window("Finder", "Downloads")];

        assert!(best_match(&windows, "").is_none());
        assert!(best_match(&windows, "   ").is_none());
        assert!(best_match(&windows, "spotify").is_none());
    }
}
