//! Black-box tests of `cst update`, against a release published in a temp dir.

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use cst::claude::install::{is_ours, EVENTS};
use serde_json::Value;
use support::{command_of, env, output_retrying, publish, release_binary, TestEnv};

/// An installed copy of `cst` and a release directory to update it from.
struct Rig {
    env: TestEnv,
}

impl Rig {
    /// Installs a copy of the test binary at [`Rig::installed`].
    fn new() -> Self {
        let rig = Self { env: env() };
        fs::create_dir_all(rig.installed().parent().expect("bin dir")).expect("mkdir");
        fs::copy(env!("CARGO_BIN_EXE_cst"), rig.installed()).expect("copy the binary");
        fs::set_permissions(rig.installed(), fs::Permissions::from_mode(0o755)).expect("chmod");
        rig
    }

    fn installed(&self) -> PathBuf {
        self.env.path().join("bin/cst")
    }

    fn release(&self) -> PathBuf {
        self.env.path().join("release")
    }

    /// Runs `cst update` from the installed copy.
    fn update(&self) -> Output {
        let mut cmd = command_of(&self.installed(), &self.env, &["update"]);
        cmd.env(
            "CST_RELEASE_BASE",
            format!("file://{}", self.release().display()),
        )
        // The test may run where the Python version's variables are set
        .env_remove("XDG_STATE_HOME")
        .env("CLAUDE_SESSIONS_STATE", self.env.path().join("no-python"));
        output_retrying(&mut cmd)
    }

    fn settings(&self) -> PathBuf {
        self.env.claude_dir().join("settings.json")
    }

    /// The names in the binary's directory.
    fn bin_dir(&self) -> Vec<String> {
        let dir = self.installed().parent().expect("bin dir").to_path_buf();
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read the bin dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn test_binary() -> Vec<u8> {
    fs::read(env!("CARGO_BIN_EXE_cst")).expect("read the test binary")
}

/// The hook commands registered for each event.
fn hook_commands(settings: &Path) -> Vec<String> {
    let data: Value =
        serde_json::from_slice(&fs::read(settings).expect("read settings")).expect("settings json");
    EVENTS
        .iter()
        .flat_map(|e| data["hooks"][e].as_array().cloned().unwrap_or_default())
        .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
        .filter_map(|h| h["command"].as_str().map(str::to_owned))
        .filter(|c| is_ours(c))
        .collect()
}

#[test]
fn update_replaces_the_binary_after_verifying() {
    let rig = Rig::new();
    let asset = release_binary("release v9.9.9");
    publish(&rig.release(), "v9.9.9", &asset, None);

    let out = rig.update();

    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).ends_with("updated to v9.9.9; quit and reopen cst\n"),
        "{}",
        text(&out.stdout)
    );
    assert!(fs::read(rig.installed()).expect("read") == asset);
    let mode = fs::metadata(rig.installed())
        .expect("meta")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    assert_eq!(rig.bin_dir(), vec!["cst"]);
    // The new binary registered itself by its path, not the old one's "(deleted)" one
    let path = fs::canonicalize(rig.installed()).expect("canonical");
    let commands = hook_commands(&rig.settings());
    assert_eq!(commands.len(), EVENTS.len());
    for command in commands {
        assert_eq!(command, format!("{} hook", path.display()));
    }
}

#[test]
fn update_refuses_a_bad_checksum() {
    let rig = Rig::new();
    let wrong = "0".repeat(64);
    publish(&rig.release(), "v9.9.9", &release_binary("x"), Some(&wrong));

    let out = rig.update();

    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.contains("checksum"), "{err}");
    assert!(fs::read(rig.installed()).expect("read") == test_binary());
    assert_eq!(rig.bin_dir(), vec!["cst"]);
    assert!(!rig.settings().exists());
}

#[test]
fn update_says_up_to_date() {
    let rig = Rig::new();
    let tag = format!("v{}", cst::VERSION);
    publish(&rig.release(), &tag, &release_binary("same"), None);

    let out = rig.update();

    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        format!("already up to date ({})\n", cst::VERSION)
    );
    assert!(fs::read(rig.installed()).expect("read") == test_binary());
    assert!(!rig.settings().exists());
}

#[test]
fn update_fails_cleanly_without_a_release() {
    let rig = Rig::new();

    let out = rig.update();

    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).starts_with("error: "),
        "{}",
        text(&out.stderr)
    );
    assert!(fs::read(rig.installed()).expect("read") == test_binary());
    assert_eq!(rig.bin_dir(), vec!["cst"]);
}
