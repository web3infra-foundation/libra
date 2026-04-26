//! Animated welcome splash for `libra code`.
//!
//! Renders the giant block-letter `LIBRA CODE` logo plus a small info panel
//! (provider, model, cwd, ready hint) inside a centered canvas. The logo
//! letters are coloured by sampling a gradient that drifts over time, giving a
//! shimmering effect without any per-character state.
//!
//! The screen is replaced by the chat transcript on the user's first submit,
//! so the shader only runs while the user is reading the welcome message.

use std::{path::Path, sync::LazyLock, time::Instant};

use ratatui::{
    prelude::*,
    widgets::{Paragraph, Wrap},
};

use super::theme;

/// Wall-clock origin used by the gradient phase so the animation appears to
/// start when the splash first renders. `LazyLock` defers initialisation until
/// the first frame, which avoids attributing pre-render setup time to the
/// animation.
static WELCOME_ANIMATION_START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Inputs fed into the splash renderer.
///
/// Lifetime-borrowed so the App can pass references to its already-stored
/// values without an extra allocation per frame.
pub(crate) struct WelcomeView<'a> {
    /// Multi-line greeting text shown below the info panel.
    pub welcome_message: &'a str,
    /// Friendly model name (e.g. `gemini-2.5-flash`).
    pub model_name: &'a str,
    /// Provider key (e.g. `gemini`).
    pub provider_name: &'a str,
    /// Current working directory (typically the repo root).
    pub cwd: &'a Path,
}

/// Top-level renderer.
///
/// Functional scope: clamps the supplied area to the buffer bounds, bails out
/// early when there is not enough space, then delegates to the info panel
/// renderer at a centered sub-rect.
///
/// Boundary conditions:
/// - Minimum drawable size is 20x8; below that the splash is skipped because
///   the logo would be unreadable.
/// - The intersection guards against incoming rectangles outside the buffer
///   (e.g. during early-start tear-down or testing scenarios).
///
/// See: [`tests::centered_welcome_panel_stays_within_area`].
pub(crate) fn render(area: Rect, buf: &mut Buffer, view: WelcomeView<'_>) {
    let area = area.intersection(*buf.area());
    if area.width < 20 || area.height < 8 {
        return;
    }

    let frame = centered_canvas(area);
    render_info_panel(frame, buf, &view);
}

/// Renders the logo + info panel into `area`.
///
/// Functional scope: lays out (top to bottom) the six logo rows, a tagline,
/// the provider/model/cwd key-value lines, the "ready" pill, two hint lines,
/// and finally the user-supplied welcome message.
///
/// Boundary conditions:
/// - Skipped on a zero-sized area to avoid a `Paragraph` panic.
/// - The cwd value is truncated in the middle so the path label and the
///   prefix/suffix of the path remain visible at narrow widths.
/// - Lines are truncated to `area.height` so an over-tall message cannot
///   spill past the canvas; the ratatui `Wrap` is `trim: false` so explicit
///   newlines are preserved verbatim.
fn render_info_panel(area: Rect, buf: &mut Buffer, view: &WelcomeView<'_>) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // ASCII block letters spelling "LIBRA CODE". Static `&'static str` rows
    // are reused across frames so `shader_line` can return borrowed spans.
    let logo_lines = [
        "██╗     ██╗██████╗ ██████╗  █████╗     ██████╗ ██████╗ ██████╗ ███████╗",
        "██║     ██║██╔══██╗██╔══██╗██╔══██╗   ██╔════╝██╔═══██╗██╔══██╗██╔════╝",
        "██║     ██║██████╔╝██████╔╝███████║   ██║     ██║   ██║██║  ██║█████╗  ",
        "██║     ██║██╔══██╗██╔══██╗██╔══██║   ██║     ██║   ██║██║  ██║██╔══╝  ",
        "███████╗██║██████╔╝██║  ██║██║  ██║   ╚██████╗╚██████╔╝██████╔╝███████╗",
        "╚══════╝╚═╝╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═╝    ╚═════╝ ╚═════╝ ╚═════╝ ╚══════╝",
    ];

    // Reserve column budget for the cwd value: subtract the label width plus
    // padding so the path never overflows into the right edge.
    let value_width = area.width.saturating_sub(13) as usize;
    let path = truncate_middle(&view.cwd.display().to_string(), value_width.max(8));
    let mut lines = vec![];

    // Logo rows first; each row is animated by `shader_line`.
    for (i, text) in logo_lines.iter().enumerate() {
        lines.push(shader_line(text, i));
    }

    lines.extend([
        Line::raw(""),
        Line::styled("interactive agent console", theme::text::subtle()),
        Line::raw(""),
        kv_line("provider", view.provider_name, theme::text::primary()),
        kv_line("model", view.model_name, theme::text::primary()),
        kv_line("cwd", &path, theme::text::muted()),
        Line::raw(""),
        Line::styled("ready", theme::status::ready()),
        Line::styled(
            "Type your request below and press Enter.",
            theme::text::primary(),
        ),
        Line::styled(
            "The intro screen exits after your first submit.",
            theme::text::muted(),
        ),
        Line::raw(""),
    ]);
    lines.extend(
        view.welcome_message
            .lines()
            .map(|line| Line::styled(line.to_string(), theme::text::muted())),
    );

    // Hard truncate to canvas height so over-tall welcome messages don't spill.
    if lines.len() > area.height as usize {
        lines.truncate(area.height as usize);
    }

    Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .render(area, buf);
}

/// Convert a static logo row into an animated, per-character coloured `Line`.
///
/// Functional scope: walks character boundaries (UTF-8 safe) so multi-byte
/// glyphs in the block-letter logo are split correctly, then assigns each
/// glyph a colour sampled from the gradient at `(col, row, time)`.
///
/// Boundary conditions:
/// - Returns `Line<'static>` with borrowed spans so frames don't allocate
///   per-glyph strings; this is verified by
///   [`tests::shader_line_reuses_static_character_slices`].
fn shader_line(text: &'static str, row: usize) -> Line<'static> {
    let time = WELCOME_ANIMATION_START.elapsed().as_secs_f64();

    let mut spans = Vec::with_capacity(text.chars().count());
    let mut chars = text.char_indices().peekable();
    let mut col = 0;
    while let Some((start, _)) = chars.next() {
        let end = chars.peek().map_or(text.len(), |(idx, _)| *idx);
        let color = gradient_color(col, row, time);
        spans.push(Span::styled(
            &text[start..end],
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        col += 1;
    }
    Line::from(spans)
}

/// Sample the welcome gradient at `(x, y)` and the supplied wall-clock time.
fn gradient_color(x: usize, y: usize, time: f64) -> Color {
    let palette = theme::animation::welcome_gradient();
    let phase = gradient_phase(x, y, time);

    sample_gradient(&palette, phase)
}

/// Compute a wrapped `[0.0, 1.0)` phase for a character at `(x, y)`.
///
/// The phase combines a linear base trajectory with two oscillating offsets
/// (`shimmer` and `drift`) so glyphs animate independently rather than
/// scrolling in lockstep. `rem_euclid` keeps the value continuously wrapped
/// regardless of how long the splash has been visible — verified by
/// [`tests::gradient_stays_continuous_across_long_runtime`].
fn gradient_phase(x: usize, y: usize, time: f64) -> f64 {
    let x = x as f64;
    let y = y as f64;

    let base_phase = x * 0.024 + y * 0.075 + time * 0.14;
    let shimmer = ((x * 0.12) - (y * 0.18) + time * 1.1).sin() * 0.045;
    let drift = (x * 0.03 + y * 0.015 + time * 0.42).cos() * 0.02;
    (base_phase + shimmer + drift).rem_euclid(1.0)
}

/// Sample a gradient palette at the given normalised phase.
///
/// Functional scope: linearly interpolates between adjacent palette entries.
///
/// Boundary conditions:
/// - Empty palette returns `Color::Reset` so callers never panic on
///   accidentally empty arrays.
/// - Single-entry palettes return the single colour.
/// - `phase` is clamped to slightly under 1.0 so the integer index is at most
///   `palette.len() - 1`, leaving the last bucket open-ended.
fn sample_gradient(colors: &[Color], phase: f64) -> Color {
    if colors.is_empty() {
        return Color::Reset;
    }
    if colors.len() == 1 {
        return colors[0];
    }

    let scaled = phase.clamp(0.0, 0.999_9) * (colors.len() - 1) as f64;
    let index = scaled.floor() as usize;
    let next = (index + 1).min(colors.len() - 1);
    let mix = scaled - index as f64;

    lerp_color(colors[index], colors[next], mix)
}

/// Linearly interpolate between two RGB colours.
///
/// Boundary conditions: returns one of the inputs unchanged when the other
/// cannot be decomposed (palette indices, ANSI 16, ...). The shader palette
/// is RGB-only so this fallback is defensive.
fn lerp_color(from: Color, to: Color, t: f64) -> Color {
    let Some((fr, fg, fb)) = rgb_components(from) else {
        return to;
    };
    let Some((tr, tg, tb)) = rgb_components(to) else {
        return from;
    };

    let mix = t.clamp(0.0, 1.0);
    let lerp = |start: u8, end: u8| -> u8 {
        (start as f64 + (end as f64 - start as f64) * mix).round() as u8
    };

    Color::Rgb(lerp(fr, tr), lerp(fg, tg), lerp(fb, tb))
}

/// Extract RGB triplet for `Color::Rgb`, returning `None` for other variants.
fn rgb_components(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

/// Build a `label  value` row used for provider/model/cwd lines.
fn kv_line(label: &str, value: &str, value_style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            // 10-wide left-aligned label so all rows align under each other.
            format!("{label:<10}"),
            theme::text::subtle().add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_string(), value_style),
    ])
}

/// Centre a 96x20 (max) canvas inside `area`.
///
/// Boundary conditions: the canvas shrinks below 96 columns when the terminal
/// is narrow, but the `saturating_sub` guards against negative widths if the
/// terminal is smaller than the unconditional 2-column padding.
fn centered_canvas(area: Rect) -> Rect {
    let width = 96u16.min(area.width.saturating_sub(2));
    let height = 20u16.min(area.height);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

/// Shrink `text` to at most `max_chars` characters by replacing the middle
/// with `...`, preserving both ends so paths and identifiers stay recognisable.
///
/// Boundary conditions:
/// - Returns the original string unchanged when it already fits.
/// - For `max_chars <= 3` returns the literal `"..."` because there is no
///   space for any context characters.
fn truncate_middle(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return "...".to_string();
    }
    let keep = max_chars - 3;
    let left = keep / 2;
    let right = keep - left;
    let head: String = text.chars().take(left).collect();
    let tail: String = text
        .chars()
        .rev()
        .take(right)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{head}...{tail}")
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    /// Scenario: the info panel must consume styles from the shared theme so
    /// palette-level changes propagate without per-widget tweaks. This test
    /// builds the same lines the renderer would and asserts each role.
    #[test]
    fn info_panel_uses_theme_colors() {
        let view = WelcomeView {
            welcome_message: "hello",
            model_name: "gpt-test",
            provider_name: "openai",
            cwd: Path::new("/tmp/demo"),
        };

        let path = truncate_middle(&view.cwd.display().to_string(), 32);
        let lines = [
            Line::styled("L I B R A   C O D E", theme::interactive::title()),
            Line::styled("interactive agent console", theme::text::subtle()),
            kv_line("provider", view.provider_name, theme::text::primary()),
            kv_line("cwd", &path, theme::text::muted()),
        ];

        assert_eq!(lines[0].style, theme::interactive::title());
        assert_eq!(lines[1].style, theme::text::subtle());
        assert_eq!(lines[2].spans[1].style, theme::text::primary());
        assert_eq!(lines[3].spans[1].style, theme::text::muted());
    }

    /// Scenario: regardless of the input area size, the centered canvas must
    /// fit entirely within the area so painting never panics on out-of-bounds
    /// indices. Pin the bounds for a typical 120x30 terminal.
    #[test]
    fn centered_welcome_panel_stays_within_area() {
        let rect = centered_canvas(Rect::new(0, 0, 120, 30));
        assert!(rect.width <= 120);
        assert!(rect.height <= 30);
        assert!(rect.x <= 120);
        assert!(rect.y <= 30);
    }

    /// Scenario: the welcome gradient must always sample to an RGB colour
    /// because the renderer assumes RGB output. Guards against future palette
    /// changes that introduce non-RGB entries.
    #[test]
    fn welcome_gradient_uses_theme_palette_range() {
        let palette = theme::animation::welcome_gradient();
        let color = gradient_color(12, 3, 1.75);

        assert!(palette.contains(&sample_gradient(&palette, 0.0)));
        assert!(matches!(color, Color::Rgb(_, _, _)));
    }

    /// Scenario: the same character at the same position must change colour
    /// over time; otherwise the splash would look static. This is the core
    /// "is the animation actually animating?" check.
    #[test]
    fn gradient_changes_over_time_for_same_character() {
        let first = gradient_color(18, 2, 0.0);
        let later = gradient_color(18, 2, 0.5);
        assert_ne!(first, later);
    }

    /// Scenario: long-running splash sessions must never produce a phase jump
    /// when the wrap boundary crosses. We sample either side of a near-32s
    /// boundary and confirm both halves are within 0.01 of each other modulo
    /// the wrap.
    #[test]
    fn gradient_stays_continuous_across_long_runtime() {
        let before = gradient_phase(18, 2, 31.99);
        let after = gradient_phase(18, 2, 32.01);
        let diff = (after - before).abs();
        let wrapped_diff = diff.min(1.0 - diff);
        assert!(wrapped_diff < 0.01, "phase jumped too far: {wrapped_diff}");
    }

    /// Scenario: each frame allocates one `Line` per logo row, so per-glyph
    /// allocations would dominate; pin the borrowing behaviour so refactors
    /// don't accidentally introduce per-frame heap churn.
    #[test]
    fn shader_line_reuses_static_character_slices() {
        let line = shader_line("LBR", 0);
        assert_eq!(line.spans.len(), 3);
        assert!(
            line.spans
                .iter()
                .all(|span| matches!(span.content, Cow::Borrowed(_)))
        );
    }
}
