//! The `cst` binary: dispatches the command line to the library.

use std::collections::HashMap;
use std::io::Read;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use cst::claude::{hook, install, statusline};
use cst::paths;
use cst::term::tmux::Cli;
use cst::ui::{landing_app, panel_app, sidebar_app};

const USAGE: &str = "\
Usage: cst [OPTIONS] [COMMAND]

Commands:
  update      Install the latest release in place and register it with Claude Code
  uninstall   Unregister cst from Claude Code, restoring your own status line

Options:
  --ghostty   Open in a new Ghostty window
  --version   Print the version
  --help      Print this help
";

/// Applies the hook event on stdin; logs a failure and exits 0, so no event breaks a session.
fn run_hook() -> ExitCode {
    let mut input = Vec::new();
    let read = std::io::stdin().lock().read_to_end(&mut input);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    // `vars()` panics on non-UTF-8 values, so convert lossily instead.
    let env: HashMap<String, String> = std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    let result = match read {
        Ok(_) => hook::run(&input, &env, now, &Cli::default()),
        Err(e) => Err(anyhow::Error::new(e).context("cannot read stdin")),
    };
    if let Err(e) = result {
        paths::log_error(&format_args!("hook: {e:#}"));
    }
    ExitCode::SUCCESS
}

/// Records the usage on stdin and prints the status line; logs a failure and exits 0, so no
/// update leaves Claude Code without a line.
fn run_statusline() -> ExitCode {
    let mut input = Vec::new();
    if let Err(e) = std::io::stdin().lock().read_to_end(&mut input) {
        paths::log_error(&format_args!("statusline: cannot read stdin: {e}"));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    statusline::run(&input, now, &mut std::io::stdout().lock());
    ExitCode::SUCCESS
}

/// Registers this binary with Claude Code; `install.sh` and `cst update` run it.
fn run_install() -> ExitCode {
    // The hooks must keep working whatever the current directory or the name it was run by
    match std::env::current_exe().and_then(std::fs::canonicalize) {
        Ok(exe) => exit_code(install::install(&exe)),
        Err(e) => {
            eprintln!("error: cannot find the cst executable: {e}");
            ExitCode::from(1)
        }
    }
}

/// Runs one of the screens tmux opens, `cst <command>`; a failure goes to the error log and
/// stderr, and exits 1.
fn run_screen(command: &str, screen: fn() -> anyhow::Result<()>) -> ExitCode {
    match screen() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            paths::log_error(&format_args!("{command}: {e:#}"));
            eprintln!("cst {command}: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn main() -> ExitCode {
    // `args()` panics on non-UTF-8 input, so convert lossily instead.
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    // The hook and the status line run on every Claude Code update: nothing but the subcommand
    // name is parsed.
    if args.first().map(String::as_str) == Some("hook") {
        return run_hook();
    }
    if args.first().map(String::as_str) == Some("statusline") {
        return run_statusline();
    }
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        // Hidden: users get it through `install.sh` and `cst update`
        ["install"] => run_install(),
        ["uninstall"] => exit_code(install::uninstall()),
        // Hidden: tmux runs them, in each session window's side pane, in the first window and
        // in the Alt+n popup
        ["sidebar"] => run_screen("sidebar", sidebar_app::main_sidebar),
        ["landing"] => run_screen("landing", landing_app::main_landing),
        ["new", "--pick"] => run_screen("new", panel_app::main_panel),
        ["--version"] => {
            println!("cst {}", cst::VERSION);
            ExitCode::SUCCESS
        }
        ["--help" | "-h"] => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        [other, ..] => {
            eprintln!("cst: unknown command '{other}'\n\n{USAGE}");
            ExitCode::from(2)
        }
        [] => {
            eprintln!("cst: opening the session window is not implemented yet");
            ExitCode::from(1)
        }
    }
}
