//! `suzu install` — the Resident deploys itself.
//!
//! The compiled form of the deployment recipe proven across the Linux
//! testbeds (`docs/test-platforms.md`) and stone-halcyon-savanna: the
//! running binary copies itself to `<prefix>/bin`, the resources to
//! `<prefix>/share/suzu`, writes the udev rule and the service
//! definition for whichever init the host runs (systemd, or OpenRC on
//! musl hosts), then enables and verifies the service. Where the
//! install runs under sudo, the service user is the invoking user.
//!
//! `scripts/install-linux.sh` is the ancestor reference for this
//! procedure (ADR-0008); the OpenRC service file is its own promotion
//! out of the Alpine testbed, log-ownership lesson included.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

const SYSTEMD_UNIT: &str = include_str!("../../../packaging/systemd/suzu@.service");
const OPENRC_SCRIPT: &str = include_str!("../../../packaging/openrc/suzu");
const UDEV_RULE: &str = include_str!("../../../packaging/udev/60-suzu.rules");

const HW_GROUP: &str = "suzu-hw";
const STATE_ROOT: &str = "/var/lib/suzu";
const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/suzu@.service";
const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/60-suzu.rules";

/// The init systems the testbeds taught, in detection order.
#[derive(Debug, PartialEq, Clone, Copy)]
enum Init {
    Systemd,
    OpenRC,
}

impl Init {
    fn name(self) -> &'static str {
        match self {
            Init::Systemd => "systemd",
            Init::OpenRC => "OpenRC",
        }
    }
}

fn detect_init(probe: &dyn Fn(&str) -> bool) -> Option<Init> {
    if probe("systemctl") {
        Some(Init::Systemd)
    } else if probe("openrc-run") {
        Some(Init::OpenRC)
    } else {
        None
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum Action {
    Install,
    Verify,
    Uninstall,
}

struct Options {
    action: Action,
    user: Option<String>,
    prefix: Option<PathBuf>,
    start: bool,
}

fn parse_args(args: &[String]) -> Result<Options> {
    let mut opts = Options {
        action: Action::Install,
        user: None,
        prefix: None,
        start: true,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--user" => {
                i += 1;
                opts.user = Some(
                    args.get(i)
                        .context("--user needs an account name")?
                        .clone(),
                );
            }
            "--prefix" => {
                i += 1;
                let p = PathBuf::from(args.get(i).context("--prefix needs a path")?);
                // Textual, not Path::is_absolute: the prefix names a
                // Unix path even when suzu is compiled on Windows.
                if !p.to_string_lossy().starts_with('/') {
                    bail!("--prefix must be an absolute path");
                }
                opts.prefix = Some(p);
            }
            "--no-start" => opts.start = false,
            "--verify" => opts.action = Action::Verify,
            "--uninstall" => opts.action = Action::Uninstall,
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unknown option {other:?} — see `suzu install --help`"),
        }
        i += 1;
    }
    Ok(opts)
}

fn print_help() {
    println!(
        "Usage: sudo suzu install [options]\n\
         \n\
         Deploy the running binary as the Resident service (root required).\n\
         \n\
         Options:\n\
         \x20 --user NAME     Unix account that runs the Resident (default: the sudo-er)\n\
         \x20 --prefix PATH   Installation prefix (default: the binary's, else /usr/local)\n\
         \x20 --no-start      Install and enable, but do not start now\n\
         \x20 --verify        Check an existing installation and exit\n\
         \x20 --uninstall     Stop the service and remove binary, resources, and unit\n\
         \x20 -h, --help      Show this help"
    );
}

/// Substitute the `@NAME@` placeholders of a packaging template.
fn render_template(tpl: &str, bindings: &[(&str, &str)]) -> String {
    let mut out = tpl.to_string();
    for (name, value) in bindings {
        out = out.replace(name, value);
    }
    out
}

struct Plan {
    prefix: PathBuf,
    user: String,
    init: Init,
    state_dir: PathBuf,
}

impl Plan {
    fn bindir(&self) -> PathBuf {
        self.prefix.join("bin")
    }
    fn resource_dir(&self) -> PathBuf {
        self.prefix.join("share/suzu")
    }
}

/// One numbered step of the deployment, reported as it runs. A failed
/// step reports what it was doing and stops the run; nothing about the
/// failure is hidden from the operator.
struct Runner {
    index: u32,
    total: u32,
}

impl Runner {
    fn new(total: u32) -> Self {
        Self { index: 0, total }
    }
    fn step<T, F>(&mut self, label: &str, run: F) -> Result<T>
    where
        F: FnOnce() -> Result<T>,
    {
        self.index += 1;
        println!("[{}/{}] {}", self.index, self.total, label);
        run()
    }
}

/// Run a shell command, surfacing its stderr on failure.
fn sh(cmd: &str) -> Result<()> {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .context("run sh")?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    bail!("`{cmd}` failed ({}): {}", out.status, stderr.trim())
}

/// Copy a resource tree, directories 0755, files 0644. The installer
/// runs as root, so fresh files are root-owned by construction.
fn copy_tree(src: &Path, dest: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src).with_context(|| format!("read {}", src.display()))? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            std::fs::create_dir_all(&to)?;
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .with_context(|| format!("copy {}", from.display()))?;
            set_file_mode(&to, 0o644);
        }
    }
    set_dir_mode(dest, 0o755);
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_file_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(target_os = "linux")]
fn set_dir_mode(path: &Path, mode: u32) {
    set_file_mode(path, mode);
}

#[cfg(not(target_os = "linux"))]
fn set_file_mode(_path: &Path, _mode: u32) {}
#[cfg(not(target_os = "linux"))]
fn set_dir_mode(_path: &Path, _mode: u32) {}

/// The entry point for `suzu install` (and its verify/uninstall flags).
pub fn run(args: &[String]) -> Result<()> {
    let opts = parse_args(args)?;
    if !cfg!(target_os = "linux") {
        bail!(
            "suzu install deploys the Resident as a Linux service; this host \
             is not Linux. On the bench, run `suzu serve` directly."
        );
    }
    match opts.action {
        Action::Verify => verify(&opts),
        Action::Uninstall => uninstall(&opts),
        Action::Install => install(&opts),
    }
}

/// Resolve everything the deployment needs before touching the host.
fn plan(opts: &Options) -> Result<Plan> {
    let user: String = match &opts.user {
        Some(u) => u.clone(),
        None => std::env::var("SUDO_USER")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default(),
    };
    if user.is_empty() || user == "root" {
        bail!(
            "no service user resolved — run under sudo as the service user, \
             or pass --user NAME"
        );
    }
    let init = detect_init(&|name| which(name))
        .context("no systemd or OpenRC found — this host has no supported init")?;
    let prefix = opts
        .prefix
        .clone()
        .or_else(default_prefix)
        .unwrap_or_else(|| PathBuf::from("/usr/local"));
    let state_dir = PathBuf::from(STATE_ROOT).join(&user);
    Ok(Plan {
        prefix,
        user,
        init,
        state_dir,
    })
}

/// The binary's own prefix when it already lives in a `<prefix>/bin`
/// layout (an upgrade), else /usr/local (a fresh install).
fn default_prefix() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bin = exe.parent()?;
    if bin.file_name()?.to_str()? != "bin" {
        return None;
    }
    bin.parent().map(Path::to_path_buf)
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| p.to_string_lossy().split(':').any(|dir| {
            Path::new(dir).join(name).is_file()
        }))
        .unwrap_or(false)
        || Path::new("/usr/sbin").join(name).is_file()
        || Path::new("/sbin").join(name).is_file()
}

/// Where the resources to install live: a checkout's relative layout, or
/// an already-installed share dir (an upgrade through the installed
/// binary itself).
fn resource_source() -> Result<PathBuf> {
    let candidate = crate::paths::resource_dir();
    if candidate.join("hardware/classes").is_dir() && candidate.join("firmware").is_dir() {
        return Ok(candidate);
    }
    bail!(
        "no resources found under {} (need hardware/classes and firmware) — \
         run from a repo checkout, or set SUZU_RESOURCE_DIR",
        candidate.display()
    );
}

fn install(opts: &Options) -> Result<()> {
    if sh("test \"$(id -u)\" = 0").is_err() {
        bail!("install needs root — run: sudo suzu install");
    }
    let plan = plan(opts)?;
    let source = resource_source()?;
    let exe = std::env::current_exe().context("locate the running binary")?;
    let bindir = plan.bindir();
    let resdir = plan.resource_dir();
    let unit = format!("suzu@{}.service", plan.user);

    // Preflight before anything changes: the init's system dirs must
    // exist for what we are about to write.
    match plan.init {
        Init::Systemd if !Path::new("/etc/systemd/system").is_dir() => {
            bail!("/etc/systemd/system missing — systemd present but not installed system-wide?")
        }
        Init::OpenRC if !Path::new("/etc/init.d").is_dir() => {
            bail!("/etc/init.d missing — OpenRC present but not installed system-wide?")
        }
        _ => {}
    }

    let mut runner = Runner::new(6);
    runner.step(&format!("Ensuring the {HW_GROUP} group"), || {
        sh(&format!(
            "getent group {HW_GROUP} >/dev/null || groupadd --system {HW_GROUP}"
        ))
    })?;
    runner.step("Installing the binary", || {
        std::fs::create_dir_all(&bindir)?;
        // Stage beside the destination and rename over it: the running
        // Resident holds the old inode, so writing in place fails with
        // ETXTBSY — and an atomic swap upgrades a live service cleanly.
        let staged = bindir.join(".suzu.new");
        std::fs::copy(&exe, &staged)
            .with_context(|| format!("copy {}", exe.display()))?;
        set_file_mode(&staged, 0o755);
        std::fs::rename(&staged, bindir.join("suzu"))?;
        Ok(())
    })?;
    runner.step("Installing the resources", || {
        std::fs::create_dir_all(resdir.join("hardware"))?;
        std::fs::create_dir_all(resdir.join("firmware"))?;
        copy_tree(&source.join("hardware"), &resdir.join("hardware"))?;
        copy_tree(&source.join("firmware"), &resdir.join("firmware"))?;
        Ok(())
    })?;
    runner.step("Installing the udev rule", || {
        std::fs::create_dir_all("/etc/udev/rules.d")?;
        std::fs::write(UDEV_RULE_PATH, UDEV_RULE)?;
        if which("udevadm") {
            sh("udevadm control --reload-rules 2>/dev/null || true")?;
        }
        Ok(())
    })?;
    runner.step(&format!(
        "Writing the {} service definition",
        plan.init.name()
    ), || {
        std::fs::create_dir_all(plan.state_dir.join("captures"))?;
        match plan.init {
            Init::Systemd => {
                let rendered = render_template(
                    SYSTEMD_UNIT,
                    &[
                        ("@SUZU_BINDIR@", &bindir.to_string_lossy()),
                        ("@SUZU_RESOURCE_DIR@", &resdir.to_string_lossy()),
                    ],
                );
                std::fs::write(SYSTEMD_UNIT_PATH, rendered)?;
                sh("systemctl daemon-reload")?;
                sh(&format!("systemctl enable {unit}"))?;
            }
            Init::OpenRC => {
                let rendered = render_template(
                    OPENRC_SCRIPT,
                    &[
                        ("@SUZU_BINDIR@", &bindir.to_string_lossy()),
                        ("@SUZU_RESOURCE_DIR@", &resdir.to_string_lossy()),
                        ("@SUZU_STATE_DIR@", &plan.state_dir.to_string_lossy()),
                        ("@SUZU_SERVICE_USER@", &plan.user),
                    ],
                );
                std::fs::write("/etc/init.d/suzu", rendered)?;
                set_file_mode(Path::new("/etc/init.d/suzu"), 0o755);
                sh("rc-update add suzu default 2>/dev/null || true")?;
            }
        }
        Ok(())
    })?;
    runner.step(&format!("Starting the Resident ({})", plan.init.name()), || {
        if !opts.start {
            println!("    --no-start: left stopped");
            return Ok(());
        }
        match plan.init {
            Init::Systemd => sh(&format!("systemctl restart {unit}"))?,
            Init::OpenRC => sh("rc-service suzu restart")?,
        }
        Ok(())
    })?;

    verify_plan(&plan, opts.start)?;
    println!("done — `suzu list` watches the fleet, `journalctl -u {unit}` (or the OpenRC log) reads the Resident");
    Ok(())
}

fn verify(opts: &Options) -> Result<()> {
    let plan = plan(opts)?;
    verify_plan(&plan, opts.start)
}

fn verify_plan(plan: &Plan, started: bool) -> Result<()> {
    let bin = plan.bindir().join("suzu");
    if !bin.is_file() {
        bail!("{} is missing — not installed?", bin.display());
    }
    if !plan
        .resource_dir()
        .join("hardware/classes")
        .is_dir()
    {
        bail!(
            "{} is missing — resources not installed?",
            plan.resource_dir().join("hardware/classes").display()
        );
    }
    match plan.init {
        Init::Systemd => {
            let unit = format!("suzu@{}.service", plan.user);
            sh(&format!("systemctl is-enabled --quiet {unit}"))?;
            if started {
                sh(&format!("systemctl is-active --quiet {unit}"))?;
            }
        }
        Init::OpenRC => {
            sh("rc-service suzu status")?;
        }
    }
    println!(
        "verified: {} service user {}, prefix {}",
        plan.init.name(),
        plan.user,
        plan.prefix.display()
    );
    Ok(())
}

fn uninstall(opts: &Options) -> Result<()> {
    if sh("test \"$(id -u)\" = 0").is_err() {
        bail!("uninstall needs root — run: sudo suzu uninstall");
    }
    let plan = plan(opts)?;
    let unit = format!("suzu@{}.service", plan.user);
    let mut runner = Runner::new(4);
    runner.step("Stopping the Resident", || {
        match plan.init {
            Init::Systemd => {
                sh(&format!("systemctl disable --now {unit} 2>/dev/null || true"))?;
                let _ = std::fs::remove_file(SYSTEMD_UNIT_PATH);
                sh("systemctl daemon-reload")?;
            }
            Init::OpenRC => {
                sh("rc-service suzu stop 2>/dev/null || true")?;
                sh("rc-update del suzu default 2>/dev/null || true")?;
                let _ = std::fs::remove_file("/etc/init.d/suzu");
            }
        }
        Ok(())
    })?;
    runner.step("Removing the binary and resources", || {
        let _ = std::fs::remove_file(plan.bindir().join("suzu"));
        let _ = std::fs::remove_dir_all(plan.resource_dir());
        Ok(())
    })?;
    runner.step("Removing the udev rule", || {
        let _ = std::fs::remove_file(UDEV_RULE_PATH);
        if which("udevadm") {
            sh("udevadm control --reload-rules 2>/dev/null || true")?;
        }
        Ok(())
    })?;
    runner.step("Preserving service state", || {
        println!(
            "    kept {} (pass --purge-state to scripts/install-linux.sh to remove)",
            plan.state_dir.display()
        );
        Ok(())
    })?;
    println!("uninstalled — the {HW_GROUP} group was preserved");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_render_every_placeholder() {
        let plan_user = "test";
        let rendered = render_template(
            OPENRC_SCRIPT,
            &[
                ("@SUZU_BINDIR@", "/usr/local/bin"),
                ("@SUZU_RESOURCE_DIR@", "/usr/local/share/suzu"),
                ("@SUZU_STATE_DIR@", "/var/lib/suzu/test"),
                ("@SUZU_SERVICE_USER@", plan_user),
            ],
        );
        assert!(!rendered.contains('@'), "placeholder survived: {rendered}");
        // The bench lesson stays baked in: logs pre-created writable by
        // the service user.
        assert!(rendered.contains("checkpath --file --owner test"));
        assert!(rendered.contains("command_user=\"test\""));
    }

    #[test]
    fn systemd_template_renders_to_the_installed_shape() {
        let rendered = render_template(
            SYSTEMD_UNIT,
            &[
                ("@SUZU_BINDIR@", "/usr/local/bin"),
                ("@SUZU_RESOURCE_DIR@", "/usr/local/share/suzu"),
            ],
        );
        assert!(rendered.contains("ExecStart=/usr/local/bin/suzu serve"));
        assert!(rendered.contains("Environment=SUZU_RESOURCE_DIR=/usr/local/share/suzu"));
        assert!(!rendered.contains('@'), "placeholder survived");
    }

    #[test]
    fn init_detection_prefers_systemd_then_openrc() {
        let both = |name: &str| name == "systemctl" || name == "openrc-run";
        assert_eq!(detect_init(&both), Some(Init::Systemd));
        let openrc_only = |name: &str| name == "openrc-run";
        assert_eq!(detect_init(&openrc_only), Some(Init::OpenRC));
        assert_eq!(detect_init(&|_| false), None);
    }

    #[test]
    fn arguments_parse_with_defaults_and_flags() {
        let none: Vec<String> = vec![];
        let o = parse_args(&none).unwrap();
        assert_eq!(o.action, Action::Install);
        assert!(o.start);
        assert!(o.user.is_none() && o.prefix.is_none());

        let o = parse_args(&[
            "--user".into(),
            "stone".into(),
            "--prefix".into(),
            "/opt/suzu".into(),
            "--no-start".into(),
        ])
        .unwrap();
        assert_eq!(o.user.as_deref(), Some("stone"));
        assert_eq!(o.prefix, Some(PathBuf::from("/opt/suzu")));
        assert!(!o.start);

        let o = parse_args(&["--verify".into()]).unwrap();
        assert_eq!(o.action, Action::Verify);
        let o = parse_args(&["--uninstall".into()]).unwrap();
        assert_eq!(o.action, Action::Uninstall);

        assert!(parse_args(&["--wat".into()]).is_err());
        assert!(parse_args(&["--prefix".into(), "relative".into()]).is_err());
    }

    // The platform gate is the one behavior testable on the Windows
    // bench: off-Linux, install refuses before touching anything.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn install_refuses_off_linux() {
        let err = run(&[]).unwrap_err().to_string();
        assert!(err.contains("Linux"), "{err}");
    }
}
