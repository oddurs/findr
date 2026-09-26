//! Drawing, by the regions of `docs/design.md`: location and repository along the top;
//! context, listing and inspector across the middle; status and command along the bottom.
//! Colours come from the theme and shapes from the components. Reads `App`, and writes back
//! only the viewport, the page size, and the areas a mouse click maps to.

use std::env;
use std::path::Path;
use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Areas, Confirm, MessageKind, Mode, count, parent_offset};
use crate::dir::{self, Entry, SortKey};
use crate::icons;
use crate::preview::{DiffLine, Preview};

mod components;
pub mod glyphs;
mod overlays;
mod rows;
pub mod theme;

use components::{chip, empty, facts, hints, message, prompt_label};
use overlays::{draw_find, draw_help};
use rows::{Row, entry_line, name_style, status};
use theme::{Theme, Tone};

/// Below this width the context column goes; below the next, the inspector does too.
const WITH_PARENT: u16 = 90;
const WITH_PREVIEW: u16 = 60;

/// The screen's regions for one frame.
struct Regions {
    location: Rect,
    context: Rect,
    listing: Rect,
    filter: Rect,
    title: Rect,
    facts: Rect,
    content: Rect,
    bar: Rect,
    /// Zero height unless key hints are turned on.
    hints: Rect,
}

/// What the bottom of the screen shows beyond what it must.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bottom {
    /// A second row of keys for the selection. Off by default: `?` has them all.
    pub hints: bool,
    /// `4 of 10` at the right of the bar.
    pub position: bool,
}

impl Default for Bottom {
    fn default() -> Bottom {
        Bottom {
            hints: false,
            position: true,
        }
    }
}

pub fn draw(frame: &mut Frame, app: &mut App, theme: &Theme, bottom: Bottom) {
    let r = layout(frame, app, theme, bottom);
    // Recorded so a mouse click can be mapped back to the row under it.
    app.areas = Areas {
        parent: r.context,
        current: r.listing,
        preview: r.content,
    };

    draw_location(frame, app, theme, r.location);
    if r.context.width > 0 {
        draw_context(frame, app, theme, r.context);
    }
    draw_listing(frame, app, theme, r.listing);
    if r.filter.height > 0 {
        draw_filter(frame, app, theme, r.filter);
    }
    if r.content.width > 0 {
        draw_inspector(frame, app, theme, &r);
    }
    draw_bar(frame, app, theme, bottom, r.bar);
    if r.hints.height > 0 {
        draw_hints(frame, app, theme, r.hints);
    }
    match &app.mode {
        Mode::Help => draw_help(frame, frame.area(), theme),
        Mode::Find(finder) => draw_find(frame, finder, frame.area(), theme),
        _ => {}
    }
}

fn layout(frame: &mut Frame, app: &App, theme: &Theme, bottom: Bottom) -> Regions {
    let [location, body, bar, hints] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(u16::from(bottom.hints)),
    ])
    .areas(frame.area());
    // Narrow terminals give up the context column first, then the inspector, so the listing
    // always keeps a usable width.
    let widths = match body.width {
        w if w >= WITH_PARENT => [18, 34, 48],
        w if w >= WITH_PREVIEW => [0, 42, 58],
        _ => [0, 100, 0],
    };
    let [context, listing, inspector] =
        Layout::horizontal(widths.map(Constraint::Percentage)).areas(body);
    let listing = if context.width > 0 {
        rule(frame, theme, listing)
    } else {
        listing
    };
    // A filter is shown on the listing it narrows.
    let filtering = matches!(app.mode, Mode::Filter) || !app.filter.is_empty();
    let [listing, filter] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(u16::from(filtering))])
            .areas(listing);
    let (title, facts, content) = if inspector.width > 0 {
        let inner = rule(frame, theme, inspector);
        let inner = Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(1),
            ..inner
        };
        let [title, facts, content] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(inner);
        (title, facts, content)
    } else {
        Default::default()
    };
    Regions {
        location,
        context,
        listing,
        filter,
        title,
        facts,
        content,
        bar,
        hints,
    }
}

/// The faint rule between columns; returns what is right of it.
fn rule(frame: &mut Frame, theme: &Theme, area: Rect) -> Rect {
    let set = border::Set {
        vertical_left: theme.glyphs.rule,
        ..border::PLAIN
    };
    let block = Block::new()
        .borders(Borders::LEFT)
        .border_set(set)
        .border_style(theme.faint());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

/// Location on the left, the repository's state on the right.
fn draw_location(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let path = tildify(&app.cwd);
    let (dir, here) = match path.rsplit_once('/') {
        Some((dir, here)) if !here.is_empty() => (format!("{dir}/"), here.to_string()),
        _ => (String::new(), path),
    };
    let left = Line::from(vec![
        Span::raw(" "),
        Span::styled(dir, theme.muted()),
        Span::styled(here, Style::new().add_modifier(Modifier::BOLD)),
    ]);

    let mut right = Vec::new();
    if let Some(repo) = &app.git {
        let icon = theme.glyphs.branch;
        let branch = repo.branch.as_deref().unwrap_or("?");
        right.push(Span::styled(
            format!("{icon}{branch}"),
            Style::new().fg(theme.branch),
        ));
        if repo.is_dirty() {
            right.push(Span::styled(
                format!(" {}", theme.glyphs.dot),
                Style::new().fg(theme.warning),
            ));
        }
        if repo.ahead > 0 {
            right.push(Span::styled(
                format!(" {}{}", theme.glyphs.up, repo.ahead),
                Style::new().fg(theme.success),
            ));
        }
        if repo.behind > 0 {
            right.push(Span::styled(
                format!(" {}{}", theme.glyphs.down, repo.behind),
                Style::new().fg(theme.danger),
            ));
        }
        right.push(Span::raw(" "));
    }
    split_line(frame, area, left, Line::from(right));
}

/// Draws `left` and `right` on one line, the right flush to the edge.
fn split_line(frame: &mut Frame, area: Rect, left: Line, right: Line) {
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right.width() as u16)])
        .areas(area);
    frame.render_widget(Paragraph::new(left), l);
    frame.render_widget(Paragraph::new(right), r);
}

fn tildify(path: &Path) -> String {
    if let Some(home) = env::var_os("HOME")
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

/// The parent directory, muted, with where you are picked out.
fn draw_context(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let height = area.height as usize;
    let sel = app.parent_selected.unwrap_or(0);
    let offset = parent_offset(sel, app.parent.len(), height);
    let lines: Vec<Line> = app.parent[offset..]
        .iter()
        .take(height)
        .enumerate()
        .map(|(i, e)| {
            let here = Some(offset + i) == app.parent_selected;
            let row = Row {
                theme,
                width: area.width as usize,
                size: false,
                muted: !here,
            };
            let line = entry_line(e, &[], status(app.git.as_ref(), e), false, &row);
            if here {
                line.patch_style(theme.context_selected())
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_listing(frame: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let height = (area.height as usize).max(1);
    app.page = height;
    if app.view.is_empty() {
        let note = if app.filter.is_empty() {
            "empty directory"
        } else {
            "nothing matches"
        };
        frame.render_widget(Paragraph::new(empty(note, theme)), area);
        return;
    }
    // Scroll only as far as needed to keep the cursor visible.
    if app.selected < app.offset {
        app.offset = app.selected;
    } else if app.selected >= app.offset + height {
        app.offset = app.selected + 1 - height;
    }
    app.offset = app.offset.min(app.view.len().saturating_sub(height));

    // A scrollbar only when there is somewhere to scroll, and then the rows give up a column.
    let overflow = app.view.len() > height;
    let row = Row {
        theme,
        width: area.width as usize - usize::from(overflow),
        size: true,
        muted: false,
    };
    let lines: Vec<Line> = app.view[app.offset..]
        .iter()
        .take(height)
        .enumerate()
        .map(|(i, v)| {
            let e = &app.entries[v.idx];
            let marked = app.marked.contains(&e.path);
            let status = status(app.git.as_ref(), e);
            let mut line = entry_line(e, &v.hits, status, marked, &row);
            if app.offset + i != app.selected {
                return line;
            }
            // The cursor row carries an accent bar in the mark column, unless it is marked.
            if !marked {
                line.spans[0] = Span::styled(theme.glyphs.cursor, Style::new().fg(theme.accent));
            }
            line.patch_style(theme.selected())
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
    if overflow {
        let mut state = ScrollbarState::new(app.view.len() - height + 1)
            .viewport_content_length(height)
            .position(app.offset);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(None)
            .thumb_symbol(theme.glyphs.thumb)
            .thumb_style(theme.muted());
        frame.render_stateful_widget(bar, area, &mut state);
    }
}

/// The filter, on the listing it narrows: the query, and how much of the directory it kept.
fn draw_filter(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let left = Line::from(vec![
        Span::styled(
            " / ",
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ),
        Span::raw(app.filter.clone()),
    ]);
    let kept = format!("{} of {} ", app.view.len(), app.entries.len());
    split_line(frame, area, left, Line::styled(kept, theme.muted()));
    if matches!(app.mode, Mode::Filter) {
        let x = area.x + 3 + app.filter.width() as u16;
        frame.set_cursor_position((x.min(area.right().saturating_sub(1)), area.y));
    }
}

/// What the selection is (title), the facts about it, then what is in it.
fn draw_inspector(frame: &mut Frame, app: &App, theme: &Theme, r: &Regions) {
    let Some(entry) = app.selected() else {
        return;
    };
    let preview = app
        .preview
        .as_ref()
        .filter(|(key, _)| key.path == entry.path)
        .map(|(_, p)| p);

    let mut title = Vec::new();
    if theme.glyphs.icons() {
        let icon = icons::for_entry(entry);
        title.push(Span::styled(
            format!("{} ", icon.glyph),
            Style::new().fg(icon.color),
        ));
    }
    title.push(Span::styled(
        entry.name.clone(),
        name_style(entry, None, theme).add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(Line::from(title)), r.title);

    let mut line = facts(&entry_facts(entry, preview, theme), theme);
    if entry.is_symlink
        && let Ok(target) = std::fs::read_link(&entry.path)
    {
        line.push(Span::styled(
            format!("  {} {}", theme.glyphs.arrow, target.display()),
            Style::new().fg(theme.link),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(line)), r.facts);

    let area = r.content;
    let height = area.height as usize;
    let lines: Vec<Line> = match preview {
        None | Some(Preview::Note(_)) => Vec::new(),
        Some(Preview::Dir(entries)) if entries.is_empty() => vec![empty("empty directory", theme)],
        Some(Preview::Dir(entries)) => {
            let row = Row {
                theme,
                width: area.width as usize,
                size: true,
                muted: false,
            };
            entries
                .iter()
                .skip(app.preview_scroll)
                .take(height)
                .map(|e| entry_line(e, &[], status(app.git.as_ref(), e), false, &row))
                .collect()
        }
        Some(Preview::Text { lines, .. }) => lines
            .iter()
            .skip(app.preview_scroll)
            .take(height)
            .cloned()
            .collect(),
        Some(Preview::Diff { lines, .. }) => lines
            .iter()
            .skip(app.preview_scroll)
            .take(height)
            .map(|line| diff_line(line, area.width as usize, theme))
            .collect(),
    };
    frame.render_widget(Paragraph::new(lines), area);
}

/// A diff line: the marker in its semantic role (added success, removed danger), then the
/// highlighted code on a faint wash of the same meaning. Hunk headers are info.
fn diff_line(line: &DiffLine, width: usize, theme: &Theme) -> Line<'static> {
    let (marker, code, color, bg) = match line {
        DiffLine::Hunk(t) => return Line::styled(t.clone(), Style::new().fg(theme.info)),
        DiffLine::Added(code) => ("+", code, theme.success, Some(theme.added_bg)),
        DiffLine::Removed(code) => ("-", code, theme.danger, Some(theme.removed_bg)),
        DiffLine::Context(code) => (" ", code, theme.muted, None),
    };
    let mut spans = vec![Span::styled(marker, Style::new().fg(color))];
    spans.extend(code.spans.iter().cloned());
    let mut line = Line::from(spans);
    match bg {
        Some(bg) => {
            // The wash runs to the edge, as a changed row, not just under the text.
            let pad = width.saturating_sub(line.width());
            line.spans.push(Span::raw(" ".repeat(pad)));
            line.patch_style(Style::new().bg(bg))
        }
        None => line,
    }
}

/// The inspector's facts line: what kind of thing, how big, how old, who may touch it.
fn entry_facts(entry: &Entry, preview: Option<&Preview>, theme: &Theme) -> Vec<String> {
    let mut facts = match preview {
        Some(Preview::Dir(entries)) => vec![count(entries.len())],
        Some(Preview::Text {
            syntax,
            lines,
            truncated,
        }) => {
            let more = if *truncated { "+" } else { "" };
            let n = lines.len();
            let noun = if n == 1 && !truncated {
                "line"
            } else {
                "lines"
            };
            vec![
                syntax.clone(),
                dir::human_size(entry.size),
                format!("{n}{more} {noun}"),
            ]
        }
        Some(Preview::Diff { added, removed, .. }) => vec![
            "changes against HEAD".into(),
            format!("+{added} {}{removed}", theme.glyphs.minus),
        ],
        Some(Preview::Note(note)) => vec![note.clone()],
        None if entry.is_dir => Vec::new(),
        None => vec![dir::human_size(entry.size)],
    };
    let age = entry
        .modified
        .map(|t| dir::fmt_age(t, SystemTime::now()))
        .unwrap_or_default();
    facts.push(age);
    facts.push(dir::mode_string(entry));
    facts
}

/// The bar: on the left what is happening now, on the right where you are. It shows only what
/// applies — a prompt, a question, what just happened, or what is waiting — and is otherwise
/// nearly empty.
fn draw_bar(frame: &mut Frame, app: &App, theme: &Theme, bottom: Bottom, area: Rect) {
    let mut right = Vec::new();
    if bottom.position && !app.view.is_empty() {
        let at = format!("{} of {} ", app.selected + 1, app.view.len());
        right.push(Span::styled(at, theme.muted()));
    }
    let right = Line::from(right);
    let left = match &app.mode {
        Mode::Input(input) => {
            let label = prompt_label(input.kind.prompt(), theme);
            let before_cursor = input.text[..input.byte(input.cursor)].width();
            let x = area.x + (label.width() + before_cursor) as u16;
            frame.set_cursor_position((x.min(area.right().saturating_sub(1)), area.y));
            Line::from(vec![label, Span::raw(input.text.clone())])
        }
        // Some things must be on screen: the question being asked and the keys that answer it.
        Mode::Confirm(confirm) => {
            let question = match confirm {
                Confirm::Trash(paths) => {
                    let what = match paths.as_slice() {
                        [one] => one.file_name().map_or_else(
                            || one.display().to_string(),
                            |n| n.to_string_lossy().into_owned(),
                        ),
                        many => count(many.len()),
                    };
                    format!(" Move {what} to the Trash?   ")
                }
                Confirm::Quit { .. } => " A paste is still running. Quit anyway?   ".into(),
            };
            let mut spans = vec![Span::styled(
                question,
                Style::new().add_modifier(Modifier::BOLD),
            )];
            spans.extend(hints(&owned(&[("y", "yes"), ("n", "no")]), theme));
            Line::from(spans)
        }
        Mode::Goto => {
            let mut spans = vec![Span::styled(" go to   ", theme.muted())];
            let keys = [
                ("g", "top"),
                ("h", "home"),
                ("r", "repository"),
                ("/", "root"),
            ];
            spans.extend(hints(&owned(&keys), theme));
            Line::from(spans)
        }
        _ => match app.message() {
            Some(m) => {
                let tone = match m.kind {
                    MessageKind::Success => Tone::Success,
                    MessageKind::Error => Tone::Danger,
                    MessageKind::Info => Tone::Muted,
                };
                message(&m.text, tone, theme)
            }
            None => pending(app, theme, bottom),
        },
    };
    split_line(frame, area, left, right);
}

/// What is waiting or differs from normal, quietly. Nothing at all when nothing is.
fn pending(app: &App, theme: &Theme, bottom: Bottom) -> Line<'static> {
    let g = theme.glyphs;
    let mut chips: Vec<Vec<Span<'static>>> = Vec::new();
    if !app.marked.is_empty() {
        chips.push(vec![
            chip(g.dot.to_string(), Tone::Mark, theme),
            Span::styled(format!(" {} marked", app.marked.len()), theme.muted()),
        ]);
    }
    if let Some(job) = &app.job {
        let text = format!("pasting {} of {}", job.done + 1, job.total);
        chips.push(vec![chip(text, Tone::Info, theme)]);
    }
    if let Some(clip) = &app.clip {
        let (glyph, verb) = if clip.cut {
            (g.cut, "to move")
        } else {
            (g.copied, "copied")
        };
        chips.push(vec![
            chip(glyph.to_string(), Tone::Warning, theme),
            Span::styled(format!("{} {verb}", count(clip.paths.len())), theme.muted()),
        ]);
    }
    if app.diff {
        chips.push(vec![Span::styled("showing diffs", theme.muted())]);
    }
    if app.show_hidden {
        chips.push(vec![Span::styled("showing hidden", theme.muted())]);
    }
    // Name, a to z, is what everyone expects; say so only when it is something else.
    if app.sort != SortKey::Name || app.reverse {
        let arrow = if app.reverse { g.up } else { g.down };
        let text = format!("by {} {arrow}", app.sort.label());
        chips.push(vec![Span::styled(text, theme.muted())]);
    }
    if chips.is_empty() && !bottom.hints {
        // The one pointer to everything else, as quiet as the screen can make it.
        return Line::from(Span::styled(" ? for keys", theme.faint()));
    }
    let mut spans = vec![Span::raw(" ")];
    for (i, chip) in chips.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("    "));
        }
        spans.extend(chip);
    }
    Line::from(spans)
}

/// The optional second row: keys for the selection and the mode.
fn draw_hints(frame: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let up_down = theme.glyphs.up_down;
    let pairs = match &app.mode {
        Mode::Normal => browse_hints(app),
        Mode::Filter => owned(&[("enter", "keep"), ("esc", "clear"), (up_down, "move")]),
        Mode::Find(_) => owned(&[("enter", "go there"), ("esc", "cancel"), (up_down, "move")]),
        Mode::Input(_) => owned(&[("enter", "ok"), ("esc", "cancel")]),
        Mode::Help => owned(&[("any key", "close")]),
        Mode::Confirm(_) | Mode::Goto => Vec::new(),
    };
    let mut spans = vec![Span::raw(" ")];
    spans.extend(hints(&pairs, theme));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn owned(pairs: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    pairs.iter().map(|(k, l)| (*k, l.to_string())).collect()
}

/// What can be done next, given what is selected and what is pending. The full list is `?`.
fn browse_hints(app: &App) -> Vec<(&'static str, String)> {
    let mut h = Vec::new();
    if !app.marked.is_empty() {
        let n = app.marked.len();
        h.push(("y", format!("copy {n}")));
        h.push(("x", format!("cut {n}")));
        h.push(("d", format!("trash {n}")));
        h.push(("r", format!("rename {n}")));
        h.push(("esc", "unmark".into()));
        return h;
    }
    if let Some(clip) = &app.clip {
        h.push(("p", format!("paste {} here", count(clip.paths.len()))));
    }
    match app.selected() {
        Some(e) if e.is_dir => h.push(("l", "open".into())),
        Some(e) => {
            h.push(("e", "edit".into()));
            h.push(("o", "open".into()));
            if app.has_changes(e) {
                h.push(("D", if app.diff { "content" } else { "diff" }.into()));
            }
        }
        None => h.push(("a", "new file".into())),
    }
    if app.filter.is_empty() {
        h.push(("/", "filter".into()));
    } else {
        h.push(("esc", "clear filter".into()));
    }
    h.push(("f", "find".into()));
    h.push(("?", "keys".into()));
    h
}

#[cfg(test)]
mod tests {
    use super::overlays::{HELP, Help};
    use super::*;
    use crate::dir::testutil::TempDir;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};

    fn render(app: &mut App, width: u16, height: u16) -> String {
        render_with(app, width, height, &Theme::DARK, Bottom::default())
    }

    fn render_with(
        app: &mut App,
        width: u16,
        height: u16,
        theme: &Theme,
        bottom: Bottom,
    ) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app, theme, bottom)).unwrap();
        let buf = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn main_view() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let tmp = TempDir::new();
        tmp.file("src/lib.rs", "");
        tmp.file("src/main.rs", "fn main() {}\n");
        tmp.file("notes.txt", "hello");
        tmp.file("a-rather-long-file-name-for-truncation.md", "");
        let mode = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(tmp.path().join("src"), mode).unwrap();
        let mut app = App::new(tmp.path(), None).unwrap();
        app.sync_preview();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !app.poll_preview() {
            assert!(Instant::now() < deadline, "preview never arrived");
            std::thread::sleep(Duration::from_millis(5));
        }

        let screen = render(&mut app, 100, 8);
        let rows: Vec<&str> = screen.lines().collect();
        // The header holds the temp path and the parent column lists whatever else is in the
        // temp dir, so the snapshot takes the body right of the parent column plus the bottom bars.
        let parent = app.areas.parent.width as usize;
        assert!(parent > 0, "wide enough for the parent column");
        let body = rows[1..rows.len() - 1]
            .iter()
            .map(|r| r.chars().skip(parent).collect::<String>());
        let snapshot: Vec<String> = body
            .chain(rows[rows.len() - 1..].iter().map(|r| r.to_string()))
            .collect();
        insta::assert_snapshot!(snapshot.join("\n"));
        assert_eq!(app.page, 6);
    }

    #[test]
    fn help_overlay_draws() {
        let tmp = TempDir::new();
        let mut app = App::new(tmp.path(), None).unwrap();
        app.mode = Mode::Help;
        for (width, height) in [(120, 40), (120, 24), (120, 16), (200, 12)] {
            let screen = render(&mut app, width, height);
            for line in HELP {
                let (Help::Key(_, text) | Help::Section(text)) = line;
                assert!(
                    screen.contains(text),
                    "{text:?} missing at {width}x{height}:\n{screen}"
                );
            }
        }
    }

    #[test]
    fn narrow_terminals_drop_the_parent_then_the_preview() {
        let tmp = TempDir::new();
        tmp.file("src/lib.rs", "");
        let mut app = App::new(tmp.path(), None).unwrap();
        render(&mut app, 120, 10);
        assert!(app.areas.parent.width > 0 && app.areas.preview.width > 0);
        render(&mut app, 70, 10);
        assert_eq!(app.areas.parent.width, 0);
        assert!(app.areas.preview.width > 0);
        let screen = render(&mut app, 40, 10);
        assert_eq!(app.areas.preview.width, 0);
        assert!(screen.contains("src/"), "{screen}");
    }

    fn setup() -> (TempDir, App) {
        let tmp = TempDir::new();
        tmp.file("src/lib.rs", "");
        tmp.file("notes.txt", "hello");
        tmp.file("README.md", "");
        let app = App::new(tmp.path(), None).unwrap();
        (tmp, app)
    }

    fn keys(app: &App) -> Vec<String> {
        browse_hints(app)
            .into_iter()
            .map(|(k, l)| format!("{k} {l}"))
            .collect()
    }

    #[test]
    fn hints_follow_the_selection_and_what_is_pending() {
        let (tmp, mut app) = setup();
        assert!(
            keys(&app).contains(&"l open".to_string()),
            "a directory opens"
        );
        app.selected = 1;
        let file = keys(&app);
        assert!(file.contains(&"e edit".to_string()) && file.contains(&"o open".to_string()));
        app.clip = Some(crate::app::Clip {
            paths: vec![tmp.path().join("x")],
            cut: false,
        });
        assert_eq!(keys(&app)[0], "p paste 1 item here");
        app.marked.insert(tmp.path().join("notes.txt"));
        assert_eq!(
            keys(&app),
            [
                "y copy 1",
                "x cut 1",
                "d trash 1",
                "r rename 1",
                "esc unmark"
            ]
        );
    }

    #[test]
    fn the_filter_is_shown_on_the_listing() {
        let (_tmp, mut app) = setup();
        for c in ['/', 'e'] {
            app.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        let screen = render(&mut app, 100, 10);
        let rows: Vec<&str> = screen.lines().collect();
        // Rows: location, eight of body, the bar. The filter is the body's last.
        let filter_row = rows[8];
        assert!(
            filter_row.contains("/ e"),
            "filter row: {filter_row:?}\n{screen}"
        );
        assert!(filter_row.contains("2 of 3"), "{screen}");
        assert!(!rows[0].contains("/e"), "not in the location bar");
        assert_eq!(app.areas.current.height, 7, "the listing gives up a row");
    }

    #[test]
    fn facts_live_in_the_inspector_not_the_status_bar() {
        let (_tmp, mut app) = setup();
        app.selected = 1;
        let screen = render(&mut app, 100, 10);
        let rows: Vec<&str> = screen.lines().collect();
        assert!(
            rows[2].contains("5 B") && rows[2].contains("rw"),
            "facts row: {}",
            rows[2]
        );
        let bar = rows[rows.len() - 1];
        assert!(bar.contains("2 of 3") && !bar.contains("rw-"), "bar: {bar}");
    }

    #[test]
    fn pending_state_shows_as_chips() {
        let (tmp, mut app) = setup();
        app.marked.insert(tmp.path().join("notes.txt"));
        app.show_hidden = true;
        let screen = render(&mut app, 100, 10);
        let bar = screen.lines().last().unwrap();
        assert!(
            bar.contains("1 marked") && bar.contains("showing hidden"),
            "{bar}"
        );
        assert!(
            !bar.contains("for keys"),
            "the pointer gives way to real state"
        );
    }

    #[test]
    fn the_bar_is_quiet_when_nothing_is_happening() {
        let (_tmp, mut app) = setup();
        let screen = render(&mut app, 100, 10);
        let bar = screen.lines().last().unwrap();
        let words: Vec<&str> = bar.split_whitespace().collect();
        assert_eq!(words, ["?", "for", "keys", "1", "of", "3"], "bar: {bar:?}");
    }

    #[test]
    fn a_sort_other_than_name_says_so() {
        let (_tmp, mut app) = setup();
        app.sort = SortKey::Size;
        let screen = render(&mut app, 100, 10);
        assert!(
            screen.lines().last().unwrap().contains("by size"),
            "{screen}"
        );
    }

    #[test]
    fn hints_and_position_are_settings() {
        let (_tmp, mut app) = setup();
        let bottom = Bottom {
            hints: true,
            position: false,
        };
        let screen = render_with(&mut app, 100, 10, &Theme::DARK, bottom);
        let rows: Vec<&str> = screen.lines().collect();
        assert!(
            rows[9].contains("l open") && rows[9].contains("f find"),
            "{screen}"
        );
        assert!(!rows[8].contains(" of "), "position off: {:?}", rows[8]);
        assert!(
            !rows[8].contains("for keys"),
            "the hints row points to ? already"
        );
        assert_eq!(app.page, 7, "the hints row costs the listing a row");
    }

    #[test]
    fn an_ascii_terminal_gets_an_ascii_screen() {
        let (tmp, mut app) = setup();
        app.marked.insert(tmp.path().join("notes.txt"));
        let ascii = Theme {
            glyphs: super::glyphs::Glyphs::ASCII,
            ..Theme::DARK
        };
        for bottom in [
            Bottom::default(),
            Bottom {
                hints: true,
                position: true,
            },
        ] {
            let screen = render_with(&mut app, 100, 10, &ascii, bottom);
            let odd: Vec<char> = screen.chars().filter(|c| !c.is_ascii()).collect();
            assert!(odd.is_empty(), "non-ASCII {odd:?} in\n{screen}");
        }
    }

    #[test]
    fn diff_lines_keep_code_colour_on_a_wash() {
        let t = Theme::DARK;
        let code = Line::from(Span::styled("fn", Style::new().fg(t.accent)));
        let added = diff_line(&DiffLine::Added(code.clone()), 10, &t);
        assert_eq!(added.spans[0].content, "+");
        assert_eq!(added.spans[0].style.fg, Some(t.success));
        assert_eq!(added.style.bg, Some(t.added_bg), "the whole line is washed");
        assert_eq!(added.width(), 10, "to the edge of the inspector");
        assert_eq!(
            added.spans[1].style.fg,
            Some(t.accent),
            "code keeps its colour"
        );
        let removed = diff_line(&DiffLine::Removed(code.clone()), 10, &t);
        assert_eq!(removed.style.bg, Some(t.removed_bg));
        let context = diff_line(&DiffLine::Context(code), 10, &t);
        assert_eq!(context.style.bg, None);
    }

    #[test]
    fn the_light_theme_changes_the_surfaces() {
        let (_tmp, mut app) = setup();
        let bg = |theme: &Theme, app: &mut App| {
            let mut terminal = Terminal::new(TestBackend::new(100, 10)).unwrap();
            terminal
                .draw(|f| draw(f, app, theme, Bottom::default()))
                .unwrap();
            let x = app.areas.current.x + 3;
            terminal.backend().buffer()[(x, app.areas.current.y)].bg
        };
        assert_eq!(bg(&Theme::DARK, &mut app), Theme::DARK.selected_bg);
        assert_eq!(bg(&Theme::LIGHT, &mut app), Theme::LIGHT.selected_bg);
    }
}
