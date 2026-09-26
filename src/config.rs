//! Settings from `config.toml` in `$XDG_CONFIG_HOME/findr/` (usually `~/.config/findr/`).
//! Command-line flags override them. The format is the flat subset of TOML findr needs —
//! `key = value` lines, `#` comments — so it takes no parser dependency.

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::ui::glyphs::Tier;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// `None` means detect.
    pub glyphs: Option<Tier>,
    pub mouse: bool,
    pub light: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            glyphs: None,
            mouse: true,
            light: false,
        }
    }
}

pub fn path() -> Option<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("findr").join("config.toml"))
}

/// The settings, or the defaults when there is no config file.
pub fn load() -> Result<Config, String> {
    let Some(path) = path() else {
        return Ok(Config::default());
    };
    match fs::read_to_string(&path) {
        Ok(text) => parse(&text).map_err(|e| format!("{}:{e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn parse(text: &str) -> Result<Config, String> {
    let mut config = Config::default();
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("{n}: expected `key = value`, found `{line}`"));
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "glyphs" => {
                config.glyphs = match string(n, value)?.as_str() {
                    "auto" => None,
                    "nerd" => Some(Tier::Nerd),
                    "unicode" => Some(Tier::Unicode),
                    "ascii" => Some(Tier::Ascii),
                    other => {
                        return Err(format!(
                            "{n}: glyphs is \"auto\", \"nerd\", \"unicode\" or \"ascii\", not \"{other}\""
                        ));
                    }
                }
            }
            "theme" => {
                config.light = match string(n, value)?.as_str() {
                    "dark" => false,
                    "light" => true,
                    other => {
                        return Err(format!(
                            "{n}: theme is \"dark\" or \"light\", not \"{other}\""
                        ));
                    }
                }
            }
            "mouse" => config.mouse = boolean(n, value)?,
            other => return Err(format!("{n}: unknown setting `{other}`")),
        }
    }
    Ok(config)
}

/// Drops a `#` comment, leaving any `#` inside a quoted string alone.
fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

fn string(n: usize, value: &str) -> Result<String, String> {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .map(str::to_string)
        .ok_or_else(|| format!("{n}: expected a quoted string, found `{value}`"))
}

fn boolean(n: usize, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("{n}: expected true or false, found `{value}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
        assert_eq!(parse("# just a comment\n\n").unwrap(), Config::default());
    }

    #[test]
    fn reads_every_setting() {
        let config = parse(
            "glyphs = \"ascii\"   # for a serial console\ntheme = \"light\"\nmouse = false\n",
        )
        .unwrap();
        assert_eq!(
            config,
            Config {
                glyphs: Some(Tier::Ascii),
                mouse: false,
                light: true,
            }
        );
        assert_eq!(parse("glyphs = \"auto\"").unwrap().glyphs, None);
    }

    #[test]
    fn mistakes_name_the_line() {
        assert_eq!(
            parse("\nmose = true").unwrap_err(),
            "2: unknown setting `mose`"
        );
        assert!(parse("mouse = yes").unwrap_err().contains("true or false"));
        assert!(parse("glyphs = nerd").unwrap_err().contains("quoted"));
        assert!(
            parse("glyphs = \"emoji\"")
                .unwrap_err()
                .contains("\"emoji\"")
        );
        assert!(parse("mouse").unwrap_err().contains("key = value"));
    }
}
