//! The pieces every view is built from, as listed in `docs/design.md`. Each takes the theme and
//! returns spans or a block; none knows which region it will be drawn in.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;

use super::theme::{Theme, Tone};

/// A piece of pending state on the bottom bar.
pub fn chip(text: String, tone: Tone, theme: &Theme) -> Span<'static> {
    Span::styled(text, Style::new().fg(theme.tone(tone)))
}

/// Keys and what they do: the key bold in the normal colour, its label muted, pairs spaced
/// apart so each reads as one unit.
pub fn hints(pairs: &[(&str, String)], theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, (key, label)) in pairs.iter().enumerate() {
        spans.push(Span::raw(if i == 0 { "" } else { "   " }));
        spans.push(Span::styled(
            key.to_string(),
            Style::new().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}"), theme.muted()));
    }
    spans
}

/// Facts about one thing, muted and separated by `·`. Empty facts are skipped.
pub fn facts(items: &[String], theme: &Theme) -> Vec<Span<'static>> {
    let items: Vec<&String> = items.iter().filter(|s| !s.is_empty()).collect();
    let mut spans = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                format!(" {} ", theme.glyphs.separator),
                theme.faint(),
            ));
        }
        spans.push(Span::styled(item.to_string(), theme.muted()));
    }
    spans
}

/// A label for text input: `rename ›`.
pub fn prompt_label(label: &str, theme: &Theme) -> Span<'static> {
    let prompt = theme.glyphs.prompt;
    Span::styled(format!(" {label} {prompt} "), Style::new().fg(theme.accent))
}

/// An outcome: a mark and the text in the tone of how it went.
pub fn message(text: &str, tone: Tone, theme: &Theme) -> Line<'static> {
    let mark = match tone {
        Tone::Success => format!("{} ", theme.glyphs.success),
        Tone::Danger => format!("{} ", theme.glyphs.failure),
        _ => String::new(),
    };
    let color = match tone {
        Tone::Success | Tone::Danger => Style::new().fg(theme.tone(tone)),
        _ => Style::new(),
    };
    Line::from(Span::styled(format!(" {mark}{text}"), color))
}

/// A muted sentence for when there is nothing to show.
pub fn empty(text: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(format!("  {text}"), theme.muted()))
}

/// The frame every overlay shares: rounded, faint, with an accent title.
pub fn panel(title: &str, theme: &Theme) -> Block<'static> {
    Block::bordered()
        .border_set(theme.glyphs.border)
        .border_style(theme.faint())
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ))
}

#[cfg(test)]
mod tests {
    use super::super::glyphs::Glyphs;
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn hints_read_as_key_then_label() {
        let spans = hints(
            &[("y", "copy".into()), ("esc", "unmark".into())],
            &Theme::DARK,
        );
        assert_eq!(text(&spans), "y copy   esc unmark");
        assert!(spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[2].style.fg, Some(Theme::DARK.muted));
    }

    #[test]
    fn facts_skip_what_is_unknown() {
        let spans = facts(&["Rust".into(), String::new(), "12 K".into()], &Theme::DARK);
        assert_eq!(text(&spans), "Rust · 12 K");
    }

    #[test]
    fn messages_mark_success_and_failure_in_every_tier() {
        for glyphs in [Glyphs::NERD, Glyphs::UNICODE, Glyphs::ASCII] {
            let t = Theme {
                glyphs,
                ..Theme::DARK
            };
            let line = |tone| message("done", tone, &t).to_string();
            assert_eq!(line(Tone::Success), format!(" {} done", glyphs.success));
            assert_eq!(line(Tone::Danger), format!(" {} done", glyphs.failure));
            assert_eq!(line(Tone::Muted), " done");
        }
    }
}
