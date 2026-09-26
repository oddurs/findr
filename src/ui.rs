//! Drawing. Reads `App`; writes back only the list viewport and page size.

use std::env;
use std::path::Path;
use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Areas, Confirm, FIND_LIMIT, Finder, Mode, parent_offset};
use crate::dir::{self, Entry};
use crate::git::{Repo, Status};
use crate::icons;
use crate::preview::Preview;

const SELECTED: Style = Style::new()
    .bg(Color::Indexed(238))
    .add_modifier(Modifier::BOLD);
const PARENT_SELECTED: Style = Style::new().bg(Color::Indexed(236));
const DIM: Style = Style::new().fg(Color::DarkGray);

const HELP: &[(&str, &str)] = &[
    ("j k  ↑ ↓", "move"),
    ("h l  ← →", "parent / open"),
    ("enter", "enter directory or edit file"),
    ("gg G  ^d ^u", "top, bottom, half page"),
    ("gh gr g/", "home, git root, filesystem root"),
    ("-", "previous directory"),
    (":", "go to a path"),
    ("/", "fuzzy filter"),
    ("f ^p", "find below (honours .gitignore)"),
    ("esc", "clear filter, then marks"),
    ("space", "mark"),
    ("y x p", "copy, cut, paste"),
    ("d", "move to trash"),
    ("r", "rename"),
    ("a A", "new file (a/b.rs makes dirs), new dir"),
    ("e", "edit in $EDITOR"),
    ("o", "open with the default app"),
    ("c", "copy path to clipboard"),
    ("!", "shell here"),
    ("J K", "scroll preview"),
    (".", "show hidden files"),
    ("s S", "cycle sort, reverse"),
    ("R", "reload"),
    ("q Q", "quit and cd, quit"),
    ("mouse", "wheel, click, double-click to open"),
];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, status, command] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [parent, current, preview] = Layout::horizontal([
        Constraint::Percentage(20),
        Constraint::Percentage(35),
        Constraint::Percentage(45),
    ])
    .areas(body);

    let current = column(frame, current);
    let preview = column(frame, preview);
    let preview = Rect {
        x: preview.x + 1,
        width: preview.width.saturating_sub(1),
        ..preview
    };
    // Recorded so a mouse click can be mapped back to the row under it.
    app.areas = Areas {
        parent,
        current,
        preview,
    };

    draw_header(frame, app, header);
    draw_parent(frame, app, parent);
    draw_current(frame, app, current);
    draw_preview(frame, app, preview);
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
    let mut spans = vec![Span::styled(
        format!(" {}", tildify(&app.cwd)),
        Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
    )];
    if let Some(repo) = &app.git {
        let branch = repo.branch.as_deref().unwrap_or("?");
        spans.push(Span::styled(
            format!("  {branch}"),
            Style::new().fg(Color::Magenta),
        ));
        if repo.is_dirty() {
            spans.push(Span::styled("*", Style::new().fg(Color::Yellow)));
        }
    }
    if !app.filter.is_empty() && !matches!(app.mode, Mode::Filter) {
        spans.push(Span::styled(
            format!("  /{}", app.filter),
            Style::new().fg(Color::Yellow),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
            " empty"
        } else {
            " no matches"
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

    let width = inner.width as usize;
    let lines: Vec<Line> = app.view[app.offset..]
        .iter()
        .take(height)
        .enumerate()
        .map(|(i, v)| {
            let e = &app.entries[v.idx];
            let line = entry_line(
                e,
                &v.hits,
                status(app.git.as_ref(), e),
                app.marked.contains(&e.path),
                width,
                true,
                app.icons,
            );
            if app.offset + i == app.selected {
                line.patch_style(SELECTED)
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
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
        Preview::Dir(entries) if entries.is_empty() => vec![Line::styled("empty", DIM)],
        Preview::Dir(entries) => entries
            .iter()
            .skip(app.preview_scroll)
            .take(height)
            .map(|e| {
                entry_line(
                    e,
                    &[],
                    status(app.git.as_ref(), e),
                    false,
                    inner.width as usize,
                    true,
                    app.icons,
                )
            })
            .collect(),
        Preview::Text(lines) => lines
            .iter()
            .skip(app.preview_scroll)
            .take(height)
            .cloned()
            .collect(),
        Preview::Note(note) => vec![Line::styled(note.clone(), DIM)],
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
    let mut left = String::from(" ");
    if let Some(e) = app.selected() {
        left.push_str(&dir::mode_string(e));
        if !e.is_dir {
            left.push_str(&format!("  {}", dir::human_size(e.size)));
        }
        if let Some(t) = e.modified {
            left.push_str(&format!("  {}", dir::fmt_age(t, SystemTime::now())));
        }
        if e.is_symlink
            && let Ok(target) = std::fs::read_link(&e.path)
        {
            left.push_str(&format!("  → {}", target.display()));
        }
    }

    let mut right = Vec::new();
    if !app.marked.is_empty() {
        right.push(format!("{} marked", app.marked.len()));
    }
    if let Some(job) = &app.job {
        right.push(format!("pasting {}/{}", job.done, job.total));
    }
    if let Some(clip) = &app.clip {
        right.push(format!(
            "{} {}",
            clip.paths.len(),
            if clip.cut { "cut" } else { "copied" }
        ));
    }
    if app.show_hidden {
        right.push("hidden".into());
    }
    right.push(format!(
        "{}{}",
        app.sort.label(),
        if app.reverse { "↑" } else { "↓" }
    ));
    let position = if app.view.is_empty() {
        0
    } else {
        app.selected + 1
    };
    right.push(format!("{position}/{}", app.view.len()));
    let right = format!("{} ", right.join("  "));

    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(right.width() as u16)])
        .areas(area);
    frame.render_widget(Paragraph::new(Span::styled(left, DIM)), l);
    frame.render_widget(Paragraph::new(Span::styled(right, DIM)), r);
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
                    Span::styled(format!(" {}", m.text), Style::new().fg(Color::Red))
                }
                Some(m) => Span::raw(format!(" {}", m.text)),
                None => Span::styled(" ? help", DIM),
            };
            frame.render_widget(Paragraph::new(line), area);
        }
    }
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let keys_width = HELP.iter().map(|(k, _)| k.width()).max().unwrap_or(0);
    let desc_width = HELP.iter().map(|(_, d)| d.width()).max().unwrap_or(0);
    // Flow into more columns when the terminal is too short for one.
    let rows = (area.height.saturating_sub(2) as usize).max(1);
    let columns = HELP.len().div_ceil(rows);
    let rows = HELP.len().div_ceil(columns);
    let lines: Vec<Line> = (0..rows)
        .map(|r| {
            let cells = (0..columns).filter_map(|c| HELP.get(c * rows + r));
            let spans = cells.flat_map(|(k, d)| {
                [
                    Span::styled(
                        format!(" {k:<keys_width$}  "),
                        Style::new().fg(Color::Yellow),
                    ),
                    Span::raw(format!("{d:<desc_width$} ")),
                ]
            });
            Line::from(spans.collect::<Vec<_>>())
        })
        .collect();
    let width = (columns * (keys_width + desc_width + 4)) as u16 + 2;
    let popup = centered(area, width, rows as u16 + 2);
    frame.render_widget(Clear, popup);
    let block = Block::bordered().title(" findr ").border_style(DIM);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

fn draw_find(frame: &mut Frame, finder: &Finder, area: Rect, icons: bool) {
    let width = (area.width * 9 / 10).max(20);
    let height = (area.height * 8 / 10).max(5);
    let popup = centered(area, width, height);
    frame.render_widget(Clear, popup);
    let block = Block::bordered().title(" find ").border_style(DIM);
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

        let screen = render(&mut app, 80, 8);
        let rows: Vec<&str> = screen.lines().collect();
        // The header holds the temp path and the parent column lists whatever else is in the
        // temp dir, so the snapshot takes the body right of the parent column plus the bottom bars.
        let body = rows[1..rows.len() - 2]
            .iter()
            .map(|r| r.chars().skip(16).collect::<String>());
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
        for (width, height) in [(120, 40), (120, 24), (220, 12)] {
            let screen = render(&mut app, width, height);
            for (_, desc) in HELP {
                assert!(
                    screen.contains(desc),
                    "{desc:?} missing at height {height}:\n{screen}"
                );
            }
        }
    }
}
