//! Overlays drawn above the columns: the key help and the finder.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::rows::find_line;
use super::{ACCENT, DIM, SELECTED};
use crate::app::{FIND_LIMIT, Finder};

pub(super) enum Help {
    Section(&'static str),
    Key(&'static str, &'static str),
}

pub(super) const HELP: &[Help] = &[
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

pub(super) fn draw_help(frame: &mut Frame, area: Rect) {
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
pub(super) fn popup_block(title: &str) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(DIM)
        .title(Span::styled(
            title.to_string(),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
}

pub(super) fn draw_find(frame: &mut Frame, finder: &Finder, area: Rect, icons: bool) {
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

pub(super) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let (width, height) = (width.min(area.width), height.min(area.height));
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}
