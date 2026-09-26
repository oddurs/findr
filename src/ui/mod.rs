//! Drawing. Reads `App`; writes back only the list viewport and page size.

use std::env;
use std::path::Path;
use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Areas, Confirm, Mode, parent_offset};
use crate::dir;
use crate::icons;
use crate::preview::Preview;

mod overlays;
mod rows;

use overlays::{draw_find, draw_help};
use rows::{entry_line, name_style, status};

const SELECTED: Style = Style::new()
    .bg(Color::Indexed(237))
    .add_modifier(Modifier::BOLD);

const PARENT_SELECTED: Style = Style::new().bg(Color::Indexed(236));

const DIM: Style = Style::new().fg(Color::DarkGray);

const ACCENT: Color = Color::Blue;

/// Below this width the parent column goes; below the next, the preview does too.
const WITH_PARENT: u16 = 90;

const WITH_PREVIEW: u16 = 60;

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

#[cfg(test)]
mod tests {
    use super::overlays::{HELP, Help};
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
