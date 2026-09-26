//! The preview column, built on a worker thread so highlighting never stalls the cursor.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
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

const MAX_BYTES: u64 = 256 * 1024;
const MAX_LINES: usize = 500;
const MAX_DIR_ENTRIES: usize = 1000;
// Highlighting a minified bundle's single enormous line takes seconds; show such lines plain.
const MAX_HIGHLIGHT_LINE: usize = 2000;

pub enum Preview {
    Dir(Vec<Entry>),
    Text(Vec<Line<'static>>),
    Note(String),
}

pub struct Request {
    pub path: PathBuf,
    pub show_hidden: bool,
}

pub struct Response {
    pub path: PathBuf,
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
                    path: req.path,
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
    let meta = match fs::metadata(&req.path) {
        Ok(meta) => meta,
        Err(e) => return Preview::Note(e.to_string()),
    };
    if meta.is_dir() {
        return match dir::list(&req.path, req.show_hidden) {
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
    if let Err(e) = File::open(&req.path).and_then(|f| f.take(MAX_BYTES).read_to_end(&mut buf)) {
        return Preview::Note(e.to_string());
    }
    if buf[..buf.len().min(8000)].contains(&0) {
        return Preview::Note(format!("binary · {}", dir::human_size(meta.len())));
    }
    let text = String::from_utf8_lossy(&buf);
    Preview::Text(
        highlighter
            .get_or_insert_with(Highlighter::new)
            .highlight(&req.path, &text),
    )
}

struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

impl Highlighter {
    fn new() -> Highlighter {
        let mut themes = ThemeSet::load_defaults();
        Highlighter {
            syntaxes: SyntaxSet::load_defaults_newlines(),
            theme: themes
                .themes
                .remove("base16-ocean.dark")
                .expect("syntect bundles base16-ocean.dark"),
        }
    }

    fn syntax_for(&self, path: &Path, first_line: &str) -> &SyntaxReference {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        // syntect's bundled grammars lack these; the nearest relative reads well enough.
        let ext = match ext {
            "ts" | "tsx" | "mts" | "cts" | "jsx" | "mjs" | "cjs" => "js",
            "fish" | "zsh" | "bash" => "sh",
            "jsonc" | "json5" => "json",
            other => other,
        };
        let s = &self.syntaxes;
        s.find_syntax_by_extension(ext)
            .or_else(|| s.find_syntax_by_extension(name))
            .or_else(|| s.find_syntax_by_first_line(first_line))
            .unwrap_or_else(|| s.find_syntax_plain_text())
    }

    fn highlight(&self, path: &Path, text: &str) -> Vec<Line<'static>> {
        let lines: Vec<&str> = LinesWithEndings::from(text).take(MAX_LINES).collect();
        let syntax = self.syntax_for(path, lines.first().copied().unwrap_or(""));
        let mut state = HighlightLines::new(syntax, &self.theme);
        let width = lines.len().to_string().len();
        let gutter = Style::new().fg(Color::DarkGray);
        lines
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
            .collect()
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
                path,
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
        let Preview::Text(lines) = preview(tmp.file("main.rs", "fn main() {\n\tlet x = 1;\n}\n"))
        else {
            panic!("expected text");
        };
        assert_eq!(lines.len(), 3);
        assert_eq!(text(&lines[0]), "1 fn main() {");
        assert_eq!(text(&lines[1]), "2     let x = 1;");
        assert!(
            lines[0].spans.len() > 2,
            "rust source should get several styled spans"
        );
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
        let Preview::Text(lines) = preview(tmp.file("Makefile", "a:\n\tb\nlong_name:\tc\n")) else {
            panic!("expected text");
        };
        assert_eq!(text(&lines[1]), "2     b");
        assert_eq!(text(&lines[2]), "3 long_name:  c");
    }
}
