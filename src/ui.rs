//! Drawing. Reads `App`; writes back only the list viewport and page size.

use std::env;
use std::path::Path;
use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Areas, Confirm, FIND_LIMIT, Finder, Mode, parent_offset};
use crate::dir::{self, Entry};
use crate::git::{Repo, Status};
use crate::icons;
use crate::preview::Preview;

const SELECTED: Style = Style::new()
    .bg(Color::Indexed(237))
    .add_modifier(Modifier::BOLD);
const PARENT_SELECTED: Style = Style::new().bg(Color::Indexed(236));
const DIM: Style = Style::new().fg(Color::DarkGray);
const ACCENT: Color = Color::Blue;
/// Below this width the parent column goes; below the next, the preview does too.
const WITH_PARENT: u16 = 90;
const WITH_PREVIEW: u16 = 60;

enum Help {
    Section(&'static str),
    Key(&'static str, &'static str),
}

const HELP: &[Help] = &[
    Help::Section("Move"),
    Help::Key("j k  ↑ ↓", "down, up"),
    Help::Key("h l  ← →", "parent, open"),
    Help::Key("enter", "enter directory or edit file"),
    Help::Key("gg G  ^d ^u", "top, bottom, half page"),
    Help::Key("gh gr g/", "home, git root, root"),
    Help::Key("-", "previous directory"),
    Help::Key(":", "go to a path"),
    Help::Section("Find"),
    Help::Key("/", "fuzzy filter this directory"),
    Help::Key("f ^p", "find below, per .gitignore"),
    Help::Key("esc", "clear filter, then marks"),
    Help::Section("Files"),
    Help::Key("space", "mark"),
    Help::Key("y x p", "copy, cut, paste"),
    Help::Key("d", "move to trash"),
    Help::Key("r", "rename"),
    Help::Key("a A", "new file or path, new dir"),
    Help::Key("c", "copy path to clipboard"),
    Help::Section("Open"),
    Help::Key("e", "edit in $EDITOR"),
    Help::Key("o", "open with the default app"),
    Help::Key("!", "shell here"),
    Help::Section("View"),
    Help::Key("J K", "scroll preview"),
    Help::Key(".", "show hidden files"),
    Help::Key("s S", "cycle sort, reverse"),
    Help::Key("R", "reload"),
    Help::Key("mouse", "wheel, click, double-click"),
    Help::Section("Quit"),
    Help::Key("q Q", "quit and cd, quit"),
];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, status, command] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    // Narrow terminals give up the parent column first, then the preview, so the list
    // itself always keeps a usable width.
    let widths = match body.width {
        w if w >= WITH_PARENT => [18, 34, 48],
        w if w >= WITH_PREVIEW => [0, 42, 58],
        _ => [0, 100, 0],
    };
    let [parent, current, preview] =
        Layout::horizontal(widths.map(Constraint::Percentage)).areas(body);

    let current = if parent.width > 0 {
        column(frame, current)
    } else {
        current
    };
    let (preview_title, preview) = if preview.width > 0 {
        let inner = column(frame, preview);
        let inner = Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(1),
            ..inner
        };
        let [title, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        (title, rest)
    } else {
        (Rect::default(), Rect::default())
    };
    // Recorded so a mouse click can be mapped back to the row under it.
    app.areas = Areas {
        parent,
        current,
        preview,
    };

    draw_header(frame, app, header);
    if parent.width > 0 {
        draw_parent(frame, app, parent);
    }
    draw_current(frame, app, current);
    if preview.width > 0 {
        draw_preview_title(frame, app, preview_title);
        draw_preview(frame, app, preview);
    }
    draw_status(frame, app, status);
    draw_command(frame, app, command);
    match &app.mode {
        Mode::Help => draw_help(frame, body),
        Mode::Find(finder) => draw_find(frame, finder, body, app.icons),
        _ => {}
    }
}

fn column(frame: &mut Frame, area: Rect) -> Rect {
    let block = Block::new().borders(Borders::LEFT).border_style(DIM);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    // The path dim up to the last segment, which is where you are.
    let path = tildify(&app.cwd);
    let (dir, here) = match path.rsplit_once('/') {
        Some((dir, here)) if !here.is_empty() => (format!("{dir}/"), here.to_string()),
        _ => (String::new(), path),
    };
    let left = Line::from(vec![
        Span::raw(" "),
        Span::styled(dir, Style::new().fg(ACCENT)),
        Span::styled(here, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
    ]);

    let mut right = Vec::new();
    if !app.filter.is_empty() && !matches!(app.mode, Mode::Filter) {
        right.push(Span::styled(
            format!("/{}  ", app.filter),
            Style::new().fg(Color::Yellow),
        ));
    }
    if let Some(repo) = &app.git {
        let icon = if app.icons { "\u{e725} " } else { "" };
        let branch = repo.branch.as_deref().unwrap_or("?");
        right.push(Span::styled(
            format!("{icon}{branch}"),
            Style::new().fg(Color::Magenta),
        ));
        if repo.is_dirty() {
            right.push(Span::styled(" ●", Style::new().fg(Color::Yellow)));
        }
        if repo.ahead > 0 {
            right.push(Span::styled(
                format!(" ↑{}", repo.ahead),
                Style::new().fg(Color::Green),
            ));
        }
        if repo.behind > 0 {
            right.push(Span::styled(
                format!(" ↓{}", repo.behind),
                Style::new().fg(Color::Red),
            ));
        }
        right.push(Span::raw(" "));
    }
    let right = Line::from(right);
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

fn draw_parent(frame: &mut Frame, app: &App, area: Rect) {
    let height = area.height as usize;
    let width = area.width as usize;
    let sel = app.parent_selected.unwrap_or(0);
    let offset = parent_offset(sel, app.parent.len(), height);
    let lines: Vec<Line> = app.parent[offset..]
        .iter()
        .take(height)
        .enumerate()
        .map(|(i, e)| {
            let status = status(app.git.as_ref(), e);
            let line = entry_line(e, &[], status, false, width, false, app.icons);
            if Some(offset + i) == app.parent_selected {
                line.patch_style(PARENT_SELECTED)
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_current(frame: &mut Frame, app: &mut App, inner: Rect) {
    let height = (inner.height as usize).max(1);
    app.page = height;
    if app.view.is_empty() {
        let note = if app.filter.is_empty() {
            "  empty directory".to_string()
        } else {
            format!("  nothing matches /{}", app.filter)
        };
        frame.render_widget(Paragraph::new(Span::styled(note, DIM)), inner);
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
    let width = inner.width as usize - usize::from(overflow);
    let lines: Vec<Line> = app.view[app.offset..]
        .iter()
        .take(height)
        .enumerate()
        .map(|(i, v)| {
            let e = &app.entries[v.idx];
            let marked = app.marked.contains(&e.path);
            let status = status(app.git.as_ref(), e);
            let mut line = entry_line(e, &v.hits, status, marked, width, true, app.icons);
            if app.offset + i != app.selected {
                return line;
            }
            // The cursor row carries an accent bar in the mark column, unless it is marked.
            if !marked {
                line.spans[0] = Span::styled("▌", Style::new().fg(ACCENT));
            }
            line.patch_style(SELECTED)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
    if overflow {
        let mut state = ScrollbarState::new(app.view.len() - height + 1)
            .viewport_content_length(height)
            .position(app.offset);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(None)
            .thumb_symbol("▐")
            .thumb_style(DIM);
        frame.render_stateful_widget(bar, inner, &mut state);
    }
}

/// What the preview is of: the entry's icon and name, then a dim summary.
fn draw_preview_title(frame: &mut Frame, app: &App, area: Rect) {
    let Some(entry) = app.selected() else {
        return;
    };
    let mut spans = Vec::new();
    if app.icons {
        let icon = icons::for_entry(entry);
        spans.push(Span::styled(
            format!("{} ", icon.glyph),
            Style::new().fg(icon.color),
        ));
    }
    spans.push(Span::styled(
        entry.name.clone(),
        name_style(entry, None).add_modifier(Modifier::BOLD),
    ));
    let preview = app.preview.as_ref().filter(|(p, _)| *p == entry.path);
    let summary = match preview.map(|(_, p)| p) {
        Some(Preview::Dir(entries)) => match entries.len() {
            1 => "1 item".to_string(),
            n => format!("{n} items"),
        },
        Some(Preview::Text {
            syntax, truncated, ..
        }) => {
            let mut s = format!("{syntax} · {}", dir::human_size(entry.size));
            if *truncated {
                s.push_str(" · showing the start");
            }
            s
        }
        Some(Preview::Note(note)) => note.clone(),
        None => String::new(),
    };
    if !summary.is_empty() {
        spans.push(Span::styled(format!("  {summary}"), DIM));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_preview(frame: &mut Frame, app: &App, inner: Rect) {
    let Some(selected) = app.selected() else {
        return;
    };
    let Some((_, preview)) = app.preview.as_ref().filter(|(p, _)| *p == selected.path) else {
        return;
    };
    let height = inner.height as usize;
    let lines: Vec<Line> = match preview {
        Preview::Dir(entries) if entries.is_empty() => vec![Line::styled("empty directory", DIM)],
        Preview::Dir(entries) => entries
            .iter()
            .skip(app.preview_scroll)
            .take(height)
            .map(|e| {
                let status = status(app.git.as_ref(), e);
                entry_line(e, &[], status, false, inner.width as usize, true, app.icons)
            })
            .collect(),
        Preview::Text { lines, .. } => lines
            .iter()
            .skip(app.preview_scroll)
            .take(height)
            .cloned()
            .collect(),
        // Already said in the title.
        Preview::Note(_) => Vec::new(),
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

fn status(git: Option<&Repo>, entry: &Entry) -> Option<Status> {
    git?.status_of(&entry.path)
}

fn status_style(status: Status) -> Style {
    let color = match status {
        Status::Modified => Color::Yellow,
        Status::Added => Color::Green,
        Status::Renamed => Color::Cyan,
        Status::Deleted | Status::Conflicted => Color::Red,
        Status::Untracked => Color::LightRed,
        Status::Ignored => Color::DarkGray,
    };
    Style::new().fg(color)
}

fn name_style(entry: &Entry, status: Option<Status>) -> Style {
    let style = match (entry.is_symlink, entry.is_dir) {
        (true, true) => Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        (true, false) => Style::new().fg(Color::Cyan),
        (false, true) => Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
        (false, false) if entry.is_executable() => Style::new().fg(Color::Green),
        (false, false) => Style::new(),
    };
    if entry.is_hidden() || status == Some(Status::Ignored) {
        style.add_modifier(Modifier::DIM)
    } else {
        style
    }
}

/// One row: mark, git status, name (truncated, match positions highlighted), size flush right.
fn entry_line(
    entry: &Entry,
    hits: &[usize],
    status: Option<Status>,
    marked: bool,
    width: usize,
    with_size: bool,
    icons: bool,
) -> Line<'static> {
    let mut spans = vec![
        if marked {
            Span::styled("▍", Style::new().fg(Color::Magenta))
        } else {
            Span::raw(" ")
        },
        match status {
            Some(s) => Span::styled(s.symbol().to_string(), status_style(s)),
            None => Span::raw(" "),
        },
        Span::raw(" "),
    ];
    let size = if with_size && !entry.is_dir {
        dir::human_size(entry.size)
    } else {
        String::new()
    };
    let base = name_style(entry, status);
    // The glyph plus a space: Nerd Font icons are drawn wider than a cell and need the room.
    let icon_width = if icons { 2 } else { 0 };
    if icons {
        let icon = icons::for_entry(entry);
        let style = Style::new().fg(icon.color).add_modifier(base.add_modifier);
        spans.push(Span::styled(format!("{} ", icon.glyph), style));
    }
    let gutter = 3 + icon_width;
    let reserved = gutter + if size.is_empty() { 0 } else { size.len() + 1 };
    let room = width.saturating_sub(reserved);
    let mut name = entry.name.clone();
    if entry.is_dir {
        name.push('/');
    }
    let hit = base.fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let used = push_name(&mut spans, &name, hits, base, hit, room);
    if !size.is_empty() {
        let pad = width.saturating_sub(gutter + used + size.len());
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(size, DIM));
    }
    Line::from(spans)
}

/// Pushes `name` cut to `room` columns, styling the characters at `hits`. Returns columns used.
fn push_name(
    spans: &mut Vec<Span<'static>>,
    name: &str,
    hits: &[usize],
    base: Style,
    hit: Style,
    room: usize,
) -> usize {
    let total = name.width();
    let limit = if total > room {
        room.saturating_sub(1)
    } else {
        room
    };
    let mut hits = hits.iter().peekable();
    let (mut used, mut run, mut run_hit) = (0, String::new(), false);
    for (i, c) in name.chars().enumerate() {
        let c = if c.is_control() { '?' } else { c };
        let w = c.width().unwrap_or(0);
        if used + w > limit {
            break;
        }
        let is_hit = hits.next_if(|&&h| h == i).is_some();
        if is_hit != run_hit && !run.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                if run_hit { hit } else { base },
            ));
        }
        run_hit = is_hit;
        run.push(c);
        used += w;
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if run_hit { hit } else { base }));
    }
    if total > room && room > 0 {
        spans.push(Span::styled("…", base));
        used += 1;
    }
    used
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let (label, color) = match &app.mode {
        Mode::Normal => ("NORMAL", ACCENT),
        Mode::Filter => ("FILTER", Color::Yellow),
        Mode::Find(_) => ("FIND", Color::Magenta),
        Mode::Input(_) => ("INPUT", Color::Cyan),
        Mode::Confirm(_) => ("CONFIRM", Color::Red),
        Mode::Goto => ("GO", Color::Cyan),
        Mode::Help => ("HELP", Color::Green),
    };
    let badge = Style::new()
        .fg(Color::Black)
        .bg(color)
        .add_modifier(Modifier::BOLD);
    let mut left = vec![Span::styled(format!(" {label} "), badge), Span::raw(" ")];
    if let Some(e) = app.selected() {
        left.extend(permission_spans(&dir::mode_string(e)));
        if !e.is_dir {
            left.push(Span::raw(format!("  {}", dir::human_size(e.size))));
        }
        if let Some(t) = e.modified {
            left.push(Span::styled(
                format!("  {}", dir::fmt_age(t, SystemTime::now())),
                DIM,
            ));
        }
        if e.is_symlink
            && let Ok(target) = std::fs::read_link(&e.path)
        {
            left.push(Span::styled(
                format!("  → {}", target.display()),
                Style::new().fg(Color::Cyan),
            ));
        }
    }

    let sep = || Span::styled("  ", DIM);
    let mut right = Vec::new();
    if !app.marked.is_empty() {
        right.push(Span::styled(
            format!("▍{} marked", app.marked.len()),
            Style::new().fg(Color::Magenta),
        ));
        right.push(sep());
    }
    if let Some(job) = &app.job {
        right.push(Span::styled(
            format!("pasting {}/{}", job.done, job.total),
            Style::new().fg(Color::Cyan),
        ));
        right.push(sep());
    }
    if let Some(clip) = &app.clip {
        let verb = if clip.cut { "cut" } else { "copied" };
        right.push(Span::styled(
            format!("{} {verb}", clip.paths.len()),
            Style::new().fg(Color::Yellow),
        ));
        right.push(sep());
    }
    if app.show_hidden {
        right.push(Span::styled("hidden", DIM));
        right.push(sep());
    }
    let arrow = if app.reverse { "↑" } else { "↓" };
    right.push(Span::styled(format!("{}{arrow}", app.sort.label()), DIM));
    right.push(sep());
    let position = if app.view.is_empty() {
        0
    } else {
        app.selected + 1
    };
    right.push(Span::raw(format!("{position}/{} ", app.view.len())));

    let right = Line::from(right);
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right.width() as u16)])
        .areas(area);
    frame.render_widget(Paragraph::new(Line::from(left)), l);
    frame.render_widget(Paragraph::new(right), r);
}

/// `drwxr-xr-x` coloured the way `ls` users read it: type, then read, write, execute.
fn permission_spans(mode: &str) -> Vec<Span<'static>> {
    mode.chars()
        .map(|c| {
            let color = match c {
                'd' => ACCENT,
                'l' => Color::Cyan,
                'r' => Color::Yellow,
                'w' => Color::Red,
                'x' => Color::Green,
                _ => Color::DarkGray,
            };
            Span::styled(c.to_string(), Style::new().fg(color))
        })
        .collect()
}

fn draw_command(frame: &mut Frame, app: &App, area: Rect) {
    let prompt = |frame: &mut Frame, label: &str, text: &str, before_cursor: &str| {
        let line = Line::from(vec![
            Span::styled(label.to_string(), Style::new().fg(Color::Cyan)),
            Span::raw(text.to_string()),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        let x = area.x + (label.width() + before_cursor.width()) as u16;
        frame.set_cursor_position((x.min(area.right().saturating_sub(1)), area.y));
    };
    match &app.mode {
        Mode::Filter => prompt(frame, " /", &app.filter, &app.filter),
        Mode::Input(input) => {
            let label = format!(" {}", input.kind.prompt());
            prompt(
                frame,
                &label,
                &input.text,
                &input.text[..input.byte(input.cursor)],
            );
        }
        Mode::Confirm(confirm) => {
            let text = match confirm {
                Confirm::Trash(paths) => {
                    let what = match paths.as_slice() {
                        [one] => one.file_name().map_or_else(
                            || one.display().to_string(),
                            |n| n.to_string_lossy().into_owned(),
                        ),
                        many => format!("{} items", many.len()),
                    };
                    format!(" move {what} to trash? [y/N]")
                }
                Confirm::Quit { .. } => " a paste is still running — quit anyway? [y/N]".into(),
            };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    text,
                    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                )),
                area,
            );
        }
        Mode::Find(_) => {
            let text = " find: ↑ ↓ move · enter go there · esc cancel";
            frame.render_widget(Paragraph::new(Span::styled(text, DIM)), area);
        }
        Mode::Goto => {
            let text = " g: g top · h home · r git root · / root";
            frame.render_widget(Paragraph::new(Span::styled(text, DIM)), area);
        }
        Mode::Normal | Mode::Help => {
            let line = match app.message() {
                Some(m) if m.error => {
                    Span::styled(format!(" ✗ {}", m.text), Style::new().fg(Color::Red))
                }
                Some(m) => Span::raw(format!(" {}", m.text)),
                None => Span::styled(" ? help  f find  / filter  q quit", DIM),
            };
            frame.render_widget(Paragraph::new(line), area);
        }
    }
}

fn draw_help(frame: &mut Frame, area: Rect) {
    // Flow into more columns when the terminal is too short for one.
    let rows = (area.height.saturating_sub(2) as usize).max(1);
    let columns = HELP.len().div_ceil(rows);
    let rows = HELP.len().div_ceil(columns);
    // Each column is as wide as its own contents, so a column of short entries does not
    // take the room a long one needs.
    let widths: Vec<(usize, usize)> = HELP
        .chunks(rows)
        .map(|column| {
            let keys = column.iter().map(|h| match h {
                Help::Key(k, _) => k.width(),
                Help::Section(_) => 0,
            });
            let keys = keys.max().unwrap_or(0);
            let cells = column.iter().map(|h| match h {
                Help::Key(_, d) => keys + d.width() + 4,
                Help::Section(s) => s.width() + 2,
            });
            (keys, cells.max().unwrap_or(0))
        })
        .collect();
    let heading = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
    let lines: Vec<Line> = (0..rows)
        .map(|r| {
            let cells = widths
                .iter()
                .enumerate()
                .filter_map(|(c, &(keys, cell))| Some((HELP.get(c * rows + r)?, keys, cell)));
            let spans = cells.flat_map(|(h, keys, cell)| match h {
                Help::Section(title) => {
                    vec![Span::styled(
                        format!(" {title:<w$} ", w = cell - 2),
                        heading,
                    )]
                }
                Help::Key(k, d) => vec![
                    Span::styled(format!(" {k:<keys$}  "), Style::new().fg(Color::Yellow)),
                    Span::raw(format!("{d:<w$} ", w = cell - keys - 4)),
                ],
            });
            Line::from(spans.collect::<Vec<_>>())
        })
        .collect();
    let width = widths.iter().map(|(_, cell)| cell).sum::<usize>() as u16 + 2;
    let popup = centered(area, width, rows as u16 + 2);
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(popup_block(" keys ")), popup);
}

/// The frame every overlay shares.
fn popup_block(title: &str) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(DIM)
        .title(Span::styled(
            title.to_string(),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
}

fn draw_find(frame: &mut Frame, finder: &Finder, area: Rect, icons: bool) {
    let width = (area.width * 9 / 10).max(20);
    let height = (area.height * 8 / 10).max(5);
    let popup = centered(area, width, height);
    frame.render_widget(Clear, popup);
    let block = popup_block(" find ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.height < 2 {
        return;
    }

    let count = match &finder.index {
        _ if finder.indexing() => "indexing…".to_string(),
        Some(index) => {
            // Only the best FIND_LIMIT are kept, so a full list means there were more.
            let capped = if finder.matches.len() >= FIND_LIMIT {
                "+"
            } else {
                ""
            };
            let cut = if index.truncated { "+" } else { "" };
            format!(
                "{}{capped}/{}{cut}",
                finder.matches.len(),
                index.paths.len()
            )
        }
        None => String::new(),
    };
    let [prompt_area, count_area] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(count.width() as u16 + 1),
    ])
    .areas(Rect { height: 1, ..inner });
    let prompt = Line::from(vec![
        Span::styled(" › ", Style::new().fg(Color::Cyan)),
        Span::raw(finder.query.clone()),
    ]);
    frame.render_widget(Paragraph::new(prompt), prompt_area);
    frame.render_widget(Paragraph::new(Span::styled(count, DIM)), count_area);
    let cursor = prompt_area.x + 3 + finder.query.width() as u16;
    frame.set_cursor_position((cursor.min(prompt_area.right().saturating_sub(1)), inner.y));

    let Some(index) = &finder.index else {
        return;
    };
    let rows = Rect {
        y: inner.y + 1,
        height: inner.height - 1,
        ..inner
    };
    let height = rows.height as usize;
    // Keep the selection in view; the list is short enough to recompute every frame.
    let offset = finder.selected.saturating_sub(height.saturating_sub(1));
    let lines: Vec<Line> = finder
        .matches
        .iter()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(i, m)| {
            let line = find_line(&index.paths[m.idx], &m.hits, rows.width as usize, icons);
            if i == finder.selected {
                line.patch_style(SELECTED)
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), rows);
}

/// A found path: its directory dimmed, the name bright, matches highlighted, and cut from the
/// left when too long, since the end of a path is the part that tells files apart.
fn find_line(path: &str, hits: &[usize], width: usize, icons: bool) -> Line<'static> {
    let chars: Vec<char> = path.chars().collect();
    let is_dir = path.ends_with('/');
    let name_start = chars[..chars.len().saturating_sub(1)]
        .iter()
        .rposition(|&c| c == '/')
        .map_or(0, |i| i + 1);
    // One column goes to the leading space, two more to an icon and its gap.
    let room = width.saturating_sub(if icons { 3 } else { 1 });
    let mut start = 0;
    let mut used: usize = chars.iter().map(|c| c.width().unwrap_or(0)).sum();
    if used > room {
        // Leave a column for the ellipsis.
        while start < chars.len() && used > room.saturating_sub(1) {
            used -= chars[start].width().unwrap_or(0);
            start += 1;
        }
    }

    let hit = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let name = if is_dir {
        Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    let mut spans = vec![Span::raw(" ")];
    if icons {
        let name = path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(path);
        let kind = if is_dir {
            icons::Kind::Dir
        } else {
            icons::Kind::File
        };
        let icon = icons::for_name(name, kind);
        spans.push(Span::styled(
            format!("{} ", icon.glyph),
            Style::new().fg(icon.color),
        ));
    }
    if start > 0 {
        spans.push(Span::styled("…", DIM));
    }
    let mut hits = hits.iter().peekable();
    while hits.next_if(|&&h| h < start).is_some() {}
    let (mut run, mut run_style) = (String::new(), None);
    for (i, &c) in chars.iter().enumerate().skip(start) {
        let style = if hits.next_if(|&&h| h == i).is_some() {
            hit
        } else if i < name_start {
            DIM
        } else {
            name
        };
        if run_style.is_some_and(|s| s != style) {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                run_style.unwrap_or_default(),
            ));
        }
        run_style = Some(style);
        run.push(if c.is_control() { '?' } else { c });
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, run_style.unwrap_or_default()));
    }
    Line::from(spans)
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let (width, height) = (width.min(area.width), height.min(area.height));
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dir::testutil::TempDir;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
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
        let body = rows[1..rows.len() - 2]
            .iter()
            .map(|r| r.chars().skip(parent).collect::<String>());
        let snapshot: Vec<String> = body
            .chain(rows[rows.len() - 2..].iter().map(|r| r.to_string()))
            .collect();
        insta::assert_snapshot!(snapshot.join("\n"));
        assert_eq!(app.page, 5);
    }

    #[test]
    fn long_names_truncate_with_an_ellipsis() {
        let tmp = TempDir::new();
        tmp.file("a-very-long-file-name-that-will-not-fit.txt", "");
        let e = Entry::from_path(
            tmp.path()
                .join("a-very-long-file-name-that-will-not-fit.txt"),
        )
        .unwrap();
        let line = entry_line(&e, &[], None, false, 20, true, false);
        assert_eq!(line.width(), 20);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains('…'), "{text}");
        assert!(text.ends_with("0 B"), "{text}");
    }

    #[test]
    fn find_lines_cut_from_the_left() {
        let text = |line: Line| {
            line.spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };
        assert_eq!(
            text(find_line("src/app.rs", &[4], 40, false)),
            " src/app.rs"
        );
        let long = find_line("a/very/deeply/nested/path/to/main.rs", &[], 16, false);
        assert_eq!(text(long.clone()), " …ath/to/main.rs");
        assert_eq!(long.width(), 16);
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

    #[test]
    fn permissions_are_coloured_by_meaning() {
        let spans = permission_spans("drwxr-x---");
        let color = |i: usize| spans[i].style.fg;
        assert_eq!(color(0), Some(ACCENT));
        assert_eq!(color(1), Some(Color::Yellow));
        assert_eq!(color(2), Some(Color::Red));
        assert_eq!(color(3), Some(Color::Green));
        assert_eq!(color(9), Some(Color::DarkGray));
    }
}
