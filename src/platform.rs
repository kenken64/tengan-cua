use enigo::Key;

pub(crate) fn platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        std::env::consts::OS
    }
}

/// Best-effort OS version string, for example "15.4" on macOS.
pub(crate) fn os_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        command_stdout("sw_vers", &["-productVersion"])
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/etc/os-release")
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("PRETTY_NAME="))
            .map(|value| value.trim().trim_matches('"').to_string())
            .filter(|value| !value.is_empty())
    }
    #[cfg(target_os = "windows")]
    {
        command_stdout("cmd", &["/C", "ver"])
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// Platform name plus version when detectable, for example "macOS 15.4".
pub(crate) fn platform_description() -> String {
    match os_version() {
        Some(version) => format!("{} {}", platform_name(), version),
        None => platform_name().to_string(),
    }
}

/// The modifier used for ordinary application shortcuts on this OS:
/// Command on macOS, Control everywhere else.
pub(crate) fn primary_modifier() -> Key {
    if cfg!(target_os = "macos") {
        Key::Meta
    } else {
        Key::Control
    }
}

pub(crate) fn primary_modifier_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "Command"
    } else {
        "Control"
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() { None } else { Some(stdout) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_modifier_matches_platform_convention() {
        if cfg!(target_os = "macos") {
            assert_eq!(primary_modifier(), Key::Meta);
            assert_eq!(primary_modifier_name(), "Command");
        } else {
            assert_eq!(primary_modifier(), Key::Control);
            assert_eq!(primary_modifier_name(), "Control");
        }
    }

    #[test]
    fn platform_description_starts_with_platform_name() {
        assert!(platform_description().starts_with(platform_name()));
    }
}
