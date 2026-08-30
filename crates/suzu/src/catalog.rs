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

/// One display's zones: (y_from, y_to, phosphor) rows of the panel.
pub type DisplayZones = Vec<(usize, usize, [u8; 3])>;

/// A declared faceplate (ADR-0005): the wire id, the human side, the
/// hang. `based_on` marks a derived bundle (regenerated, never
/// hand-edited); `has_preview` says whether a captured preview ships.
#[derive(Debug, Deserialize)]
struct RingsDecl {
    #[serde(default = "default_true")]
    qualifiers: bool,
    #[serde(default = "default_true")]
    text: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct FaceplateDecl {
    name: String,
    class: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    blurb: Option<String>,
    #[serde(default)]
    mount: Option<String>,
    #[serde(default)]
    based_on: Option<String>,
    #[serde(default)]
    rings: Option<RingsDecl>,
}

/// Scan a faceplates root (`<repo>/faceplates/<class-dir>/<id>/`).
/// A bundle without a parseable declaration is skipped with a word —
/// the catalog never guesses.
fn scan_faceplates(root: PathBuf) -> Vec<FaceplateInfo> {
    let mut out = Vec::new();
    let Ok(class_dirs) = std::fs::read_dir(&root) else {
        return out;
    };
    for class_dir in class_dirs.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let Ok(bundles) = std::fs::read_dir(&class_dir) else {
            continue;
        };
        for bundle in bundles.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
            let Ok(text) = std::fs::read_to_string(bundle.join("faceplate.yaml")) else {
                continue;
            };
            let Ok(decl) = serde_yaml::from_str::<FaceplateDecl>(&text) else {
                eprintln!("catalog: faceplate declaration unreadable in {}", bundle.display());
                continue;
            };
            let has_preview = bundle.join("preview.gif").exists()
                || bundle.join("preview.png").exists();
            out.push(FaceplateInfo {
                display_name: decl.display_name.unwrap_or_else(|| decl.name.clone()),
                id: decl.name,
                class: decl.class,
                blurb: decl.blurb,
                mount: decl.mount,
                based_on: decl.based_on,
                has_preview,
                rings: RingDialect {
                    qualifiers: decl.rings.as_ref().map(|r| r.qualifiers).unwrap_or(true),
                    text: decl.rings.as_ref().map(|r| r.text).unwrap_or(true),
                },
                dir: bundle,
            });
        }
    }
    out.sort_by_key(|f| (f.based_on.is_some(), f.id.clone()));
    out
}

/// The ring voice a faceplate declares (ADR-0006): what the instance
/// may say to this face, and whether it announces integration. A
/// declaration that says nothing is heard whole.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct RingDialect {
    /// dotted signals arrive whole (WARN.disk)
    pub qualifiers: bool,
    /// a words channel exists
    pub text: bool,
}

impl Default for RingDialect {
    fn default() -> Self {
        Self { qualifiers: true, text: true }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FaceplateInfo {
    pub id: String,
    pub class: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blurb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub based_on: Option<String>,
    pub has_preview: bool,
    pub rings: RingDialect,
    #[serde(skip)]
    pub dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DisplaySpec {
    #[serde(default)]
    pub zones: Vec<DisplayZone>,
}

/// The parsed `frame:` section — what raw bytes a J shot carries and
/// how the host turns them into pixels. This is the only per-device
/// decoding knowledge in the whole tool: one generic decoder reads it,
/// so a new face ships with a manifest entry, never a code change.
#[derive(Debug, Clone, Deserialize)]
pub struct FrameSpec {
    /// Bytes on the wire — the whole-frame gate for the J ack.
    pub size: usize,
    /// `mvlsb` (1-bit column bytes, 8 vertical px, D0 = top) | `rgb24`.
    pub format: String,
    /// Bits per pixel as shipped: 1 | 24.
    pub depth: u8,
    /// `row-major` | `column-major`.
    #[serde(default)]
    pub order: Option<String>,
    /// Native pixel width.
    pub width: usize,
    /// Native pixel height.
    pub height: usize,
    /// Colors for low depths: index 0 = off, index 1 = lit. Zones
    /// (display section) override the lit color per row when present.
    #[serde(default)]
    pub palette: Vec<String>,
    /// Output view hints: rotation (deg cw) and integer upscale.
    pub render: Option<RenderHint>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RenderHint {
    /// 0 | 90 (clockwise) — how the panel is mounted.
    #[serde(default)]
    pub rotate: i32,
    /// Integer nearest-neighbour upscale so a 5×5 is lookable-at.
    #[serde(default)]
    pub scale: usize,
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
    pub frame: Option<FrameSpec>,
    /// The class's product photo, relative to the class folder.
    pub image: Option<String>,
    /// The folder the manifest was parsed from. Not declared — a
    /// class id and its folder name agree only by luck.
    #[serde(skip)]
    pub dir: Option<PathBuf>,
}

#[derive(Clone)]
pub struct Catalog {
    /// Where the signatures came from (for the boot line).
    pub source: String,
    classes: Vec<ClassSignature>,
    faceplates: Vec<FaceplateInfo>,
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
        roots.push(PathBuf::from("../../hardware/classes"));

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
            // The faceplates root sits beside `hardware/` — the repo
            // root; try the parent and the grandparent so both repo
            // layouts and SUZU_HARDWARE_DIR resolve.
            let mut faceplates = Vec::new();
            for dir in [root.parent(), root.parent().and_then(|p| p.parent())].into_iter().flatten() {
                faceplates = scan_faceplates(dir.join("faceplates"));
                if !faceplates.is_empty() {
                    break;
                }
            }
            for dir in class_dirs {
                if let Ok(text) = std::fs::read_to_string(dir.join("manifest.yaml"))
                    && let Ok(mut m) = serde_yaml::from_str::<ClassManifest>(&text) {
                        m.dir = Some(dir.clone());
                        manifests.insert(m.id.clone(), m);
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
                faceplates,
            };
        }

        Catalog {
            source: "no manifests found — using built-in seed hints".into(),
            classes: Vec::new(),
            index: HashMap::new(),
            faceplates: Vec::new(),
            manifests: HashMap::new(),
        }
    }

    /// The class's declared product photo, resolved against the
    /// catalog roots. `None` when the class declares no image or the
    /// file is missing — the workbench shows nothing rather than a
    /// placeholder pretending to be hardware.
    pub fn device_image(&self, class_id: &str) -> Option<PathBuf> {
        let manifest = self.manifests.get(class_id)?;
        let dir = manifest.dir.as_ref()?;
        let image = manifest.image.as_ref()?;
        // `dir` was resolved from the catalog root at load time — it
        // already knows where home is.
        let path = dir.join(image);
        path.exists().then_some(path)
    }

    /// The class's declared faceplates (ADR-0005): parents before
    /// derived bundles, so "first" is a sensible default.
    pub fn faceplates_for_class(&self, class_id: &str) -> Vec<&FaceplateInfo> {
        let mut out: Vec<&FaceplateInfo> = self
            .faceplates
            .iter()
            .filter(|f| f.class == class_id)
            .collect();
        out.sort_by_key(|f| (f.based_on.is_some(), f.id.clone()));
        out
    }

    /// One declared faceplate of a class, by id.
    pub fn faceplate(&self, class_id: &str, id: &str) -> Option<&FaceplateInfo> {
        self.faceplates
            .iter()
            .find(|f| f.class == class_id && f.id == id)
    }

    /// A faceplate's preview capture, gif first, png as the older
    /// fallback — `None` means the chooser degrades to pictogram and
    /// words (ADR-0005: a missing preview is graceful).
    pub fn faceplate_preview(&self, class_id: &str, id: &str) -> Option<PathBuf> {
        let dir = &self.faceplate(class_id, id)?.dir;
        for name in ["preview.gif", "preview.png"] {
            let path = dir.join(name);
            if path.exists() {
                return Some(path);
            }
        }
        None
    }

    /// VID/PID lookup — used when the port is silent (fresh firmware).
    pub fn class_by_vidpid(&self, vid: u16, pid: u16) -> Option<&ClassSignature> {
        let bucket = self.index.get(&vid)?;
        bucket
            .iter()
            .find(|(mp, _)| mp.is_none_or(|p| p == pid))
            .map(|(_, idx)| &self.classes[*idx])
    }

    /// The class id a port belongs to: signature match first, seed
    /// hints second. What screenshots key their manifest lookup on.
    pub fn class_id_for(&self, vid: u16, pid: u16) -> Option<String> {
        self.class_by_vidpid(vid, pid)
            .map(|c| c.id.clone())
            .or_else(|| seed_class_for(vid, pid))
    }

    /// The frame law of a class: what its J shot carries and how to
    /// decode it.
    pub fn frame(&self, class_id: &str) -> Option<&FrameSpec> {
        self.manifests.get(class_id).and_then(|m| m.frame.as_ref())
    }

    /// The display zones of a class's manifest: (first_row, last_row,
    /// rgb) — what a faithful screenshot colors.
    pub fn display_zones(&self, class_id: &str) -> DisplayZones {
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

