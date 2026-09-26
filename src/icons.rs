//! Nerd Font icons for files and directories: by exact name first, then by extension.
//!
//! Codepoints are Nerd Fonts v3. Colours follow the conventions most editors share, so a Rust
//! file looks the same here as in the sidebar you are used to.

use ratatui::style::Color;

use crate::dir::Entry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Executable,
    Symlink,
    SymlinkDir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Icon {
    pub glyph: char,
    pub color: Color,
}

const fn icon(glyph: char, rgb: u32) -> Icon {
    Icon {
        glyph,
        color: Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8),
    }
}

// Directories take the terminal's blue, like their names, rather than a fixed colour.
const DIR: Icon = Icon {
    glyph: '\u{f07b}',
    color: Color::Blue,
};
const DIR_GIT: Icon = icon('\u{e5fb}', 0xf14c28);
const DIR_NPM: Icon = icon('\u{e5fa}', 0xe8274b);
const DIR_GITHUB: Icon = icon('\u{e5fd}', 0xcccccc);
const DIR_CONFIG: Icon = icon('\u{e5fc}', 0x6d8086);
const FILE: Icon = icon('\u{f15b}', 0x9aa5ce);
const EXECUTABLE: Icon = icon('\u{f489}', 0x9ece6a);
const SYMLINK: Icon = icon('\u{f481}', 0x7dcfff);
const SYMLINK_DIR: Icon = icon('\u{f482}', 0x7dcfff);

const RUST: Icon = icon('\u{e7a8}', 0xdea584);
const TOML: Icon = icon('\u{e6b2}', 0x9c4221);
const MARKDOWN: Icon = icon('\u{f48a}', 0xdddddd);
const JSON: Icon = icon('\u{f0626}', 0xcbcb41);
const YAML: Icon = icon('\u{e6a8}', 0x6d8086);
const JAVASCRIPT: Icon = icon('\u{e60c}', 0xcbcb41);
const TYPESCRIPT: Icon = icon('\u{e628}', 0x519aba);
const REACT: Icon = icon('\u{e7ba}', 0x20c2e3);
const PYTHON: Icon = icon('\u{e606}', 0xffbc03);
const GO: Icon = icon('\u{e627}', 0x00add8);
const C: Icon = icon('\u{e61e}', 0x599eff);
const CPP: Icon = icon('\u{e61d}', 0xf34b7d);
const HEADER: Icon = icon('\u{e61e}', 0xa074c4);
const JAVA: Icon = icon('\u{e738}', 0xcc3e44);
const KOTLIN: Icon = icon('\u{e634}', 0x7f52ff);
const SWIFT: Icon = icon('\u{e755}', 0xe37933);
const RUBY: Icon = icon('\u{e791}', 0x701516);
const PHP: Icon = icon('\u{e73d}', 0xa074c4);
const CSHARP: Icon = icon('\u{f031b}', 0x596706);
const SHELL: Icon = icon('\u{e795}', 0x89e051);
const HTML: Icon = icon('\u{e736}', 0xe44d26);
const CSS: Icon = icon('\u{e749}', 0x42a5f5);
const LUA: Icon = icon('\u{e620}', 0x51a0cf);
const VIM: Icon = icon('\u{e62b}', 0x019833);
const HASKELL: Icon = icon('\u{e777}', 0xa074c4);
const ELIXIR: Icon = icon('\u{e62d}', 0xa074c4);
const ZIG: Icon = icon('\u{e6a9}', 0xf69a1b);
const NIX: Icon = icon('\u{f313}', 0x7ebae4);
const SVELTE: Icon = icon('\u{e697}', 0xff3e00);
const VUE: Icon = icon('\u{e6a0}', 0x8dc149);
const SQL: Icon = icon('\u{e706}', 0xdad8d8);
const CONFIG: Icon = icon('\u{e615}', 0x6d8086);
const ENV: Icon = icon('\u{f462}', 0xfaf743);
const LOCK: Icon = icon('\u{f023}', 0xbbbbbb);
const TEXT: Icon = icon('\u{f0219}', 0x89e051);
const CSV: Icon = icon('\u{e64a}', 0x89e051);
const DIFF: Icon = icon('\u{f440}', 0x41535b);
const IMAGE: Icon = icon('\u{f1c5}', 0xa074c4);
const SVG: Icon = icon('\u{e698}', 0xffb13b);
const VIDEO: Icon = icon('\u{e69f}', 0xfd971f);
const AUDIO: Icon = icon('\u{f001}', 0x66d8ef);
const FONT: Icon = icon('\u{e659}', 0xececec);
const PDF: Icon = icon('\u{f1c1}', 0xb30b00);
const ARCHIVE: Icon = icon('\u{f410}', 0xeca517);
const BINARY: Icon = icon('\u{eae8}', 0x9f0500);
const KEY: Icon = icon('\u{f084}', 0xe3c58e);
const GIT: Icon = icon('\u{e702}', 0xf14c28);
const DOCKER: Icon = icon('\u{f308}', 0x458ee6);
const MAKE: Icon = icon('\u{e673}', 0x6d8086);
const LICENSE: Icon = icon('\u{e60a}', 0xd0bf41);
const NPM: Icon = icon('\u{e71e}', 0xe8274b);

pub fn for_entry(entry: &Entry) -> Icon {
    let kind = match (entry.is_symlink, entry.is_dir) {
        (true, true) => Kind::SymlinkDir,
        (true, false) => Kind::Symlink,
        (false, true) => Kind::Dir,
        (false, false) if entry.is_executable() => Kind::Executable,
        (false, false) => Kind::File,
    };
    for_name(&entry.name, kind)
}

pub fn for_name(name: &str, kind: Kind) -> Icon {
    let lower = name.to_lowercase();
    match kind {
        Kind::SymlinkDir => return SYMLINK_DIR,
        Kind::Symlink => return SYMLINK,
        Kind::Dir => return dir(&lower),
        Kind::File | Kind::Executable => {}
    }
    if let Some(icon) = by_name(&lower) {
        return icon;
    }
    // An executable with a known extension (build.sh, manage.py) is better told by its type.
    let ext = lower
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, ext)| ext);
    match (ext.and_then(by_extension), kind) {
        (Some(icon), _) => icon,
        (None, Kind::Executable) => EXECUTABLE,
        (None, _) => FILE,
    }
}

fn dir(name: &str) -> Icon {
    match name {
        ".git" => DIR_GIT,
        "node_modules" => DIR_NPM,
        ".github" => DIR_GITHUB,
        ".config" => DIR_CONFIG,
        _ => DIR,
    }
}

fn by_name(name: &str) -> Option<Icon> {
    Some(match name {
        "cargo.toml" | "cargo.lock" | "rust-toolchain" | "rust-toolchain.toml" => RUST,
        "package.json" | "package-lock.json" | ".npmrc" => NPM,
        "dockerfile"
        | "containerfile"
        | ".dockerignore"
        | "docker-compose.yml"
        | "docker-compose.yaml"
        | "compose.yml"
        | "compose.yaml" => DOCKER,
        "makefile" | "gnumakefile" | "justfile" => MAKE,
        "license" | "license.md" | "license.txt" | "licence" | "copying" => LICENSE,
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" | ".mailmap" => GIT,
        "go.mod" | "go.sum" => GO,
        ".editorconfig" => CONFIG,
        ".env" => ENV,
        _ if name.starts_with(".env.") => ENV,
        _ => return None,
    })
}

fn by_extension(ext: &str) -> Option<Icon> {
    Some(match ext {
        "rs" => RUST,
        "toml" => TOML,
        "md" | "markdown" | "mdx" => MARKDOWN,
        "json" | "jsonc" | "json5" => JSON,
        "yml" | "yaml" => YAML,
        "js" | "mjs" | "cjs" => JAVASCRIPT,
        "ts" | "mts" | "cts" => TYPESCRIPT,
        "jsx" | "tsx" => REACT,
        "py" | "pyi" => PYTHON,
        "go" => GO,
        "c" => C,
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => CPP,
        "h" => HEADER,
        "java" => JAVA,
        "kt" | "kts" => KOTLIN,
        "swift" => SWIFT,
        "rb" => RUBY,
        "php" => PHP,
        "cs" => CSHARP,
        "sh" | "bash" | "zsh" | "fish" => SHELL,
        "html" | "htm" => HTML,
        "css" | "scss" | "sass" | "less" => CSS,
        "lua" => LUA,
        "vim" => VIM,
        "hs" => HASKELL,
        "ex" | "exs" => ELIXIR,
        "zig" => ZIG,
        "nix" => NIX,
        "svelte" => SVELTE,
        "vue" => VUE,
        "sql" => SQL,
        "ini" | "cfg" | "conf" => CONFIG,
        "lock" => LOCK,
        "txt" | "log" => TEXT,
        "csv" | "tsv" => CSV,
        "diff" | "patch" => DIFF,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "heic" => IMAGE,
        "svg" => SVG,
        "mp4" | "mov" | "mkv" | "webm" | "avi" => VIDEO,
        "mp3" | "wav" | "flac" | "ogg" | "m4a" => AUDIO,
        "ttf" | "otf" | "woff" | "woff2" => FONT,
        "pdf" => PDF,
        "zip" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "7z" | "rar" => ARCHIVE,
        "exe" | "dll" | "so" | "dylib" | "o" | "a" | "wasm" | "bin" => BINARY,
        "pem" | "key" | "crt" | "pub" | "gpg" | "asc" => KEY,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(name: &str, kind: Kind) -> char {
        for_name(name, kind).glyph
    }

    #[test]
    fn names_win_over_extensions() {
        assert_eq!(glyph("main.rs", Kind::File), RUST.glyph);
        assert_eq!(
            glyph("Cargo.lock", Kind::File),
            RUST.glyph,
            "not the generic lock icon"
        );
        assert_eq!(glyph("yarn.lock", Kind::File), LOCK.glyph);
        assert_eq!(glyph("Dockerfile", Kind::File), DOCKER.glyph);
        assert_eq!(glyph(".env.local", Kind::File), ENV.glyph);
    }

    #[test]
    fn matching_ignores_case() {
        assert_eq!(glyph("README.MD", Kind::File), MARKDOWN.glyph);
        assert_eq!(glyph("Photo.JPG", Kind::File), IMAGE.glyph);
    }

    #[test]
    fn directories_have_their_own_set() {
        assert_eq!(glyph(".git", Kind::Dir), DIR_GIT.glyph);
        assert_eq!(glyph("node_modules", Kind::Dir), DIR_NPM.glyph);
        assert_eq!(
            glyph("src.rs", Kind::Dir),
            DIR.glyph,
            "a directory's extension means nothing"
        );
        assert_eq!(glyph("linked", Kind::SymlinkDir), SYMLINK_DIR.glyph);
    }

    #[test]
    fn falls_back_by_kind() {
        assert_eq!(glyph("notes.qqq", Kind::File), FILE.glyph);
        assert_eq!(
            glyph(".bashrc", Kind::File),
            FILE.glyph,
            "a dotfile has no extension"
        );
        assert_eq!(glyph("run", Kind::Executable), EXECUTABLE.glyph);
        assert_eq!(glyph("build.sh", Kind::Executable), SHELL.glyph);
        assert_eq!(glyph("main.rs", Kind::Symlink), SYMLINK.glyph);
    }
}
