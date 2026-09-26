//! The preview column, built on a worker thread so highlighting never stalls the cursor.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;
use unicode_width::UnicodeWidthChar;

use crate::dir::{self, Entry, SortKey};
use crate::git;

const MAX_BYTES: u64 = 256 * 1024;
const MAX_LINES: usize = 500;
const MAX_DIR_ENTRIES: usize = 1000;
// Highlighting a minified bundle's single enormous line takes seconds; show such lines plain.
const MAX_HIGHLIGHT_LINE: usize = 2000;

pub enum Preview {
    Dir(Vec<Entry>),
    Text {
        lines: Vec<Line<'static>>,
        /// The grammar's name, such as "Rust" or "Plain Text".
        syntax: String,
        /// The file was longer than the preview reads.
        truncated: bool,
    },
    /// What changed in the file against HEAD.
    Diff {
        lines: Vec<DiffLine>,
        added: usize,
        removed: usize,
    },
    Note(String),
}

/// A diff line, classified here so the UI can colour it from the theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    Hunk(String),
    Added(String),
    Removed(String),
    Context(String),
}

/// What a preview is of: a path, and whether it shows the file's diff instead of its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub path: PathBuf,
    pub diff: bool,
}

pub struct Request {
    pub key: Key,
    pub show_hidden: bool,
}

pub struct Response {
    pub key: Key,
    pub preview: Preview,
}

pub fn spawn() -> (Sender<Request>, Receiver<Response>) {
    let (req_tx, req_rx) = mpsc::channel::<Request>();
    let (resp_tx, resp_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut highlighter = None;
        while let Ok(mut req) = req_rx.recv() {
            // Only the newest request matters; the cursor has already moved past the others.
            while let Ok(newer) = req_rx.try_recv() {
                req = newer;
            }
            let preview = build(&req, &mut highlighter);
            if resp_tx
                .send(Response {
                    key: req.key,
                    preview,
                })
                .is_err()
            {
                break;
            }
        }
    });
    (req_tx, resp_rx)
}

fn build(req: &Request, highlighter: &mut Option<Highlighter>) -> Preview {
    let path = &req.key.path;
    if req.key.diff {
        return diff(path).unwrap_or_else(|e| Preview::Note(format!("git diff: {e}")));
    }
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) => return Preview::Note(e.to_string()),
    };
    if meta.is_dir() {
        return match dir::list(path, req.show_hidden) {
            Ok(mut entries) => {
                dir::sort(&mut entries, SortKey::Name, false);
                entries.truncate(MAX_DIR_ENTRIES);
                Preview::Dir(entries)
            }
            Err(e) => Preview::Note(e.to_string()),
        };
    }
    if !meta.is_file() {
        return Preview::Note("special file".into());
    }
    if meta.len() == 0 {
        return Preview::Note("empty file".into());
    }
    let mut buf = Vec::new();
    if let Err(e) = File::open(path).and_then(|f| f.take(MAX_BYTES).read_to_end(&mut buf)) {
        return Preview::Note(e.to_string());
    }
    if buf[..buf.len().min(8000)].contains(&0) {
        return Preview::Note(format!("binary · {}", dir::human_size(meta.len())));
    }
    let text = String::from_utf8_lossy(&buf);
    let (lines, syntax) = highlighter
        .get_or_insert_with(Highlighter::new)
        .highlight(path, &text);
    let truncated = meta.len() > MAX_BYTES || lines.len() == MAX_LINES;
    Preview::Text {
        lines,
        syntax,
        truncated,
    }
}

/// The file's changes against HEAD, staged or not. In a repository with no commits yet there
/// is no HEAD, so it falls back to the unstaged changes.
fn diff(path: &Path) -> io::Result<Preview> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(io::Error::other("no file name"));
    };
    let run = |base: &[&str]| {
        let mut git = Command::new("git");
        for var in git::REPO_ENV {
            git.env_remove(var);
        }
        git.arg("-C")
            .arg(dir)
            .args(["diff", "--no-color", "--no-ext-diff"])
            .args(base)
            .arg("--")
            .arg(name)
            .stdin(Stdio::null())
            .output()
    };
    let mut out = run(&["HEAD"])?;
    if !out.status.success() {
        out = run(&[])?;
    }
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(io::Error::other(err.trim().to_string()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    if text.trim().is_empty() {
        return Ok(Preview::Note("no changes against HEAD".into()));
    }
    let (mut added, mut removed) = (0, 0);
    let mut lines = Vec::new();
    let mut in_hunks = false;
    for raw in text.lines() {
        let line = clean(raw, &mut 0);
        // Everything before the first hunk is the file header (diff --git, index, ---, +++),
        // naming what the inspector's title already names. Only its position identifies it: a
        // removed SQL comment is also a line starting "--- ".
        if !in_hunks {
            if !raw.starts_with("@@") {
                continue;
            }
            in_hunks = true;
        }
        let kind = if raw.starts_with("@@") {
            DiffLine::Hunk(line)
        } else if raw.starts_with('+') {
            added += 1;
            DiffLine::Added(line)
        } else if raw.starts_with('-') {
            removed += 1;
            DiffLine::Removed(line)
        } else {
            DiffLine::Context(line)
        };
        // Counted in full, shown up to the cap: the totals should not lie about a long diff.
        if lines.len() < MAX_LINES {
            lines.push(kind);
        }
    }
    Ok(Preview::Diff {
        lines,
        added,
        removed,
    })
}

struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

impl Highlighter {
    fn new() -> Highlighter {
        let mut themes = ThemeSet::load_defaults();
        Highlighter {
            syntaxes: two_face::syntax::extra_newlines(),
            theme: themes
                .themes
                .remove("base16-ocean.dark")
                .expect("syntect bundles base16-ocean.dark"),
        }
    }

    fn syntax_for(&self, path: &Path, first_line: &str) -> &SyntaxReference {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        // The few extensions bat's grammars do not claim, sent to their nearest relative.
        let ext = match ext {
            "mts" | "cts" => "ts",
            "jsonc" | "json5" => "json",
            other => other,
        };
        let s = &self.syntaxes;
        s.find_syntax_by_extension(ext)
            .or_else(|| s.find_syntax_by_extension(name))
            .or_else(|| s.find_syntax_by_first_line(first_line))
            .unwrap_or_else(|| s.find_syntax_plain_text())
    }

    /// The highlighted lines and the name of the grammar used.
    fn highlight(&self, path: &Path, text: &str) -> (Vec<Line<'static>>, String) {
        let lines: Vec<&str> = LinesWithEndings::from(text).take(MAX_LINES).collect();
        let syntax = self.syntax_for(path, lines.first().copied().unwrap_or(""));
        let name = syntax.name.clone();
        let mut state = HighlightLines::new(syntax, &self.theme);
        let width = lines.len().to_string().len();
        let gutter = Style::new().fg(Color::DarkGray);
        let highlighted = lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let mut spans = vec![Span::styled(format!("{:>width$} ", i + 1), gutter)];
                // Tab stops depend on everything before them on the line, across spans.
                let mut column = 0;
                let ranges = if line.len() > MAX_HIGHLIGHT_LINE {
                    None
                } else {
                    // A grammar that trips on a line leaves that line plain rather than losing the file.
                    state.highlight_line(line, &self.syntaxes).ok()
                };
                match ranges {
                    Some(ranges) => spans.extend(ranges.into_iter().map(|(style, piece)| {
                        Span::styled(clean(piece, &mut column), convert(style))
                    })),
                    None => spans.push(Span::raw(clean(line, &mut column))),
                }
                Line::from(spans)
            })
            .collect();
        (highlighted, name)
    }
}

const TAB_WIDTH: usize = 4;

/// Makes text safe to put in terminal cells: no line endings, tabs expanded to the next stop,
/// control characters replaced. `column` is where `s` starts on the line, and is advanced past it.
fn clean(s: &str, column: &mut usize) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.trim_end_matches(['\n', '\r']).chars() {
        match c {
            '\t' => {
                let pad = TAB_WIDTH - *column % TAB_WIDTH;
                out.extend(std::iter::repeat_n(' ', pad));
                *column += pad;
            }
            c if c.is_control() => {
                out.push('·');
                *column += 1;
            }
            c => {
                out.push(c);
                *column += c.width().unwrap_or(0);
            }
        }
    }
    out
}

fn convert(style: syntect::highlighting::Style) -> Style {
    let fg = style.foreground;
    let mut out = Style::new().fg(Color::Rgb(fg.r, fg.g, fg.b));
    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dir::testutil::TempDir;

    fn preview(path: PathBuf) -> Preview {
        build(
            &Request {
                key: Key { path, diff: false },
                show_hidden: false,
            },
            &mut None,
        )
    }

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn text_files_are_highlighted_with_line_numbers() {
        let tmp = TempDir::new();
        let Preview::Text {
            lines,
            syntax,
            truncated,
        } = preview(tmp.file("main.rs", "fn main() {\n\tlet x = 1;\n}\n"))
        else {
            panic!("expected text");
        };
        assert_eq!(lines.len(), 3);
        assert_eq!(syntax, "Rust");
        assert!(!truncated);
        assert_eq!(text(&lines[0]), "1 fn main() {");
        assert_eq!(text(&lines[1]), "2     let x = 1;");
        assert!(
            lines[0].spans.len() > 2,
            "rust source should get several styled spans"
        );
    }

    #[test]
    fn grammars_cover_what_developers_open() {
        let tmp = TempDir::new();
        for (file, syntax) in [
            ("app.ts", "TypeScript"),
            ("View.tsx", "TypeScriptReact"),
            ("Cargo.toml", "TOML"),
            ("config.fish", "Fish"),
            ("Dockerfile", "Dockerfile"),
            ("tsconfig.jsonc", "JSON"),
            (".env", "DotENV"),
        ] {
            let Preview::Text { syntax: found, .. } = preview(tmp.file(file, "x = 1\n")) else {
                panic!("{file}: expected text");
            };
            assert_eq!(found, syntax, "{file}");
        }
    }

    #[test]
    fn binary_empty_and_missing_files_get_notes() {
        let tmp = TempDir::new();
        let bin = tmp.path().join("blob");
        fs::write(&bin, [0u8, 1, 2, 3]).unwrap();
        assert!(matches!(preview(bin), Preview::Note(n) if n.starts_with("binary")));
        assert!(matches!(preview(tmp.file("e", "")), Preview::Note(n) if n == "empty file"));
        assert!(matches!(preview(tmp.path().join("nope")), Preview::Note(_)));
    }

    #[test]
    fn directories_list_their_entries() {
        let tmp = TempDir::new();
        tmp.file("d/b", "");
        tmp.file("d/a/x", "");
        tmp.file("d/.h", "");
        let Preview::Dir(entries) = preview(tmp.path().join("d")) else {
            panic!("expected dir");
        };
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
    }

    fn git(dir: &Path, args: &[&str]) {
        let mut cmd = Command::new("git");
        for var in git::REPO_ENV {
            cmd.env_remove(var);
        }
        let out = cmd
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    #[test]
    fn diff_shows_what_changed_against_head() {
        let tmp = TempDir::new();
        git(tmp.path(), &["init", "-q"]);
        let file = tmp.file("lib.rs", "one\ntwo\nthree\n");
        git(tmp.path(), &["add", "."]);
        git(tmp.path(), &["commit", "-qm", "init"]);
        fs::write(&file, "one\n2\nthree\nfour\n").unwrap();
        let req = Request {
            key: Key {
                path: file.clone(),
                diff: true,
            },
            show_hidden: false,
        };
        let Preview::Diff {
            lines,
            added,
            removed,
        } = build(&req, &mut None)
        else {
            panic!("expected a diff");
        };
        assert_eq!((added, removed), (2, 1));
        assert!(lines.contains(&DiffLine::Removed("-two".into())));
        assert!(lines.contains(&DiffLine::Added("+four".into())));
        assert!(
            matches!(lines[0], DiffLine::Hunk(_)),
            "headers are skipped: {lines:?}"
        );

        // Content that looks like a file header is still content.
        let sql = tmp.file("q.sql", "-- note\nselect 1;\n");
        git(tmp.path(), &["add", "q.sql"]);
        git(tmp.path(), &["commit", "-qm", "sql"]);
        fs::write(&sql, "select 1;\n++ added\n").unwrap();
        let req_sql = Request {
            key: Key {
                path: sql,
                diff: true,
            },
            show_hidden: false,
        };
        let Preview::Diff {
            lines,
            added,
            removed,
        } = build(&req_sql, &mut None)
        else {
            panic!("expected a diff");
        };
        assert_eq!((added, removed), (1, 1));
        assert!(
            lines.contains(&DiffLine::Removed("--- note".into())),
            "{lines:?}"
        );
        assert!(
            lines.contains(&DiffLine::Added("+++ added".into())),
            "{lines:?}"
        );

        git(tmp.path(), &["checkout", "--", "lib.rs"]);
        assert!(matches!(build(&req, &mut None), Preview::Note(n) if n.contains("no changes")));
    }

    #[test]
    fn clean_strips_control_characters() {
        assert_eq!(clean("\tb\x1b[0m\r\n", &mut 0), "    b·[0m");
    }

    #[test]
    fn tabs_expand_to_the_next_stop() {
        assert_eq!(clean("a\tb", &mut 0), "a   b");
        assert_eq!(clean("abcd\te", &mut 0), "abcd    e");
        // The column carries across spans, as it does between highlighted pieces.
        let mut column = 0;
        let first = clean("ab", &mut column);
        let second = clean("\tc", &mut column);
        assert_eq!(format!("{first}{second}"), "ab  c");
        assert_eq!(column, 5);
    }

    #[test]
    fn highlighted_lines_keep_tab_alignment() {
        let tmp = TempDir::new();
        let Preview::Text { lines, .. } = preview(tmp.file("Makefile", "a:\n\tb\nlong_name:\tc\n"))
        else {
            panic!("expected text");
        };
        assert_eq!(text(&lines[1]), "2     b");
        assert_eq!(text(&lines[2]), "3 long_name:  c");
    }
}
