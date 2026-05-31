//! Shared semantic theme for the TUI.
//!
//! The theme is organised by *role* rather than by colour name. Widgets call
//! `theme::text::primary()` instead of `Color::White`, which means a single
//! palette change here propagates everywhere automatically. Roles are grouped
//! into submodules (`text`, `border`, `interactive`, `status`, ...) that
//! mirror how the rest of the TUI thinks about styling.
//!
//! Two design constraints govern colour choice:
//! 1. Body copy uses [`Color::Reset`] so text follows the terminal's own
//!    foreground; this keeps it readable on both light and dark terminal
//!    backgrounds.
//! 2. Every accent / status RGB triplet is verified to maintain a contrast
//!    ratio of at least 3.0 against both pure white and pure black (see
//!    [`tests::semantic_rgb_colors_contrast_with_light_and_dark_backgrounds`]),
//!    so the TUI is legible regardless of the user's terminal theme.

use ratatui::style::{Color, Modifier, Style};

// ---------------------------------------------------------------------------
// Palette: raw colour constants. Don't reference these from outside the
// module — go through the role accessors below so the palette can be swapped
// without touching every widget.
// ---------------------------------------------------------------------------

// Body copy follows the terminal's configured foreground, so it stays readable
// on both light and dark terminal backgrounds.
const TEXT_PRIMARY: Color = Color::Reset;
/// Slightly dimmed grey for secondary text such as path strings.
const TEXT_MUTED: Color = Color::Rgb(118, 118, 118);
/// Even softer grey for ambient annotations and gutters.
const TEXT_SUBTLE: Color = Color::Rgb(104, 112, 126);

/// Default accent (focused borders, primary buttons, plan headings).
const ACCENT_PRIMARY: Color = Color::Rgb(100, 120, 156);
/// Cool blue used for "explore" tool calls (read, list, search).
const ACCENT_EXPLORE: Color = Color::Rgb(106, 122, 151);
/// Warm amber used for "edit" tool calls and warnings.
const ACCENT_EDIT: Color = Color::Rgb(142, 112, 44);
/// Sage green used for shell/run tool calls and success status.
const ACCENT_SHELL: Color = Color::Rgb(90, 126, 92);
/// Soft purple used for input-related affordances (composer, prompts).
const ACCENT_INPUT: Color = Color::Rgb(138, 92, 142);
/// Teal used for plan drafts and "in-flight" indicators.
const ACCENT_DRAFT: Color = Color::Rgb(92, 128, 132);
/// Badge accent — currently mirrors `ACCENT_PRIMARY` but kept separate so
/// badges can drift independently in future redesigns.
const ACCENT_BADGE: Color = Color::Rgb(100, 120, 156);

/// Success state colour — used for "ready" and additions in diffs.
const STATUS_SUCCESS: Color = Color::Rgb(90, 126, 92);
/// Danger state colour — used for failures and removals in diffs.
const STATUS_DANGER: Color = Color::Rgb(146, 88, 88);

/// Five-stop palindromic gradient used while a turn is active. The palindrome
/// keeps the ends matched so animations can wrap without a visible seam.
const ACTIVE_GRADIENT: [Color; 5] = [
    Color::Rgb(100, 120, 156),
    Color::Rgb(96, 124, 145),
    Color::Rgb(118, 118, 118),
    Color::Rgb(96, 124, 145),
    Color::Rgb(100, 120, 156),
];

/// Mirror of `ACTIVE_GRADIENT` retuned toward teal, used while a tool is
/// executing.
const EXECUTING_GRADIENT: [Color; 5] = [
    Color::Rgb(100, 120, 156),
    Color::Rgb(92, 128, 132),
    Color::Rgb(118, 118, 118),
    Color::Rgb(92, 128, 132),
    Color::Rgb(100, 120, 156),
];

/// Six-stop welcome gradient driving the animated splash screen letter colours.
const WELCOME_GRADIENT: [Color; 6] = [
    ACCENT_PRIMARY,
    ACCENT_EXPLORE,
    ACCENT_BADGE,
    TEXT_SUBTLE,
    ACCENT_DRAFT,
    ACCENT_PRIMARY,
];

/// Text colour roles for body copy and annotations.
pub(crate) mod text {
    use super::*;

    /// Primary body copy. Uses `Color::Reset` so it follows the user's
    /// configured terminal foreground.
    pub(crate) fn primary() -> Style {
        Style::default().fg(TEXT_PRIMARY)
    }

    /// Slightly dimmed text for secondary content (paths, summaries).
    pub(crate) fn muted() -> Style {
        Style::default().fg(TEXT_MUTED)
    }

    /// Quiet gutter / annotation text.
    pub(crate) fn subtle() -> Style {
        Style::default().fg(TEXT_SUBTLE)
    }

    /// Composer placeholder string. Combines mute with `DIM` for extra fade.
    pub(crate) fn placeholder() -> Style {
        muted().add_modifier(Modifier::DIM)
    }

    /// Help text rendered below interactive prompts. Subtle + DIM.
    pub(crate) fn help() -> Style {
        subtle().add_modifier(Modifier::DIM)
    }
}

/// Border styles.
pub(crate) mod border {
    use super::*;

    /// Border colour for the currently-focused pane.
    pub(crate) fn focused() -> Style {
        Style::default().fg(ACCENT_PRIMARY)
    }

    /// Border colour for unfocused panes.
    pub(crate) fn idle() -> Style {
        Style::default().fg(TEXT_SUBTLE)
    }
}

/// Styles for interactive UI affordances (titles, selections, accents).
pub(crate) mod interactive {
    use super::*;

    /// Bold accent used for popup titles.
    pub(crate) fn title() -> Style {
        Style::default()
            .fg(ACCENT_EXPLORE)
            .add_modifier(Modifier::BOLD)
    }

    /// Highlight applied to the currently-selected entry in a popup.
    pub(crate) fn selected_option() -> Style {
        Style::default()
            .fg(ACCENT_PRIMARY)
            .add_modifier(Modifier::BOLD)
    }

    /// Style used while an interactive widget is mid-transition.
    pub(crate) fn in_progress() -> Style {
        Style::default()
            .fg(ACCENT_PRIMARY)
            .add_modifier(Modifier::BOLD)
    }

    /// Plain accent used for keybinding hints and inline highlights.
    pub(crate) fn accent() -> Style {
        Style::default().fg(ACCENT_PRIMARY)
    }
}

/// Styles for inline badges (workspace label, tool-call tags).
pub(crate) mod badge {
    use super::*;

    /// Workspace badge — dim accent so it doesn't compete with body copy.
    pub(crate) fn workspace() -> Style {
        Style::default()
            .fg(ACCENT_BADGE)
            .add_modifier(Modifier::DIM)
    }
}

/// Per-tool category colours used by tool-call cells.
pub(crate) mod tool {
    use super::*;

    /// Read / list / search tools.
    pub(crate) fn explore() -> Style {
        Style::default().fg(ACCENT_EXPLORE)
    }

    /// Edit / patch tools.
    pub(crate) fn edit() -> Style {
        Style::default().fg(ACCENT_EDIT)
    }

    /// Shell / run tools.
    pub(crate) fn shell() -> Style {
        Style::default().fg(ACCENT_SHELL)
    }

    /// User-input requesting tools (`request_user_input`).
    pub(crate) fn input() -> Style {
        Style::default().fg(ACCENT_INPUT)
    }

    /// Draft / plan tools.
    pub(crate) fn draft() -> Style {
        Style::default().fg(ACCENT_DRAFT)
    }
}

/// Styles for status pills (ready, awaiting input, warning, ...).
pub(crate) mod status {
    use super::*;

    /// Raw success colour for callers that need to compose their own style.
    pub(crate) fn success_color() -> Color {
        STATUS_SUCCESS
    }

    /// Raw danger colour for callers that need to compose their own style.
    pub(crate) fn danger_color() -> Color {
        STATUS_DANGER
    }

    /// Plain success style.
    pub(crate) fn success() -> Style {
        Style::default().fg(STATUS_SUCCESS)
    }

    /// Plain danger style.
    pub(crate) fn danger() -> Style {
        Style::default().fg(STATUS_DANGER)
    }

    /// Bold success style used for the "ready" pill on the welcome screen.
    pub(crate) fn ready() -> Style {
        success().add_modifier(Modifier::BOLD)
    }

    /// Pill used while waiting on input from the user.
    pub(crate) fn pending_input() -> Style {
        Style::default()
            .fg(ACCENT_INPUT)
            .add_modifier(Modifier::BOLD)
    }

    /// Pill used while waiting on a sandbox approval.
    pub(crate) fn pending_approval() -> Style {
        Style::default()
            .fg(ACCENT_EDIT)
            .add_modifier(Modifier::BOLD)
    }

    /// Pill used while waiting on a plan / intent choice.
    pub(crate) fn pending_choice() -> Style {
        Style::default()
            .fg(ACCENT_DRAFT)
            .add_modifier(Modifier::BOLD)
    }

    /// Warning text colour (orange-amber).
    pub(crate) fn warning() -> Style {
        Style::default().fg(ACCENT_EDIT)
    }

    /// Raw warning colour for callers that need to compose their own style.
    pub(crate) fn warning_color() -> Color {
        ACCENT_EDIT
    }
}

/// Markdown rendering styles consumed by [`super::markdown_render`].
pub(crate) mod markdown {
    use super::*;

    /// `#` / `##` heading markers.
    pub(crate) fn heading_marker() -> Style {
        Style::default().fg(ACCENT_PRIMARY)
    }

    /// Inline `code` spans.
    pub(crate) fn code_inline() -> Style {
        Style::default().fg(ACCENT_BADGE)
    }

    /// Fenced `\`\`\`` code block bodies.
    pub(crate) fn code_block() -> Style {
        Style::default().fg(ACCENT_EDIT)
    }

    /// Hyperlink text. Underlined for affordance even though terminals
    /// generally cannot click them.
    pub(crate) fn link() -> Style {
        Style::default()
            .fg(ACCENT_EXPLORE)
            .add_modifier(Modifier::UNDERLINED)
    }

    /// Blockquote (`> `) text.
    pub(crate) fn blockquote() -> Style {
        text::muted()
    }

    /// Bullet markers (`-`, `*`).
    pub(crate) fn bullet() -> Style {
        Style::default().fg(ACCENT_PRIMARY)
    }

    /// Ordered-list markers (`1.`).
    pub(crate) fn ordered() -> Style {
        Style::default().fg(ACCENT_PRIMARY)
    }

    /// Table border characters.
    pub(crate) fn table_border() -> Style {
        text::subtle()
    }

    /// Table header cells. Bold primary text.
    pub(crate) fn table_header() -> Style {
        text::primary().add_modifier(Modifier::BOLD)
    }
}

/// Diff rendering styles consumed by [`super::diff`].
pub(crate) mod diff {
    use super::*;

    /// File header bullet for an "Added" change.
    pub(crate) fn added_header_color() -> Color {
        STATUS_SUCCESS
    }

    /// File header bullet for a "Deleted" change.
    pub(crate) fn removed_header_color() -> Color {
        STATUS_DANGER
    }

    /// File header bullet for an "Update" change.
    pub(crate) fn updated_header_color() -> Color {
        ACCENT_EDIT
    }

    /// Line-number gutter style. Subtle + DIM so numbers fade behind code.
    pub(crate) fn gutter() -> Style {
        text::subtle().add_modifier(Modifier::DIM)
    }

    /// Unchanged context lines.
    pub(crate) fn context() -> Style {
        text::primary()
    }

    /// `+` insertion lines.
    pub(crate) fn added_line() -> Style {
        status::success()
    }

    /// `-` deletion lines.
    pub(crate) fn removed_line() -> Style {
        status::danger()
    }
}

/// Animation gradient palettes consumed by gradient-based widgets.
pub(crate) mod animation {
    use super::*;

    /// Gradient used while a turn is active (idle while no tool is running).
    pub(crate) fn active_gradient() -> [Color; 5] {
        ACTIVE_GRADIENT
    }

    /// Gradient used while a tool is currently running.
    pub(crate) fn executing_gradient() -> [Color; 5] {
        EXECUTING_GRADIENT
    }

    /// Gradient used by the welcome screen's animated logo.
    pub(crate) fn welcome_gradient() -> [Color; 6] {
        WELCOME_GRADIENT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: the three interactive roles must remain visually distinct so
    /// users can tell `title`, `selected option`, and `accent` apart at a
    /// glance. Pin them to differing styles so palette tweaks don't accidentally
    /// collapse two roles.
    #[test]
    fn interactive_roles_are_distinct() {
        assert_ne!(interactive::title(), interactive::selected_option());
        assert_ne!(interactive::selected_option(), interactive::accent());
    }

    /// Scenario: pending-state pills (input / approval / choice) appear next
    /// to each other in modal popups; they must stay distinct so users can
    /// tell which is active.
    #[test]
    fn pending_status_roles_are_distinct() {
        assert_ne!(status::pending_input(), status::pending_approval());
        assert_ne!(status::pending_approval(), status::pending_choice());
    }

    /// Scenario: primary body copy must inherit the terminal's foreground,
    /// not pin a specific colour, so the TUI is readable on both light and
    /// dark themes. This is the single load-bearing property of the palette.
    #[test]
    fn primary_text_uses_terminal_default_foreground() {
        assert_eq!(text::primary().fg, Some(Color::Reset));
    }

    /// Scenario: every accent/status RGB tuple must keep at least 3.0
    /// contrast ratio against both pure white and pure black, so the TUI
    /// stays readable across user themes. Palette tweaks below WCAG-like
    /// thresholds will fail this test.
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
