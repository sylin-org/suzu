//! The embedded resource tree, and where it goes when there is no checkout.
//!
//! A checkout and an installed service keep resources on disk; a bare
//! binary — from a release archive, Homebrew, or npm — carries them
//! inside itself and materializes them on first use, so the catalog,
//! the install procedures, and the offline factory reset work with
//! nothing but the executable. The tree is stamped with the crate
//! version: an upgrade re-materializes exactly once.

use include_dir::{include_dir, Dir, DirEntry};
use std::path::{Path, PathBuf};

static EMBEDDED_HARDWARE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../hardware");
static EMBEDDED_FIRMWARE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../firmware");

const STAMP_FILE: &str = ".suzu-embedded";

/// The resource tree the binary carries. Both roots are embedded: the
/// hardware manifests that drive the catalog, and the firmware payloads
/// (runtime artifacts included) that the install and factory-reset
/// procedures vendored in the first place.
#[cfg(test)]
pub fn embedded() -> (&'static Dir<'static>, &'static Dir<'static>) {
    (&EMBEDDED_HARDWARE, &EMBEDDED_FIRMWARE)
}

/// The platform data directory for a bare binary's own resources.
fn data_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        return std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|p| PathBuf::from(p).join("suzu").join("resources"));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    if cfg!(target_os = "macos") {
        return Some(
            home.join("Library/Application Support")
                .join("suzu")
                .join("resources"),
        );
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    Some(data.join("suzu").join("resources"))
}

/// Materialize the embedded tree into an installed location when the
/// running copy lives outside a checkout and no installed resources
/// exist yet. Returns the directory serving the resources, or `None`
/// when the caller should keep its current behavior (a checkout, an
/// installed prefix, or no usable data dir).
pub fn ensure() -> Option<PathBuf> {
    // A checkout keeps its familiar relative layout — never shadow it.
    if Path::new("hardware/classes").is_dir() && Path::new("firmware").is_dir() {
        return None;
    }
    let target = data_dir()?;
    match stamped(&target) {
        true => Some(target),
        false => materialize_into(&target).ok().map(|_| target),
    }
}

/// True when `target` already holds this exact build's resource tree.
fn stamped(target: &Path) -> bool {
    let stamp = target.join(STAMP_FILE);
    std::fs::read_to_string(&stamp)
        .map(|s| s.trim() == env!("CARGO_PKG_VERSION"))
        .unwrap_or(false)
}

/// Write the embedded tree into `target`, overwriting the stamp last.
pub fn materialize_into(target: &Path) -> std::io::Result<()> {
    write_dir(&EMBEDDED_HARDWARE, &target.join("hardware"))?;
    write_dir(&EMBEDDED_FIRMWARE, &target.join("firmware"))?;
    std::fs::create_dir_all(target)?;
    std::fs::write(target.join(STAMP_FILE), env!("CARGO_PKG_VERSION"))?;
    Ok(())
}

fn write_dir(dir: &Dir<'_>, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    let entry_name = |path: &Path| {
        path.file_name()
            .map(std::ffi::OsStr::to_owned)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "embedded entry without a name",
                )
            })
    };
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(d) => {
                let sub = target.join(entry_name(d.path())?);
                write_dir(d, &sub)?;
            }
            DirEntry::File(f) => {
                std::fs::write(target.join(entry_name(f.path())?), f.contents())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_tree_carries_the_manifests_and_payloads() {
        let (hardware, firmware) = embedded();
        assert!(hardware.contains("classes/esp8266-oled/manifest.yaml"));
        assert!(hardware.contains("classes/esp8266-oled/faceplates/numerals/faceplate.yaml"));
        assert!(firmware.contains("suzu-d/rp2040-matrix/code.py"));
        // The vendored runtimes keep the offline factory reset honest.
        assert!(firmware.contains("artifacts/micropython-esp8266-1mib.bin"));
        assert!(firmware.contains("artifacts/flash_nuke.uf2"));
    }

    #[test]
    fn materialization_is_versioned_and_idempotent() {
        let target = std::env::temp_dir().join(format!(
            "suzu-resources-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&target);
        materialize_into(&target).expect("first materialization");
        assert!(stamped(&target));
        assert!(target.join("hardware/classes").is_dir());
        assert!(target
            .join("firmware/artifacts/circuitpython-raspberry_pi_pico.uf2")
            .is_file());
        let _ = std::fs::remove_dir_all(&target);
    }
}
