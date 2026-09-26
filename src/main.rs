mod app;
mod dir;
mod fuzzy;
mod git;
mod ops;
mod preview;
mod ui;

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};

use app::{App, Effect};

const USAGE: &str = "\
findr - a terminal file browser for developers

usage: findr [PATH] [--cwd-file FILE]

  PATH             directory to open, or a file to open with the cursor on it
  --cwd-file FILE  on quitting with q, write the final directory to FILE
  -h, --help       show this help
  -V, --version    show the version

Press ? inside findr for keys.";

#[derive(Debug, PartialEq)]
enum Cli {
    Run {
        start: PathBuf,
        cwd_file: Option<PathBuf>,
    },
    Help,
    Version,
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Cli, String> {
    let mut start = None;
    let mut cwd_file = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-h" | "--help") => return Ok(Cli::Help),
            Some("-V" | "--version") => return Ok(Cli::Version),
            Some("--cwd-file") => {
                cwd_file = Some(args.next().ok_or("--cwd-file needs a path")?.into())
            }
            Some(s) if s.starts_with("--cwd-file=") => {
                cwd_file = Some(PathBuf::from(&s["--cwd-file=".len()..]));
            }
            Some(s) if s.starts_with('-') => return Err(format!("unknown option {s}")),
            _ if start.is_none() => start = Some(PathBuf::from(arg)),
            _ => return Err("only one PATH may be given".into()),
        }
    }
    Ok(Cli::Run {
        start: start.unwrap_or_else(|| PathBuf::from(".")),
        cwd_file,
    })
}

fn main() -> ExitCode {
    let (start, cwd_file) = match parse(std::env::args_os().skip(1)) {
        Ok(Cli::Run { start, cwd_file }) => (start, cwd_file),
        Ok(Cli::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Cli::Version) => {
            println!("findr {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("findr: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let mut app = match App::new(&start, ops::Trash::detect()) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("findr: {}: {e}", start.display());
            return ExitCode::FAILURE;
        }
    };

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();

    let write_cwd = match result {
        Ok(write_cwd) => write_cwd,
        Err(e) => {
            eprintln!("findr: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let (true, Some(file)) = (write_cwd, cwd_file)
        && let Err(e) = std::fs::write(&file, app.cwd.as_os_str().as_encoded_bytes())
    {
        eprintln!("findr: {}: {e}", file.display());
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// The event loop. Returns whether the final directory should be written for the shell.
fn run(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<bool> {
    let mut dirty = true;
    loop {
        app.sync_preview();
        if dirty {
            terminal.draw(|frame| ui::draw(frame, app))?;
            dirty = false;
        }
        // Poll quickly only while a worker owes us an answer.
        let timeout = if app.pending() { 15 } else { 250 };
        if event::poll(Duration::from_millis(timeout))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    match app.handle_key(key) {
                        Some(Effect::Quit { write_cwd }) => return Ok(write_cwd),
                        Some(Effect::Run(cmd)) => suspend(terminal, cmd, app)?,
                        None => {}
                    }
                    dirty = true;
                }
                Event::Resize(..) => dirty = true,
                _ => {}
            }
        } else {
            dirty |= app.tick();
        }
        dirty |= app.poll_preview() | app.poll_git();
    }
}

/// Hands the terminal to `cmd` (an editor or shell) and takes it back when it exits.
fn suspend(terminal: &mut DefaultTerminal, mut cmd: Command, app: &mut App) -> io::Result<()> {
    ratatui::restore();
    let status = cmd.status();
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    terminal.clear()?;
    app.after_run(status);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Cli, String> {
        parse(list.iter().map(OsString::from))
    }

    #[test]
    fn parses_arguments() {
        assert_eq!(
            args(&[]),
            Ok(Cli::Run {
                start: ".".into(),
                cwd_file: None
            })
        );
        assert_eq!(
            args(&["src", "--cwd-file", "/tmp/x"]),
            Ok(Cli::Run {
                start: "src".into(),
                cwd_file: Some("/tmp/x".into())
            })
        );
        assert_eq!(
            args(&["--cwd-file=/tmp/y"]),
            Ok(Cli::Run {
                start: ".".into(),
                cwd_file: Some("/tmp/y".into())
            })
        );
        assert_eq!(args(&["-h"]), Ok(Cli::Help));
        assert_eq!(args(&["--version"]), Ok(Cli::Version));
        assert!(args(&["--cwd-file"]).is_err());
        assert!(args(&["--bogus"]).is_err());
        assert!(args(&["a", "b"]).is_err());
    }
}
