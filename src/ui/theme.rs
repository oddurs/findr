//! Colour roles, as tabled in `docs/design.md`. Views ask for a role, never a colour, so each
//! colour means one thing and a palette can change in one place.
//!
//! Roles map to named ANSI colours where they can, so findr follows the terminal's own scheme;
//! only the surfaces are fixed greys, and those differ between the dark and light palettes.

use ratatui::style::{Color, Modifier, Style};

use crate::git::Status;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Focus: the cursor bar, the active mode, headings.
    pub accent: Color,
    pub directory: Color,
    pub link: Color,
    pub executable: Color,
    /// Characters a filter or find query matched.
    pub matched: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub info: Color,
    /// Untracked files: danger, but lighter, since they are new rather than wrong.
    pub untracked: Color,
    pub mark: Color,
    pub branch: Color,
    /// Secondary text: sizes, ages, facts, hints' labels.
    pub muted: Color,
    /// Structure: column rules, borders, the scrollbar.
    pub faint: Color,
    /// Text on a badge.
    pub on_badge: Color,
    pub selected_bg: Color,
    pub context_bg: Color,
    /// Faint washes behind added and removed lines of a diff, so the code keeps its colours.
    pub added_bg: Color,
    pub removed_bg: Color,
}

/// The tones a badge, chip or message can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Accent,
    Success,
    Warning,
    Danger,
    Info,
    Mark,
    Muted,
}

impl Theme {
    pub const DARK: Theme = Theme {
        accent: Color::Blue,
        directory: Color::Blue,
        link: Color::Cyan,
        executable: Color::Green,
        matched: Color::Yellow,
        success: Color::Green,
        warning: Color::Yellow,
        danger: Color::Red,
        info: Color::Cyan,
        untracked: Color::LightRed,
        mark: Color::Magenta,
        branch: Color::Magenta,
        muted: Color::DarkGray,
        faint: Color::Indexed(238),
        on_badge: Color::Black,
        selected_bg: Color::Indexed(237),
        context_bg: Color::Indexed(236),
        added_bg: Color::Indexed(22),
        removed_bg: Color::Indexed(52),
    };

    pub const LIGHT: Theme = Theme {
        muted: Color::Indexed(244),
        faint: Color::Indexed(250),
        on_badge: Color::White,
        selected_bg: Color::Indexed(253),
        context_bg: Color::Indexed(254),
        added_bg: Color::Indexed(194),
        removed_bg: Color::Indexed(224),
        ..Theme::DARK
    };

    pub fn tone(&self, tone: Tone) -> Color {
        match tone {
            Tone::Accent => self.accent,
            Tone::Success => self.success,
            Tone::Warning => self.warning,
            Tone::Danger => self.danger,
            Tone::Info => self.info,
            Tone::Mark => self.mark,
            Tone::Muted => self.muted,
        }
    }

    pub fn muted(&self) -> Style {
        Style::new().fg(self.muted)
    }

    pub fn faint(&self) -> Style {
        Style::new().fg(self.faint)
    }

    pub fn selected(&self) -> Style {
        Style::new()
            .bg(self.selected_bg)
            .add_modifier(Modifier::BOLD)
    }

    pub fn context_selected(&self) -> Style {
        Style::new().bg(self.context_bg)
    }

    pub fn matched(&self) -> Style {
        Style::new().fg(self.matched).add_modifier(Modifier::BOLD)
    }

    /// Git status on the semantic roles: what changed is a warning, what is new a success,
    /// what is gone or conflicted danger.
    pub fn git(&self, status: Status) -> Style {
        let color = match status {
            Status::Modified => self.warning,
            Status::Added => self.success,
            Status::Renamed => self.info,
            Status::Deleted | Status::Conflicted => self.danger,
            Status::Untracked => self.untracked,
            Status::Ignored => self.muted,
        };
        Style::new().fg(color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_changes_only_the_greys() {
        let (dark, light) = (Theme::DARK, Theme::LIGHT);
        assert_eq!(
            light.accent, dark.accent,
            "named colours follow the terminal's palette"
        );
        assert_ne!(light.selected_bg, dark.selected_bg);
        assert_ne!(light.muted, dark.muted);
    }

    /// "One meaning per colour" only holds while every colour comes from here.
    #[test]
    fn views_name_roles_not_colours() {
        let views = [
            ("mod.rs", include_str!("mod.rs")),
            ("rows.rs", include_str!("rows.rs")),
            ("overlays.rs", include_str!("overlays.rs")),
            ("components.rs", include_str!("components.rs")),
        ];
        for (file, source) in views {
            assert!(
                !source.contains(concat!("Color", "::")),
                "src/ui/{file} names a colour; use a role from the theme"
            );
        }
    }

    #[test]
    fn git_statuses_use_the_semantic_roles() {
        let t = Theme::DARK;
        assert_eq!(t.git(Status::Modified).fg, Some(t.warning));
        assert_eq!(t.git(Status::Added).fg, Some(t.success));
        assert_eq!(t.git(Status::Conflicted).fg, Some(t.danger));
        assert_eq!(t.git(Status::Ignored).fg, Some(t.muted));
    }
}
