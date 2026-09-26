//! The row component: one entry or one found path. Every listing — context, listing, inspector,
//! find — is drawn from these, so an entry looks the same wherever it appears.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::theme::Theme;
use crate::dir::{self, Entry};
use crate::git::{Repo, Status};
use crate::icons;

/// How a row is drawn; the same entry reads differently in each column.
pub(super) struct Row<'a> {
    pub theme: &'a Theme,
    pub width: usize,
    pub size: bool,
    /// Context rows: names and icons muted so they do not compete with the listing.
    pub muted: bool,
}

pub(super) fn status(git: Option<&Repo>, entry: &Entry) -> Option<Status> {
    git?.status_of(&entry.path)
}

pub(super) fn name_style(entry: &Entry, status: Option<Status>, theme: &Theme) -> Style {
    let style = match (entry.is_symlink, entry.is_dir) {
        (true, true) => Style::new().fg(theme.link).add_modifier(Modifier::BOLD),
        (true, false) => Style::new().fg(theme.link),
        (false, true) => Style::new()
            .fg(theme.directory)
            .add_modifier(Modifier::BOLD),
        (false, false) if entry.is_executable() => Style::new().fg(theme.executable),
        (false, false) => Style::new(),
    };
    if entry.is_hidden() || status == Some(Status::Ignored) {
        style.add_modifier(Modifier::DIM)
    } else {
        style
    }
}

/// Mark, git status, icon, name (truncated, match positions highlighted), size flush right.
pub(super) fn entry_line(
    entry: &Entry,
    hits: &[usize],
    status: Option<Status>,
    marked: bool,
    row: &Row,
) -> Line<'static> {
    let theme = row.theme;
    let mut spans = vec![
        if marked {
            Span::styled(theme.glyphs.mark, Style::new().fg(theme.mark))
        } else {
            Span::raw(" ")
        },
        match status {
            Some(s) => Span::styled(s.symbol().to_string(), theme.git(s)),
            None => Span::raw(" "),
        },
        Span::raw(" "),
    ];
    let size = if row.size && !entry.is_dir {
        dir::human_size(entry.size)
    } else {
        String::new()
    };
    let base = if row.muted {
        theme.muted()
    } else {
        name_style(entry, status, theme)
    };
    // The glyph plus a space: Nerd Font icons are drawn wider than a cell and need the room.
    let icons = theme.glyphs.icons();
    let icon_width = if icons { 2 } else { 0 };
    if icons {
        let icon = icons::for_entry(entry);
        let color = if row.muted { theme.muted } else { icon.color };
        let style = Style::new().fg(color).add_modifier(base.add_modifier);
        spans.push(Span::styled(format!("{} ", icon.glyph), style));
    }
    let gutter = 3 + icon_width;
    let reserved = gutter + if size.is_empty() { 0 } else { size.len() + 1 };
    let room = row.width.saturating_sub(reserved);
    let mut name = entry.name.clone();
    if entry.is_dir {
        name.push('/');
    }
    let used = push_name(
        &mut spans,
        &name,
        hits,
        (base, theme.matched()),
        room,
        theme.glyphs.ellipsis,
    );
    if !size.is_empty() {
        let pad = row.width.saturating_sub(gutter + used + size.len());
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(size, theme.muted()));
    }
    Line::from(spans)
}

/// Pushes `name` cut to `room` columns, styling the characters at `hits`. Returns columns used.
fn push_name(
    spans: &mut Vec<Span<'static>>,
    name: &str,
    hits: &[usize],
    (base, hit): (Style, Style),
    room: usize,
    ellipsis: &'static str,
) -> usize {
    let total = name.width();
    let limit = if total > room {
        room.saturating_sub(ellipsis.width())
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
            let style = if run_hit { hit } else { base };
            spans.push(Span::styled(std::mem::take(&mut run), style));
        }
        run_hit = is_hit;
        run.push(c);
        used += w;
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if run_hit { hit } else { base }));
    }
    if total > room && room > 0 {
        spans.push(Span::styled(ellipsis, base));
        used += ellipsis.width();
    }
    used
}

/// A found path: its directory muted, the name bright, matches highlighted, and cut from the
/// left when too long, since the end of a path is the part that tells files apart.
pub(super) fn find_line(path: &str, hits: &[usize], row: &Row) -> Line<'static> {
    let theme = row.theme;
    let chars: Vec<char> = path.chars().collect();
    let is_dir = path.ends_with('/');
    let name_start = chars[..chars.len().saturating_sub(1)]
        .iter()
        .rposition(|&c| c == '/')
        .map_or(0, |i| i + 1);
    // One column goes to the leading space, two more to an icon and its gap.
    let icons = theme.glyphs.icons();
    let room = row.width.saturating_sub(if icons { 3 } else { 1 });
    let mut start = 0;
    let mut used: usize = chars.iter().map(|c| c.width().unwrap_or(0)).sum();
    if used > room {
        // Leave a column for the ellipsis.
        while start < chars.len() && used > room.saturating_sub(1) {
            used -= chars[start].width().unwrap_or(0);
            start += 1;
        }
    }

    let name = if is_dir {
        Style::new()
            .fg(theme.directory)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    let mut spans = vec![Span::raw(" ")];
    if icons {
        let base = path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(path);
        let kind = if is_dir {
            icons::Kind::Dir
        } else {
            icons::Kind::File
        };
        let icon = icons::for_name(base, kind);
        spans.push(Span::styled(
            format!("{} ", icon.glyph),
            Style::new().fg(icon.color),
        ));
    }
    if start > 0 {
        spans.push(Span::styled(theme.glyphs.ellipsis, theme.muted()));
    }
    let mut hits = hits.iter().peekable();
    while hits.next_if(|&&h| h < start).is_some() {}
    let (mut run, mut run_style) = (String::new(), None);
    for (i, &c) in chars.iter().enumerate().skip(start) {
        let style = if hits.next_if(|&&h| h == i).is_some() {
            theme.matched()
        } else if i < name_start {
            theme.muted()
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

    const PLAIN: Theme = Theme {
        glyphs: super::super::glyphs::Glyphs::UNICODE,
        ..Theme::DARK
    };

    fn row(width: usize, icons: bool, muted: bool) -> Row<'static> {
        Row {
            theme: if icons { &Theme::DARK } else { &PLAIN },
            width,
            size: true,
            muted,
        }
    }

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn long_names_truncate_with_an_ellipsis() {
        let tmp = TempDir::new();
        let path = tmp.file("a-very-long-file-name-that-will-not-fit.txt", "");
        let e = Entry::from_path(path).unwrap();
        let line = entry_line(&e, &[], None, false, &row(20, false, false));
        assert_eq!(line.width(), 20);
        let ellipsis = PLAIN.glyphs.ellipsis;
        assert!(text(&line).contains(ellipsis), "{}", text(&line));
        assert!(text(&line).ends_with("0 B"), "{}", text(&line));
    }

    #[test]
    fn context_rows_are_muted_but_keep_git_colour() {
        let tmp = TempDir::new();
        let e = Entry::from_path(tmp.path().to_path_buf()).unwrap();
        let t = Theme::DARK;
        let line = entry_line(&e, &[], Some(Status::Modified), false, &row(30, true, true));
        assert_eq!(line.spans[1].style.fg, Some(t.warning), "status stays loud");
        assert_eq!(line.spans[3].style.fg, Some(t.muted), "icon quiet");
        assert_eq!(line.spans[4].style.fg, Some(t.muted), "name quiet");
        let listing = entry_line(&e, &[], None, false, &row(30, true, false));
        assert_eq!(listing.spans[4].style.fg, Some(t.directory));
    }

    #[test]
    fn find_lines_cut_from_the_left() {
        assert_eq!(
            text(&find_line("src/app.rs", &[4], &row(40, false, false))),
            " src/app.rs"
        );
        let long = find_line(
            "a/very/deeply/nested/path/to/main.rs",
            &[],
            &row(16, false, false),
        );
        assert_eq!(
            text(&long),
            format!(" {}ath/to/main.rs", PLAIN.glyphs.ellipsis)
        );
        assert_eq!(long.width(), 16);
    }
}
