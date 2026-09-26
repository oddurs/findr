//! Every symbol the screen draws, in three tiers, so it degrades instead of breaking:
//! Nerd Font glyphs where a Nerd Font is installed, plain Unicode (checked against Menlo, the
//! macOS default) where it is not, and ASCII where the terminal is not UTF-8 at all.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use ratatui::symbols::border;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Nerd,
    Unicode,
    Ascii,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyphs {
    pub tier: Tier,
    /// The bar at the start of the cursor row.
    pub cursor: &'static str,
    /// The bar at the start of a marked row.
    pub mark: &'static str,
    pub thumb: &'static str,
    pub ellipsis: &'static str,
    /// Between facts: `Rust · 12 K`.
    pub separator: &'static str,
    /// After a prompt's label: `rename ›`.
    pub prompt: &'static str,
    pub success: &'static str,
    pub failure: &'static str,
    /// Dirty repository, marks.
    pub dot: &'static str,
    pub up: &'static str,
    pub down: &'static str,
    /// A symlink's target: `→ ../lib`.
    pub arrow: &'static str,
    pub minus: &'static str,
    /// Before the branch name, with its trailing space; empty where there is no good symbol.
    pub branch: &'static str,
    /// How the up and down keys are written in hints.
    pub up_down: &'static str,
    pub rule: &'static str,
    pub border: border::Set<'static>,
}

const ASCII_BORDER: border::Set<'static> = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

impl Glyphs {
    pub const UNICODE: Glyphs = Glyphs {
        tier: Tier::Unicode,
        cursor: "▌",
        mark: "▍",
        thumb: "▐",
        ellipsis: "…",
        separator: "·",
        prompt: "›",
        success: "✓",
        failure: "✗",
        dot: "●",
        up: "↑",
        down: "↓",
        arrow: "→",
        minus: "−",
        branch: "",
        up_down: "↑ ↓",
        rule: "│",
        border: border::ROUNDED,
    };

    pub const NERD: Glyphs = Glyphs {
        tier: Tier::Nerd,
        success: "\u{f00c}",
        failure: "\u{f00d}",
        branch: "\u{e725} ",
        ..Glyphs::UNICODE
    };

    pub const ASCII: Glyphs = Glyphs {
        tier: Tier::Ascii,
        cursor: ">",
        mark: "*",
        thumb: "#",
        ellipsis: "~",
        separator: "-",
        prompt: ">",
        success: "ok",
        failure: "!",
        dot: "*",
        up: "^",
        down: "v",
        arrow: "->",
        minus: "-",
        branch: "",
        up_down: "up down",
        rule: "|",
        border: ASCII_BORDER,
    };

    /// File-type icons need a Nerd Font; the other tiers go without.
    pub fn icons(&self) -> bool {
        self.tier == Tier::Nerd
    }

    pub fn for_tier(tier: Tier) -> Glyphs {
        match tier {
            Tier::Nerd => Glyphs::NERD,
            Tier::Unicode => Glyphs::UNICODE,
            Tier::Ascii => Glyphs::ASCII,
        }
    }
}

/// The tier to use when the user has not chosen one: ASCII outside a UTF-8 locale, Nerd when a
/// Nerd Font is installed, Unicode otherwise. Whether the terminal actually uses the installed
/// font cannot be asked, so `glyphs = "unicode"` in the config is the way out.
pub fn detect() -> Tier {
    if !utf8_locale(|name| env::var(name).ok()) {
        Tier::Ascii
    } else if nerd_font_installed(&font_dirs()) {
        Tier::Nerd
    } else {
        Tier::Unicode
    }
}

/// Whether the locale is UTF-8, by POSIX precedence (`LC_ALL`, `LC_CTYPE`, `LANG`). An unset
/// locale is taken as UTF-8, which is what every current terminal defaults to.
pub fn utf8_locale(var: impl Fn(&str) -> Option<String>) -> bool {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|name| var(name).filter(|v| !v.is_empty()));
    match locale {
        None => true,
        Some(locale) => {
            let locale = locale.to_lowercase();
            locale.contains("utf-8") || locale.contains("utf8")
        }
    }
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/Library/Fonts"),
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
    ];
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join("Library/Fonts"));
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".fonts"));
    }
    dirs
}

/// Looks for a Nerd Font by file name, two levels deep (Linux keeps fonts in subfolders).
fn nerd_font_installed(dirs: &[PathBuf]) -> bool {
    fn search(dir: &Path, depth: usize) -> bool {
        // A font directory that is missing or unreadable simply has no Nerd Font in it.
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.contains("nerdfont") || name.contains("nerd font") {
                return true;
            }
            depth > 0
                && entry.file_type().is_ok_and(|t| t.is_dir())
                && search(&entry.path(), depth - 1)
        })
    }
    dirs.iter().any(|dir| search(dir, 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dir::testutil::TempDir;

    #[test]
    fn locale_decides_utf8() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert!(utf8_locale(env(&[("LANG", "en_US.UTF-8")])));
        assert!(utf8_locale(env(&[("LANG", "C.utf8")])));
        assert!(!utf8_locale(env(&[("LANG", "C")])));
        assert!(
            !utf8_locale(env(&[("LC_ALL", "POSIX"), ("LANG", "en_US.UTF-8")])),
            "LC_ALL wins"
        );
        assert!(utf8_locale(env(&[])), "unset means UTF-8");
    }

    #[test]
    fn finds_a_nerd_font_by_name() {
        let tmp = TempDir::new();
        tmp.file("fonts/Menlo.ttc", "");
        assert!(!nerd_font_installed(&[tmp.path().join("fonts")]));
        tmp.file("fonts/jetbrains/JetBrainsMonoNerdFont-Regular.ttf", "");
        assert!(nerd_font_installed(&[tmp.path().join("fonts")]));
        assert!(!nerd_font_installed(&[tmp.path().join("missing")]));
    }

    #[test]
    fn ascii_is_ascii() {
        let g = Glyphs::ASCII;
        let all = [
            g.cursor,
            g.mark,
            g.thumb,
            g.ellipsis,
            g.separator,
            g.prompt,
            g.success,
            g.failure,
            g.dot,
            g.up,
            g.down,
            g.arrow,
            g.minus,
            g.branch,
            g.up_down,
            g.rule,
            g.border.top_left,
            g.border.horizontal_top,
            g.border.vertical_left,
        ];
        for s in all {
            assert!(s.is_ascii(), "{s:?} is not ASCII");
        }
        assert!(!g.icons());
    }
}
