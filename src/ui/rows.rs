//! One line per entry: the rows every listing, the preview and find are drawn from.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::DIM;
use crate::dir::{self, Entry};
use crate::git::{Repo, Status};
use crate::icons;

pub(super) fn status(git: Option<&Repo>, entry: &Entry) -> Option<Status> {
    git?.status_of(&entry.path)
}

pub(super) fn status_style(status: Status) -> Style {
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

pub(super) fn name_style(entry: &Entry, status: Option<Status>) -> Style {
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
pub(super) fn entry_line(
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
pub(super) fn push_name(
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

/// A found path: its directory dimmed, the name bright, matches highlighted, and cut from the
/// left when too long, since the end of a path is the part that tells files apart.
pub(super) fn find_line(path: &str, hits: &[usize], width: usize, icons: bool) -> Line<'static> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dir::testutil::TempDir;

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
}
