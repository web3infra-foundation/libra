//! Shared semantic theme for the TUI.

use ratatui::style::{Color, Modifier, Style};

const TEXT_PRIMARY: Color = Color::Rgb(220, 226, 244);
const TEXT_MUTED: Color = Color::Rgb(126, 152, 176);
const TEXT_SUBTLE: Color = Color::Rgb(95, 108, 130);

const ACCENT_PRIMARY: Color = Color::Rgb(116, 145, 199);
const ACCENT_EXPLORE: Color = Color::Rgb(128, 154, 194);
const ACCENT_EDIT: Color = Color::Rgb(176, 156, 98);
const ACCENT_SHELL: Color = Color::Rgb(102, 146, 102);
const ACCENT_INPUT: Color = Color::Rgb(152, 124, 152);
const ACCENT_DRAFT: Color = Color::Rgb(98, 146, 152);
const ACCENT_BADGE: Color = Color::Rgb(188, 208, 255);

const STATUS_SUCCESS: Color = Color::Rgb(96, 136, 96);
const STATUS_DANGER: Color = Color::Rgb(148, 102, 102);

const ACTIVE_GRADIENT: [Color; 5] = [
    Color::Rgb(76, 108, 152),
    Color::Rgb(84, 124, 160),
    Color::Rgb(156, 168, 188),
    Color::Rgb(84, 124, 160),
    Color::Rgb(76, 108, 152),
];

const EXECUTING_GRADIENT: [Color; 5] = [
    Color::Rgb(76, 108, 152),
    Color::Rgb(98, 146, 152),
    Color::Rgb(156, 168, 188),
    Color::Rgb(98, 146, 152),
    Color::Rgb(76, 108, 152),
];

const WELCOME_GRADIENT: [Color; 6] = [
    ACCENT_PRIMARY,
    ACCENT_EXPLORE,
    ACCENT_BADGE,
    TEXT_PRIMARY,
    ACCENT_DRAFT,
    ACCENT_PRIMARY,
];

pub(crate) mod text {
    use super::*;

    pub(crate) fn primary() -> Style {
        Style::default().fg(TEXT_PRIMARY)
    }

    pub(crate) fn muted() -> Style {
        Style::default().fg(TEXT_MUTED)
    }

    pub(crate) fn subtle() -> Style {
        Style::default().fg(TEXT_SUBTLE)
    }

    pub(crate) fn placeholder() -> Style {
        muted().add_modifier(Modifier::DIM)
    }

    pub(crate) fn help() -> Style {
        subtle().add_modifier(Modifier::DIM)
    }
}

pub(crate) mod border {
    use super::*;

    pub(crate) fn focused() -> Style {
        Style::default().fg(ACCENT_PRIMARY)
    }

    pub(crate) fn idle() -> Style {
        Style::default().fg(TEXT_SUBTLE)
    }
}

pub(crate) mod interactive {
    use super::*;

    pub(crate) fn title() -> Style {
        Style::default()
            .fg(ACCENT_EXPLORE)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn selected_option() -> Style {
        Style::default()
            .fg(ACCENT_PRIMARY)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn in_progress() -> Style {
        Style::default()
            .fg(ACCENT_PRIMARY)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn accent() -> Style {
        Style::default().fg(ACCENT_PRIMARY)
    }
}

pub(crate) mod badge {
    use super::*;

    pub(crate) fn workspace() -> Style {
        Style::default()
            .fg(ACCENT_BADGE)
            .add_modifier(Modifier::DIM)
    }
}

pub(crate) mod tool {
    use super::*;

    pub(crate) fn explore() -> Style {
        Style::default().fg(ACCENT_EXPLORE)
    }

    pub(crate) fn edit() -> Style {
        Style::default().fg(ACCENT_EDIT)
    }

    pub(crate) fn shell() -> Style {
        Style::default().fg(ACCENT_SHELL)
    }

    pub(crate) fn input() -> Style {
        Style::default().fg(ACCENT_INPUT)
    }

    pub(crate) fn draft() -> Style {
        Style::default().fg(ACCENT_DRAFT)
    }
}

pub(crate) mod status {
    use super::*;

    pub(crate) fn success_color() -> Color {
        STATUS_SUCCESS
    }

    pub(crate) fn danger_color() -> Color {
        STATUS_DANGER
    }

    pub(crate) fn success() -> Style {
        Style::default().fg(STATUS_SUCCESS)
    }

    pub(crate) fn danger() -> Style {
        Style::default().fg(STATUS_DANGER)
    }

    pub(crate) fn ready() -> Style {
        success().add_modifier(Modifier::BOLD)
    }

    pub(crate) fn pending_input() -> Style {
        Style::default()
            .fg(ACCENT_INPUT)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn pending_approval() -> Style {
        Style::default()
            .fg(ACCENT_EDIT)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn pending_choice() -> Style {
        Style::default()
            .fg(ACCENT_DRAFT)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn warning() -> Style {
        Style::default().fg(ACCENT_EDIT)
    }

    pub(crate) fn warning_color() -> Color {
        ACCENT_EDIT
    }
}

pub(crate) mod markdown {
    use super::*;

    pub(crate) fn heading_marker() -> Style {
        Style::default().fg(ACCENT_PRIMARY)
    }

    pub(crate) fn code_inline() -> Style {
        Style::default().fg(ACCENT_BADGE)
    }

    pub(crate) fn code_block() -> Style {
        Style::default().fg(ACCENT_EDIT)
    }

    pub(crate) fn link() -> Style {
        Style::default()
            .fg(ACCENT_EXPLORE)
            .add_modifier(Modifier::UNDERLINED)
    }

    pub(crate) fn blockquote() -> Style {
        text::muted()
    }

    pub(crate) fn bullet() -> Style {
        Style::default().fg(ACCENT_PRIMARY)
    }

    pub(crate) fn ordered() -> Style {
        Style::default().fg(ACCENT_PRIMARY)
    }

    pub(crate) fn table_border() -> Style {
        text::subtle()
    }

    pub(crate) fn table_header() -> Style {
        text::primary().add_modifier(Modifier::BOLD)
    }
}

pub(crate) mod diff {
    use super::*;

    pub(crate) fn added_header_color() -> Color {
        STATUS_SUCCESS
    }

    pub(crate) fn removed_header_color() -> Color {
        STATUS_DANGER
    }

    pub(crate) fn updated_header_color() -> Color {
        ACCENT_EDIT
    }

    pub(crate) fn gutter() -> Style {
        text::subtle().add_modifier(Modifier::DIM)
    }

    pub(crate) fn context() -> Style {
        text::primary()
    }

    pub(crate) fn added_line() -> Style {
        status::success()
    }

    pub(crate) fn removed_line() -> Style {
        status::danger()
    }
}

pub(crate) mod animation {
    use super::*;

    pub(crate) fn active_gradient() -> [Color; 5] {
        ACTIVE_GRADIENT
    }

    pub(crate) fn executing_gradient() -> [Color; 5] {
        EXECUTING_GRADIENT
    }

    pub(crate) fn welcome_gradient() -> [Color; 6] {
        WELCOME_GRADIENT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_roles_are_distinct() {
        assert_ne!(interactive::title(), interactive::selected_option());
        assert_ne!(interactive::selected_option(), interactive::accent());
    }

    #[test]
    fn pending_status_roles_are_distinct() {
        assert_ne!(status::pending_input(), status::pending_approval());
        assert_ne!(status::pending_approval(), status::pending_choice());
    }
}
