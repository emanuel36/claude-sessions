//! Black-box tests of `install.sh`, run by `sh` in a temp home.
//!
//! The script sees a temp `HOME` and XDG directories, a PATH of fakes plus `/usr/bin:/bin` (so
//! never the real `~/.local/bin` and its `uv`), a fake `uv` that only logs, and a fake `tmux`
//! (so never the real `tmux -L claude`).

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use cst::claude::install::{is_ours, EVENTS};
use serde_json::Value;
use support::{publish, release_binary};
use tempfile::TempDir;

/// What the fake `uv tool list` prints when the Python version is installed.
const PYTHON_LISTED: &str = "claude-sessions v1.4.0\n- cs\n- cst\n";
/// What it prints otherwise.
const OTHER_TOOLS: &str = "ruff v0.6.0\n- ruff\n";

/// A temp home, its fakes and a release to install from.
struct Rig {
    dir: TempDir,
}

impl Rig {
    /// A home with a fake `tmux` printing `tmux 3.4` and no server running, and a fake `uv`
    /// listing other tools.
    fn new() -> Self {
        let rig = Self {
            dir: TempDir::new().expect("temp dir"),
        };
        fs::create_dir_all(rig.fakes()).expect("mkdir");
        fs::create_dir_all(rig.home()).expect("mkdir");
        rig.fake_tmux("tmux 3.4", false);
        rig.fake_uv(OTHER_TOOLS);
        rig
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn home(&self) -> PathBuf {
        self.path("home")
    }

    fn fakes(&self) -> PathBuf {
        self.path("fakes")
    }

    fn release(&self) -> PathBuf {
        self.path("release")
    }

    /// Where `install.sh` puts `cst` by default.
    fn installed(&self) -> PathBuf {
        self.home().join(".local/bin/cst")
    }

    fn settings(&self) -> PathBuf {
        self.path("claude/settings.json")
    }

    fn write_fake(&self, name: &str, script: &str) {
        let path = self.fakes().join(name);
        fs::write(&path, script).expect("write fake");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    /// A `tmux` that prints `version` and has a `-L claude` server running or not; it logs its
    /// arguments.
    fn fake_tmux(&self, version: &str, running: bool) {
        let log = self.path("tmux.log");
        let status = if running { 0 } else { 1 };
        self.write_fake(
            "tmux",
            &format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\n\
                 case \"$1\" in -V) echo '{version}' ;; *) exit {status} ;; esac\n",
                log.display()
            ),
        );
    }

    /// A `uv` whose `tool list` prints `listed`; it logs its arguments.
    fn fake_uv(&self, listed: &str) {
        let log = self.path("uv.log");
        self.write_fake(
            "uv",
            &format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\n\
                 [ \"$*\" = 'tool list' ] && printf '%s' '{listed}'\nexit 0\n",
                log.display()
            ),
        );
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap_or_default()
    }

    /// Runs `sh install.sh` with nothing of the caller's environment but `extra`.
    fn run(&self, extra: &[(&str, &str)]) -> Output {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh");
        let path = format!("{}:/usr/bin:/bin", self.fakes().display());
        let mut cmd = Command::new("/bin/sh");
        cmd.arg(script)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_STATE_HOME", self.path("xdg-state"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("TMUX_TMPDIR", self.path("tmux"))
            .env("CST_RUNTIME_DIR", self.path("runtime"))
            .env("CST_STATE_DIR", self.path("state"))
            .env("CLAUDE_CONFIG_DIR", self.path("claude"))
            .env("CLAUDE_SESSIONS_STATE", self.path("python-state"))
            .env(
                "CST_RELEASE_BASE",
                format!("file://{}", self.release().display()),
            );
        cmd.envs(extra.iter().copied());
        cmd.output().expect("run install.sh")
    }

    /// The commands of our hooks in settings.json, one per event registered.
    fn hooks(&self) -> Vec<String> {
        let data: Value =
            serde_json::from_slice(&fs::read(self.settings()).expect("read settings"))
                .expect("settings json");
        EVENTS
            .iter()
            .flat_map(|e| data["hooks"][e].as_array().cloned().unwrap_or_default())
            .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
            .filter_map(|h| h["command"].as_str().map(str::to_owned))
            .filter(|c| is_ours(c))
            .collect()
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Fails with the script's output unless it exited with `code`.
fn assert_exit(out: &Output, code: i32) {
    assert_eq!(
        out.status.code(),
        Some(code),
        "stdout:\n{}\nstderr:\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
}

#[test]
fn install_sh_downloads_verifies_and_registers() {
    let rig = Rig::new();
    let asset = release_binary("release v9.9.9");
    publish(&rig.release(), "v9.9.9", &asset, None);

    let out = rig.run(&[]);

    assert_exit(&out, 0);
    assert!(fs::read(rig.installed()).expect("read cst") == asset);
    let hooks = rig.hooks();
    assert_eq!(hooks.len(), 11);
    let path = fs::canonicalize(rig.installed()).expect("canonical");
    assert!(hooks.iter().all(|h| h.contains(&*path.to_string_lossy())));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("v9.9.9"), "{stdout}");
    assert!(stdout.contains("is not on your PATH"), "{stdout}");
    assert!(!stdout.contains("quit and reopen"), "{stdout}");
    assert!(!rig.log("uv.log").contains("uninstall"));
}

#[test]
fn install_sh_respects_xdg_bin_home() {
    let rig = Rig::new();
    publish(&rig.release(), "v9.9.9", &release_binary("v9.9.9"), None);
    let bin = rig.path("bin");
    let path = format!("{}:/usr/bin:/bin", bin.display());

    let out = rig.run(&[
        ("XDG_BIN_HOME", &*bin.to_string_lossy()),
        ("PATH", &format!("{}:{path}", rig.fakes().display())),
    ]);

    assert_exit(&out, 0);
    assert!(bin.join("cst").exists());
    assert!(!rig.installed().exists());
    assert!(!text(&out.stdout).contains("is not on your PATH"));
}

#[test]
fn install_sh_refuses_a_bad_checksum() {
    let rig = Rig::new();
    let wrong = "0".repeat(64);
    publish(&rig.release(), "v9.9.9", &release_binary("x"), Some(&wrong));

    let out = rig.run(&[]);

    assert_exit(&out, 1);
    assert!(
        text(&out.stderr).contains("SHA-256"),
        "{}",
        text(&out.stderr)
    );
    assert!(!rig.installed().exists());
    assert!(!rig.settings().exists());
}

#[test]
fn install_sh_refuses_old_tmux() {
    let rig = Rig::new();
    rig.fake_tmux("tmux 3.2", false);
    publish(&rig.release(), "v9.9.9", &release_binary("x"), None);

    let out = rig.run(&[]);

    assert_exit(&out, 1);
    let err = text(&out.stderr);
    assert!(
        err.contains("tmux 3.3 or later is needed; this is tmux 3.2"),
        "{err}"
    );
    assert!(!rig.installed().exists());
}

#[test]
fn install_sh_removes_the_python_version() {
    let rig = Rig::new();
    rig.fake_uv(PYTHON_LISTED);
    publish(&rig.release(), "v9.9.9", &release_binary("x"), None);

    let out = rig.run(&[]);

    assert_exit(&out, 0);
    assert_eq!(
        rig.log("uv.log"),
        "tool list\ntool uninstall claude-sessions\n"
    );
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("uv tool uninstall claude-sessions"),
        "{stdout}"
    );
    assert!(rig.installed().exists());
}

#[test]
fn install_sh_says_to_reopen_a_running_cst_and_leaves_it_running() {
    let rig = Rig::new();
    rig.fake_tmux("tmux 3.4", true);
    publish(&rig.release(), "v9.9.9", &release_binary("x"), None);

    let out = rig.run(&[]);

    assert_exit(&out, 0);
    let stdout = text(&out.stdout);
    assert!(stdout.contains("quit and reopen cst"), "{stdout}");
    let calls = rig.log("tmux.log");
    assert!(calls.contains("-L claude has-session"), "{calls}");
    assert!(!calls.contains("kill"), "{calls}");
}

/// Slow: builds the release binary of this checkout.
#[test]
#[ignore]
fn install_sh_from_source_installs_and_registers() {
    let rig = Rig::new();
    let source = env!("CARGO_MANIFEST_DIR");
    // The toolchain and the registry cache live under the real home, which the script does not
    // see
    let home = std::env::var("HOME").unwrap_or_default();
    let real = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    let cargo_home = real("CARGO_HOME", &format!("{home}/.cargo"));
    let rustup_home = real("RUSTUP_HOME", &format!("{home}/.rustup"));
    let toolchain = real("RUSTUP_TOOLCHAIN", "stable");

    let out = rig.run(&[
        ("CST_SOURCE", source),
        ("CARGO_HOME", &cargo_home),
        ("RUSTUP_HOME", &rustup_home),
        ("RUSTUP_TOOLCHAIN", &toolchain),
    ]);

    assert_exit(&out, 0);
    let built = Path::new(source).join("target/release/cst");
    assert!(fs::read(rig.installed()).expect("read cst") == fs::read(built).expect("read"));
    assert_eq!(rig.hooks().len(), 11);
}
