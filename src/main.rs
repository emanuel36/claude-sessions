//! The `cst` binary: dispatches the command line to the library.

use std::process::ExitCode;

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
