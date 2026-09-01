//! Runtime locations shared by the CLI and the installed Resident.
//!
//! A checkout keeps its familiar relative layout. An installed binary
//! discovers `share/suzu` beside its prefix, while the systemd unit pins
//! explicit resource and state roots for a predictable, writable service.

use std::path::PathBuf;

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn resource_dir() -> PathBuf {
    if let Some(path) = env_path("SUZU_RESOURCE_DIR") {
        return path;
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(prefix) = exe.parent().and_then(|bin| bin.parent())
    {
        let installed = prefix.join("share/suzu");
        if installed.is_dir() {
            return installed;
        }
    }
    PathBuf::from(".")
}

pub fn hardware_dir() -> PathBuf {
    env_path("SUZU_HARDWARE_DIR").unwrap_or_else(|| resource_dir().join("hardware"))
}

pub fn firmware_dir() -> PathBuf {
    resource_dir().join("firmware")
}

pub fn state_dir() -> PathBuf {
    env_path("SUZU_STATE_DIR").unwrap_or_else(|| PathBuf::from("."))
}

pub fn backups_dir() -> PathBuf {
    state_dir().join("backups")
}

pub fn captures_dir() -> PathBuf {
    env_path("SUZU_CAPTURES_DIR").unwrap_or_else(|| state_dir().join("captures"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_paths_keep_checkout_behavior() {
        if std::env::var_os("SUZU_STATE_DIR").is_none() {
            assert_eq!(backups_dir(), PathBuf::from("./backups"));
        }
    }
}
