//! The catalog — hardware manifests loaded into memory at boot.
//!
//! Folder per class (`hardware/classes/<class-id>/`). Two files are
//! parsed, per the separation of concerns: `signature.yaml` — the
//! identification bits — and the `display` section of `manifest.yaml`
//! (servicing data: the panel's phosphor zones, so screenshots can
//! color what the eye sees). Procedures and evidence/ stay with the
//! servicing engine. serde skips unknown fields, so the files may grow
//! without this tool ever changing.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// The parsed `signature.yaml`.
#[derive(Debug, Clone, Deserialize)]
pub struct ClassSignature {
    #[allow(dead_code)] // declared for manifest validation, not identification
    pub schema: u8,
    pub id: String,
    pub family: String,
    pub variant: String,
    #[serde(rename = "match")]
    pub match_rules: MatchRules,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DisplayZone {
    /// Inclusive native row range: `rows: [0, 15]`.
    pub rows: Vec<u16>,
    /// `#rrggbb` — the phosphor color this zone shines.
    pub color: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DisplaySpec {
    #[serde(rename = "type")]
    pub panel_type: Option<String>,
    pub resolution: Option<String>,
    #[serde(default)]
    pub zones: Vec<DisplayZone>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MatchRules {
    /// `"1a86:7523"` (exact) or `"2e8a:*"` (vendor-wide).
    pub vid_pid: Vec<String>,
}

/// The parsed `manifest.yaml` — servicing data only. Fields grow as
/// the tool earns them; serde ignores the rest.
#[derive(Debug, Clone, Deserialize)]
pub struct ClassManifest {
    pub id: String,
    pub display: Option<DisplaySpec>,
}

#[derive(Clone)]
pub struct Catalog {
    /// Where the signatures came from (for the boot line).
    pub source: String,
    classes: Vec<ClassSignature>,
    /// vid -> [(pid-or-wildcard, class index)]
    index: HashMap<u16, Vec<(Option<u16>, usize)>>,
    /// class id -> its manifest's servicing bits (display zones …)
    manifests: HashMap<String, ClassManifest>,
}

/// `#rrggbb` -> RGB triple; unparsable colors fall back to white.
pub fn parse_color(s: &str) -> [u8; 3] {
    let hex = s.trim_start_matches('#');
    if hex.len() != 6 {
        return [230, 230, 230];
    }
    let mut out = [230u8, 230, 230];
    for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(pair).unwrap_or("e6"), 16).unwrap_or(230);
    }
    out
}

fn parse_vidpid(s: &str) -> Option<(u16, Option<u16>)> {
    let (v, p) = s.split_once(':')?;
    let vid = u16::from_str_radix(v.trim_start_matches("0x"), 16).ok()?;
    let pid = if p == "*" {
        None
    } else {
        Some(u16::from_str_radix(p.trim_start_matches("0x"), 16).ok()?)
    };
    Some((vid, pid))
}

impl Catalog {
    /// Load every class folder's `signature.yaml`. Search order:
    /// `$SUZU_HARDWARE_DIR/classes`, then `hardware/classes` relative to
    /// the working directory (and one level up, for `cargo run` from a
    /// crate dir). If nothing is found, return an empty catalog and let
    /// the built-in seed hints answer.
    pub fn load() -> Catalog {
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Ok(dir) = std::env::var("SUZU_HARDWARE_DIR") {
            roots.push(PathBuf::from(dir).join("classes"));
        }
        roots.push(PathBuf::from("hardware/classes"));
        roots.push(PathBuf::from("../hardware/classes"));

        for root in &roots {
            let Ok(entries) = std::fs::read_dir(root) else {
                continue;
            };
            let mut class_dirs: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            class_dirs.sort();
            if class_dirs.is_empty() {
                continue;
            }

            let mut classes = Vec::new();
            let mut index: HashMap<u16, Vec<(Option<u16>, usize)>> = HashMap::new();
            let mut manifests: HashMap<String, ClassManifest> = HashMap::new();
            for dir in class_dirs {
                if let Ok(text) = std::fs::read_to_string(dir.join("manifest.yaml")) {
                    if let Ok(m) = serde_yaml::from_str::<ClassManifest>(&text) {
                        manifests.insert(m.id.clone(), m);
                    }
                }
                let sig_path = dir.join("signature.yaml");
                let Ok(text) = std::fs::read_to_string(&sig_path) else {
                    eprintln!("catalog: no signature.yaml in {}", dir.display());
                    continue;
                };
                match serde_yaml::from_str::<ClassSignature>(&text) {
                    Ok(sig) => {
                        let idx = classes.len();
                        for rule in &sig.match_rules.vid_pid {
                            if let Some((vid, pid)) = parse_vidpid(rule) {
                                index.entry(vid).or_default().push((pid, idx));
                            }
                        }
                        classes.push(sig);
                    }
                    Err(e) => eprintln!("catalog: parse error in {}: {e}", sig_path.display()),
                }
            }

            if classes.is_empty() {
                continue;
            }
            return Catalog {
                source: format!(
                    "{} class signature(s) from {}",
                    classes.len(),
                    root.display()
                ),
                classes,
                index,
                manifests,
            };
        }

        Catalog {
            source: "no manifests found — using built-in seed hints".into(),
            classes: Vec::new(),
            index: HashMap::new(),
            manifests: HashMap::new(),
        }
    }

    /// VID/PID lookup — used when the port is silent (fresh firmware).
    pub fn class_by_vidpid(&self, vid: u16, pid: u16) -> Option<&ClassSignature> {
        let bucket = self.index.get(&vid)?;
        bucket
            .iter()
            .find(|(mp, _)| mp.is_none_or(|p| p == pid))
            .map(|(_, idx)| &self.classes[*idx])
    }

    /// The display zones of a class's manifest: (first_row, last_row,
    /// rgb) — what a faithful screenshot colors.
    pub fn display_zones(&self, class_id: &str) -> Vec<(usize, usize, [u8; 3])> {
        self.manifests
            .get(class_id)
            .and_then(|m| m.display.as_ref())
            .map(|d| {
                d.zones
                    .iter()
                    .filter(|z| z.rows.len() == 2)
                    .map(|z| {
                        (
                            z.rows[0] as usize,
                            z.rows[1] as usize,
                            parse_color(&z.color),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Signature lookup — used when a descriptor answered. Legacy CSV
    /// identities are reduced to (family, variant-token) before this.
    pub fn class_by_signature(&self, family: &str, variant: &str) -> Option<&ClassSignature> {
        self.classes
            .iter()
            .find(|c| c.signature_family_matches(family) && c.signature_variant_matches(variant))
    }
}

impl ClassSignature {
    fn signature_family_matches(&self, family: &str) -> bool {
        self.family == family
    }

    fn signature_variant_matches(&self, variant: &str) -> bool {
        // Class ids like `esp8266-oled-v2` carry variant tokens; match
        // by containment so `oled` finds `oled-v2` classes.
        self.variant == variant || self.id.contains(variant)
    }
}

/// Built-in fallback hints (used only when no manifests are found).
/// Seeds from the physical fleet: 2026-08-28 bench sessions.
const SEED: &[(u16, Option<u16>, &str)] = &[
    (0x1a86, Some(0x7523), "ESP8266 + OLED display (NodeMCU class, CH340)"),
    (0x1a86, Some(0x55d4), "ESP32 T-Display class (CH9102F)"),
    (0x1a86, Some(0x55d3), "ESP32 T-Display class (CH9102)"),
    (0x10c4, Some(0xea60), "CP2102 bridge — board class unknown until probed"),
    (0x303a, None, "ESP32-Sx native USB (XIAO S3 class)"),
    (0x2e8a, None, "RP2040 (matrix class)"),
];

pub fn seed_hint(vid: u16, pid: u16) -> Option<&'static str> {
    SEED.iter()
        .find(|(v, p, _)| *v == vid && p.is_none_or(|sp| sp == pid))
        .map(|(_, _, label)| *label)
}

pub fn seed_class_for(vid: u16, pid: u16) -> Option<String> {
    match (vid, pid) {
        (0x1a86, 0x7523) => Some("esp8266-oled-v2-class".to_string()),
        (0x1a86, _) => Some("tdisplay-esp32-ch9102".to_string()),
        (0x2e8a, _) => Some("waveshare-rp2040-matrix".to_string()),
        (0x303a, _) => Some("xiao-esp32s3-sense".to_string()),
        _ => None,
    }
}

