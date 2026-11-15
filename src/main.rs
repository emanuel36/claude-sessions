//! The `cst` binary: dispatches the command line to the library.

use std::collections::HashMap;
use std::io::Read;
use std::process::ExitCode;

use cst::claude::{hook, install, statusline};
use cst::core::paths;
use cst::core::state::unix_now;
use cst::term::tmux::{self, Cli};
use cst::term::{launcher, layout};
use cst::ui::pane::own_socket;
use cst::ui::{landing_app, panel_app, sidebar_app};
use cst::update;

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

/// All of stdin.
fn read_stdin() -> std::io::Result<Vec<u8>> {
    let mut input = Vec::new();
    std::io::stdin().lock().read_to_end(&mut input)?;
    Ok(input)
}

/// Applies the hook event on stdin; logs a failure and exits 0, so no event breaks a session.
fn run_hook() -> ExitCode {
    let read = read_stdin();
    let now = unix_now();
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
        Ok(input) => hook::run(&input, &env, now, &Cli::default()),
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
    let input = read_stdin().unwrap_or_else(|e| {
        paths::log_error(&format_args!("statusline: cannot read stdin: {e}"));
        Vec::new()
    });
    statusline::run(&input, unix_now(), &mut std::io::stdout().lock());
    ExitCode::SUCCESS
}

/// `cst focus --pane P --socket S`, which tmux runs when a window comes on screen. Prints
/// nothing and exits 0 whatever happens: tmux shows a failed command's output over the pane.
fn run_focus(args: &[&str]) -> ExitCode {
    match args {
        ["--pane", pane, "--socket", socket] => layout::focus(pane, socket, unix_now()),
        _ => paths::log_error(&format_args!("focus: bad arguments {args:?}")),
    }
    ExitCode::SUCCESS
}

/// `cst step up|down WINDOW`, for Alt+Up/Down.
fn run_step(window: &str, offset: i32) -> ExitCode {
    let records = hook::live_sessions(unix_now());
    layout::step(window, offset, &records, &own_socket(), &Cli::default());
    ExitCode::SUCCESS
}

/// Registers this binary with Claude Code; `install.sh` and `cst update` run it. With
/// `check_only` it only checks that registering would work and changes nothing.
fn run_install(check_only: bool) -> ExitCode {
    // The hooks must keep working whatever the current directory or the name it was run by
    match std::env::current_exe().and_then(std::fs::canonicalize) {
        Ok(exe) if check_only => exit_code(install::check(&exe)),
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
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        // Hidden: Claude Code runs them on every update; any arguments after the name are
        // ignored
        ["hook", ..] => run_hook(),
        ["statusline", ..] => run_statusline(),
        // Hidden: users get it through `install.sh` and `cst update`
        ["install"] => run_install(false),
        ["install", "--check"] => run_install(true),
        ["uninstall"] => exit_code(install::uninstall()),
        ["update"] => exit_code(update::update()),
        // Hidden: tmux runs them, in each session window's side pane, in the first window and
        // in the Alt+n popup
        ["sidebar"] => run_screen("sidebar", sidebar_app::main_sidebar),
        ["landing"] => run_screen("landing", landing_app::main_landing),
        ["new", "--pick"] => run_screen("new", panel_app::main_panel),
        // Hidden: tmux.conf runs them on window changes, and for Alt+Up/Down and Alt+c
        ["focus", rest @ ..] => run_focus(rest),
        ["layout"] => {
            layout::layout(None, &Cli::default());
            ExitCode::SUCCESS
        }
        ["layout", window] => {
            layout::layout(Some(window), &Cli::default());
            ExitCode::SUCCESS
        }
        ["step", "up", window] => run_step(window, -1),
        ["step", "down", window] => run_step(window, 1),
        ["close", window] => {
            tmux::close_window(window, &Cli::default());
            ExitCode::SUCCESS
        }
        [] => exit_code(launcher::main(false)),
        ["--ghostty"] => exit_code(launcher::main(true)),
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
    }
}
