//! Shared semantic theme for the TUI.

use ratatui::style::{Color, Modifier, Style};

// Body copy follows the terminal's configured foreground, so it stays readable
// on both light and dark terminal backgrounds.
const TEXT_PRIMARY: Color = Color::Reset;
const TEXT_MUTED: Color = Color::Rgb(118, 118, 118);
const TEXT_SUBTLE: Color = Color::Rgb(104, 112, 126);

const ACCENT_PRIMARY: Color = Color::Rgb(100, 120, 156);
const ACCENT_EXPLORE: Color = Color::Rgb(106, 122, 151);
const ACCENT_EDIT: Color = Color::Rgb(142, 112, 44);
const ACCENT_SHELL: Color = Color::Rgb(90, 126, 92);
const ACCENT_INPUT: Color = Color::Rgb(138, 92, 142);
const ACCENT_DRAFT: Color = Color::Rgb(92, 128, 132);
const ACCENT_BADGE: Color = Color::Rgb(100, 120, 156);

const STATUS_SUCCESS: Color = Color::Rgb(90, 126, 92);
const STATUS_DANGER: Color = Color::Rgb(146, 88, 88);

const ACTIVE_GRADIENT: [Color; 5] = [
    Color::Rgb(100, 120, 156),
    Color::Rgb(96, 124, 145),
    Color::Rgb(118, 118, 118),
    Color::Rgb(96, 124, 145),
    Color::Rgb(100, 120, 156),
];

const EXECUTING_GRADIENT: [Color; 5] = [
    Color::Rgb(100, 120, 156),
    Color::Rgb(92, 128, 132),
    Color::Rgb(118, 118, 118),
    Color::Rgb(92, 128, 132),
    Color::Rgb(100, 120, 156),
];

const WELCOME_GRADIENT: [Color; 6] = [
    ACCENT_PRIMARY,
    ACCENT_EXPLORE,
    ACCENT_BADGE,
    TEXT_SUBTLE,
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

    #[test]
    fn primary_text_uses_terminal_default_foreground() {
        assert_eq!(text::primary().fg, Some(Color::Reset));
    }

    #[test]
    fn semantic_rgb_colors_contrast_with_light_and_dark_backgrounds() {
        let colors = [
            TEXT_MUTED,
            TEXT_SUBTLE,
            ACCENT_PRIMARY,
            ACCENT_EXPLORE,
            ACCENT_EDIT,
            ACCENT_SHELL,
            ACCENT_INPUT,
            ACCENT_DRAFT,
            ACCENT_BADGE,
            STATUS_SUCCESS,
            STATUS_DANGER,
            ACTIVE_GRADIENT[0],
            ACTIVE_GRADIENT[1],
            ACTIVE_GRADIENT[2],
            EXECUTING_GRADIENT[0],
            EXECUTING_GRADIENT[1],
            EXECUTING_GRADIENT[2],
        ];

        for color in colors {
            assert_min_contrast(color, Color::White, 3.0);
            assert_min_contrast(color, Color::Black, 3.0);
        }
    }

    fn assert_min_contrast(foreground: Color, background: Color, minimum: f64) {
        let ratio = contrast_ratio(foreground, background);
        assert!(
            ratio >= minimum,
            "expected {foreground:?} to have contrast >= {minimum} against {background:?}, got {ratio:.2}"
        );
    }

    fn contrast_ratio(foreground: Color, background: Color) -> f64 {
        let foreground = relative_luminance(foreground);
        let background = relative_luminance(background);
        let (lighter, darker) = if foreground >= background {
            (foreground, background)
        } else {
            (background, foreground)
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    fn relative_luminance(color: Color) -> f64 {
        let (red, green, blue) = match color {
            Color::Black => (0, 0, 0),
            Color::White => (255, 255, 255),
            Color::Rgb(red, green, blue) => (red, green, blue),
            other => panic!("unsupported test color: {other:?}"),
        };
        0.2126 * linearized_channel(red)
            + 0.7152 * linearized_channel(green)
            + 0.0722 * linearized_channel(blue)
    }

    fn linearized_channel(value: u8) -> f64 {
        let scaled = f64::from(value) / 255.0;
        if scaled <= 0.04045 {
            scaled / 12.92
        } else {
            ((scaled + 0.055) / 1.055).powf(2.4)
        }
    }
}
