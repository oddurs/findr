mod app;
mod config;
mod dir;
mod find;
mod fuzzy;
mod git;
mod icons;
mod ops;
mod preview;
mod ui;

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};

use app::{App, Effect};
use ui::glyphs::{self, Glyphs, Tier};
use ui::theme::Theme;

const USAGE: &str = "\
findr - a terminal file browser for developers

usage: findr [PATH] [--cwd-file FILE] [--no-mouse] [--no-icons] [--light]

  PATH             directory to open, or a file to open with the cursor on it
  --cwd-file FILE  on quitting with q, write the final directory to FILE
  --no-mouse       leave the mouse to the terminal, for selecting text
  --no-icons       no file icons, for a terminal without a Nerd Font
  --light          colours for a light terminal background
  -h, --help       show this help
  -V, --version    show the version

Press ? inside findr for keys.";

#[derive(Debug, PartialEq)]
enum Cli {
    Run(Options),
    Help,
    Version,
}

#[derive(Debug, PartialEq)]
struct Options {
    start: PathBuf,
    cwd_file: Option<PathBuf>,
    mouse: bool,
    icons: bool,
    light: bool,
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Cli, String> {
    let mut start = None;
    let mut options = Options {
        start: PathBuf::from("."),
        cwd_file: None,
        mouse: true,
        icons: true,
        light: false,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-h" | "--help") => return Ok(Cli::Help),
            Some("-V" | "--version") => return Ok(Cli::Version),
            Some("--no-mouse") => options.mouse = false,
            Some("--no-icons") => options.icons = false,
            Some("--light") => options.light = true,
            Some("--cwd-file") => {
                options.cwd_file = Some(args.next().ok_or("--cwd-file needs a path")?.into())
            }
            Some(s) if s.starts_with("--cwd-file=") => {
                options.cwd_file = Some(PathBuf::from(&s["--cwd-file=".len()..]));
            }
            Some(s) if s.starts_with('-') => return Err(format!("unknown option {s}")),
            _ if start.is_none() => start = Some(PathBuf::from(arg)),
            _ => return Err("only one PATH may be given".into()),
        }
    }
    if let Some(start) = start {
        options.start = start;
    }
    Ok(Cli::Run(options))
}

fn main() -> ExitCode {
    let options = match parse(std::env::args_os().skip(1)) {
        Ok(Cli::Run(options)) => options,
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
    let Options {
        start,
        cwd_file,
        mouse,
        icons,
        light,
    } = options;
    let config = match config::load() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("findr: {e}");
            return ExitCode::from(2);
        }
    };
    // Flags override the config file.
    let mouse = mouse && config.mouse;
    let base = if light || config.light {
        Theme::LIGHT
    } else {
        Theme::DARK
    };
    let tier = config.glyphs.unwrap_or_else(glyphs::detect);
    // --no-icons means no Nerd Font glyphs; it does not raise ASCII to Unicode.
    let tier = if !icons && tier == Tier::Nerd {
        Tier::Unicode
    } else {
        tier
    };
    let theme = Theme {
        glyphs: Glyphs::for_tier(tier),
        ..base
    };
    let mut app = match App::new(&start, ops::Trash::detect()) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("findr: {}: {e}", start.display());
            return ExitCode::FAILURE;
        }
    };

    let mut terminal = ratatui::init();
    if mouse {
        // ratatui's panic hook restores the screen but knows nothing of mouse capture, and a
        // terminal left reporting mouse movement fills the shell with escape codes.
        let restore = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            release_mouse();
            restore(info);
        }));
        capture_mouse();
    }
    let result = run(&mut terminal, &mut app, mouse, &theme);
    if mouse {
        release_mouse();
    }
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
fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    mouse: bool,
    theme: &Theme,
) -> io::Result<bool> {
    let mut dirty = true;
    loop {
        app.sync_preview();
        if dirty {
            terminal.draw(|frame| ui::draw(frame, app, theme))?;
            dirty = false;
        }
        // Poll quickly only while a worker owes us an answer.
        let timeout = if app.pending() { 15 } else { 250 };
        if event::poll(Duration::from_millis(timeout))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    match app.handle_key(key) {
                        Some(Effect::Quit { write_cwd }) => return Ok(write_cwd),
                        Some(Effect::Run(cmd)) => suspend(terminal, cmd, app, mouse)?,
                        None => {}
                    }
                    dirty = true;
                }
                Event::Mouse(event) => {
                    match app.handle_mouse(event) {
                        Some(Effect::Quit { write_cwd }) => return Ok(write_cwd),
                        Some(Effect::Run(cmd)) => suspend(terminal, cmd, app, mouse)?,
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
        dirty |= app.poll_preview() | app.poll_git() | app.poll_job() | app.poll_find();
    }
}

// Mouse capture is a nicety: if the terminal refuses it, findr still works from the keyboard,
// so failures here are not worth stopping for.
fn capture_mouse() {
    let _ = execute!(io::stdout(), EnableMouseCapture);
}

fn release_mouse() {
    let _ = execute!(io::stdout(), DisableMouseCapture);
}

/// Hands the terminal to `cmd` (an editor or shell) and takes it back when it exits.
fn suspend(
    terminal: &mut DefaultTerminal,
    mut cmd: Command,
    app: &mut App,
    mouse: bool,
) -> io::Result<()> {
    if mouse {
        release_mouse();
    }
    ratatui::restore();
    let status = cmd.status();
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    if mouse {
        capture_mouse();
    }
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
        let run = |start: &str, cwd_file: Option<&str>, mouse, icons| {
            Ok(Cli::Run(Options {
                start: start.into(),
                cwd_file: cwd_file.map(PathBuf::from),
                mouse,
                icons,
                light: false,
            }))
        };
        assert_eq!(args(&[]), run(".", None, true, true));
        assert_eq!(
            args(&["src", "--cwd-file", "/tmp/x"]),
            run("src", Some("/tmp/x"), true, true)
        );
        assert_eq!(
            args(&["--cwd-file=/tmp/y"]),
            run(".", Some("/tmp/y"), true, true)
        );
        assert_eq!(args(&["--no-mouse", "src"]), run("src", None, false, true));
        assert_eq!(args(&["--no-icons"]), run(".", None, true, false));
        let Ok(Cli::Run(light)) = args(&["--light"]) else {
            panic!("--light should parse");
        };
        assert!(light.light);
        assert_eq!(args(&["-h"]), Ok(Cli::Help));
        assert_eq!(args(&["--version"]), Ok(Cli::Version));
        assert!(args(&["--cwd-file"]).is_err());
        assert!(args(&["--bogus"]).is_err());
        assert!(args(&["a", "b"]).is_err());
    }
}
