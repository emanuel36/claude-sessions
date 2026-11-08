//! `cst update`: installs the latest release in place of the running binary.
//!
//! The release is found and downloaded with `curl`, checked against its published SHA-256 and
//! renamed over the binary, so a failure at any step leaves the old one as it was. The new
//! binary then registers itself with `<new> install`: the running process cannot do it, as its
//! own path now names a deleted file. Open panes keep the old binary until `cst` runs again,
//! which restarts the sidebars and the landing page with the new one.

use std::cmp::Ordering;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context};
use sha2::{Digest, Sha256};

/// Where releases are published, unless `CST_RELEASE_BASE` names another place.
pub const DEFAULT_BASE: &str = "https://github.com/emanuel36/claude-sessions/releases";

/// The release asset built for this machine: `cst-<arch>-unknown-linux-musl`.
pub fn asset_name() -> String {
    format!("cst-{}-unknown-linux-musl", std::env::consts::ARCH)
}

/// The tag in the URL `<base>/latest` redirects to, `…/releases/tag/<tag>`, if it is one.
pub fn tag_from_url(url: &str) -> Option<&str> {
    let (_, tag) = url.trim().rsplit_once("/tag/")?;
    valid_tag(tag).then_some(tag)
}

/// Whether `tag` can be put in a download URL: not empty, no `/`, no whitespace.
fn valid_tag(tag: &str) -> bool {
    !tag.is_empty() && !tag.contains('/') && !tag.contains(char::is_whitespace)
}

/// Whether the release `tag` (`v2.0.0`, or `2.0.0`) is `version`.
fn is_current(tag: &str, version: &str) -> bool {
    tag.strip_prefix('v').unwrap_or(tag) == version
}

/// Parses `MAJOR.MINOR.PATCH`, with an optional leading `v`.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.strip_prefix('v').unwrap_or(text).split('.');
    let mut next = || parts.next()?.parse().ok();
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

/// Orders the release `tag` (`v2.0.0`, or `2.0.0`) against `version`, the running build.
///
/// Both are compared as numbers, so `v1.10.0` is newer than `1.9.0`. When either is not
/// `MAJOR.MINOR.PATCH`, the tag counts as newer unless it is the same release, so an
/// unfamiliar tag is still installed, as it was before versions were compared.
pub fn compare_release(tag: &str, version: &str) -> Ordering {
    match (parse_version(tag), parse_version(version)) {
        (Some(tag), Some(version)) => tag.cmp(&version),
        _ if is_current(tag, version) => Ordering::Equal,
        _ => Ordering::Greater,
    }
}

/// The SHA-256 that `sums`, in `sha256sum` format (`<hex>  <name>`), gives for the file `name`.
pub fn checksum_for<'a>(sums: &'a str, name: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let (hex, file) = line.split_once(' ')?;
        // `sha256sum -b` marks the name with `*`; the default with a second space
        let file = file.strip_prefix([' ', '*']).unwrap_or(file);
        let well_formed = hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit());
        (well_formed && file == name).then_some(hex)
    })
}

/// The SHA-256 of `bytes`, in lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Runs `cst update`. Returns the exit code: 0 when updated or already up to date, 1 when
/// anything failed, with the reason on stderr and the binary as it was.
pub fn update() -> i32 {
    match run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    }
}

fn run() -> anyhow::Result<()> {
    let base = std::env::var("CST_RELEASE_BASE")
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE.to_owned());
    let base = base.trim_end_matches('/');
    let tag = latest_tag(base)?;
    match compare_release(&tag, crate::VERSION) {
        Ordering::Equal => {
            println!("already up to date ({})", crate::VERSION);
            return Ok(());
        }
        Ordering::Less => {
            println!("already newer than the latest release ({tag})");
            return Ok(());
        }
        Ordering::Greater => {}
    }
    let name = asset_name();
    let url = format!("{base}/download/{tag}/{name}");
    let binary = fetch(&url)?;
    let sums = fetch(&format!("{url}.sha256"))?;
    let sums = String::from_utf8_lossy(&sums);
    let Some(want) = checksum_for(&sums, &name) else {
        bail!("{url}.sha256 has no checksum for {name}; nothing was changed");
    };
    if !sha256_hex(&binary).eq_ignore_ascii_case(want) {
        bail!("{name} does not match its checksum; nothing was changed");
    }
    let exe = std::env::current_exe()
        .and_then(fs::canonicalize)
        .context("cannot find the cst executable")?;
    replace(&exe, &binary).with_context(|| format!("cannot replace {}", exe.display()))?;
    let status = Command::new(&exe)
        .arg("install")
        .status()
        .with_context(|| format!("cannot run {} install", exe.display()))?;
    if !status.success() {
        bail!("{tag} is installed, but {} install failed", exe.display());
    }
    println!("updated to {tag}; run cst again to use it, your sessions keep running");
    Ok(())
}

/// The latest release's tag: the file `<base>/latest/tag` for a `file://` base, else where
/// `<base>/latest` redirects to.
fn latest_tag(base: &str) -> anyhow::Result<String> {
    if base.starts_with("file://") {
        let url = format!("{base}/latest/tag");
        let tag = String::from_utf8_lossy(&fetch(&url)?).trim().to_owned();
        if !valid_tag(&tag) {
            bail!("{url} does not hold a release tag");
        }
        return Ok(tag);
    }
    let url = format!("{base}/latest");
    let effective = curl(
        &["-fsSLI", "-o", "/dev/null", "-w", "%{url_effective}"],
        &url,
    )?;
    let effective = String::from_utf8_lossy(&effective);
    match tag_from_url(&effective) {
        Some(tag) => Ok(tag.to_owned()),
        None => bail!("no release found at {url}"),
    }
}

/// The body of `url`.
fn fetch(url: &str) -> anyhow::Result<Vec<u8>> {
    curl(&["-fsSL"], url)
}

/// The arguments of a `curl` call: `options` and `url`, after limits on the time it may take
/// to connect and to finish, so that a stalled server cannot hang the update.
fn curl_args<'a>(options: &[&'a str], url: &'a str) -> Vec<&'a str> {
    let mut args = vec!["--connect-timeout", "10", "--max-time", "300"];
    args.extend_from_slice(options);
    args.push(url);
    args
}

/// Runs `curl <options> <url>` and returns its stdout.
fn curl(options: &[&str], url: &str) -> anyhow::Result<Vec<u8>> {
    let out = match Command::new("curl").args(curl_args(options, url)).output() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => bail!("curl is needed to update cst"),
        result => result.context("cannot run curl")?,
    };
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        bail!("cannot download {url}: {}", why.trim());
    }
    Ok(out.stdout)
}

/// Puts `bytes` at `exe` atomically: written and synced beside it, then renamed over it.
///
/// # Errors
///
/// Any I/O error; the temp file is removed and `exe` left as it was.
fn replace(exe: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = exe.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".cst-update-{}", std::process::id()));
    // A crashed update by a process of the same id may have left it
    let _ = fs::remove_file(&tmp);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&tmp)?;
        file.write_all(bytes)?;
        // The umask may have taken bits off the mode asked for
        file.set_permissions(fs::Permissions::from_mode(0o755))?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, exe)
    })();
    match result {
        Ok(()) => {
            // The rename is durable once the directory is synced; it is done either way
            let _ = File::open(dir).and_then(|d| d.sync_all());
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use crate::update::{
        asset_name, checksum_for, compare_release, curl_args, is_current, replace, sha256_hex,
        tag_from_url,
    };

    #[test]
    fn curl_gets_time_limits_and_keeps_its_options() {
        let args = curl_args(&["-fsSL"], "https://example.test/x");
        assert_eq!(
            args,
            [
                "--connect-timeout",
                "10",
                "--max-time",
                "300",
                "-fsSL",
                "https://example.test/x"
            ]
        );
    }

    #[test]
    fn a_newer_release_is_greater() {
        for (tag, version) in [
            ("v2.0.1", "2.0.0"),
            ("2.1.0", "2.0.9"),
            ("v1.10.0", "1.9.0"),
        ] {
            assert_eq!(compare_release(tag, version), Ordering::Greater, "{tag}");
        }
    }

    #[test]
    fn the_same_release_is_equal_with_or_without_v() {
        assert_eq!(compare_release("v2.0.0", "2.0.0"), Ordering::Equal);
        assert_eq!(compare_release("2.0.0", "2.0.0"), Ordering::Equal);
    }

    #[test]
    fn an_older_release_is_less() {
        for (tag, version) in [
            ("v1.9.9", "2.0.0"),
            ("v2.0.0", "2.0.1"),
            ("v1.9.0", "1.10.0"),
        ] {
            assert_eq!(compare_release(tag, version), Ordering::Less, "{tag}");
        }
    }

    #[test]
    fn a_tag_that_is_not_a_version_falls_back_to_equality() {
        assert_eq!(compare_release("nightly", "2.0.0"), Ordering::Greater);
        assert_eq!(compare_release("v2.0", "2.0.0"), Ordering::Greater);
        assert_eq!(compare_release("v2.0.0-rc1", "2.0.0"), Ordering::Greater);
        // Equal text is the same release however it parses
        assert_eq!(compare_release("v2.0.0-rc1", "2.0.0-rc1"), Ordering::Equal);
    }

    #[test]
    fn the_tag_is_taken_from_the_release_url() {
        let url = "https://github.com/o/r/releases/tag/v2.1.0\n";
        assert_eq!(tag_from_url(url), Some("v2.1.0"));
    }

    #[test]
    fn a_url_without_a_tag_has_none() {
        assert_eq!(tag_from_url("https://github.com/o/r/releases"), None);
        assert_eq!(tag_from_url("https://github.com/o/r/releases/tag/"), None);
        assert_eq!(tag_from_url(""), None);
    }

    #[test]
    fn the_current_version_matches_with_or_without_v() {
        assert!(is_current("v2.0.0", "2.0.0"));
        assert!(is_current("2.0.0", "2.0.0"));
        assert!(!is_current("v2.0.1", "2.0.0"));
        assert!(!is_current("vv2.0.0", "2.0.0"));
    }

    #[test]
    fn the_checksum_is_read_in_sha256sum_format() {
        let hex = "a".repeat(64);
        let name = "cst-x86_64-unknown-linux-musl";
        assert_eq!(
            checksum_for(&format!("{hex}  {name}\n"), name),
            Some(hex.as_str())
        );
        assert_eq!(
            checksum_for(&format!("{hex} *{name}\n"), name),
            Some(hex.as_str())
        );
    }

    #[test]
    fn a_checksum_for_another_file_or_malformed_is_none() {
        let hex = "a".repeat(64);
        assert_eq!(checksum_for(&format!("{hex}  other\n"), "cst"), None);
        assert_eq!(checksum_for("abc  cst\n", "cst"), None);
        assert_eq!(
            checksum_for(&format!("{}  cst\n", "z".repeat(64)), "cst"),
            None
        );
        assert_eq!(checksum_for("", "cst"), None);
    }

    #[test]
    fn sha256_hex_matches_a_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_asset_names_this_machine() {
        let name = asset_name();
        assert!(name.starts_with("cst-") && name.ends_with("-unknown-linux-musl"));
        assert!(name.contains(std::env::consts::ARCH));
    }

    #[test]
    fn replace_writes_an_executable_and_leaves_no_temp() {
        let dir = TempDir::new().expect("temp dir");
        let exe = dir.path().join("cst");
        fs::write(&exe, b"old").expect("write");
        replace(&exe, b"new").expect("replace");
        assert_eq!(fs::read(&exe).expect("read"), b"new");
        let mode = fs::metadata(&exe).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        assert_eq!(fs::read_dir(dir.path()).expect("dir").count(), 1);
    }

    #[test]
    fn replace_in_a_missing_dir_fails_and_changes_nothing() {
        let dir = TempDir::new().expect("temp dir");
        let exe = dir.path().join("gone/cst");
        assert!(replace(&exe, b"new").is_err());
        assert_eq!(fs::read_dir(dir.path()).expect("dir").count(), 0);
    }
}
