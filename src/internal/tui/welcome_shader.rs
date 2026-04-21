use std::{path::Path, sync::LazyLock, time::Instant};

use ratatui::{
    prelude::*,
    widgets::{Paragraph, Wrap},
};

use super::theme;

static WELCOME_ANIMATION_START: LazyLock<Instant> = LazyLock::new(Instant::now);

pub(crate) struct WelcomeView<'a> {
    pub welcome_message: &'a str,
    pub model_name: &'a str,
    pub provider_name: &'a str,
    pub cwd: &'a Path,
}

pub(crate) fn render(area: Rect, buf: &mut Buffer, view: WelcomeView<'_>) {
    let area = area.intersection(*buf.area());
    if area.width < 20 || area.height < 8 {
        return;
    }

    let frame = centered_canvas(area);
    render_info_panel(frame, buf, &view);
}

fn render_info_panel(area: Rect, buf: &mut Buffer, view: &WelcomeView<'_>) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let logo_lines = [
        "██╗     ██╗██████╗ ██████╗  █████╗     ██████╗ ██████╗ ██████╗ ███████╗",
        "██║     ██║██╔══██╗██╔══██╗██╔══██╗   ██╔════╝██╔═══██╗██╔══██╗██╔════╝",
        "██║     ██║██████╔╝██████╔╝███████║   ██║     ██║   ██║██║  ██║█████╗  ",
        "██║     ██║██╔══██╗██╔══██╗██╔══██║   ██║     ██║   ██║██║  ██║██╔══╝  ",
        "███████╗██║██████╔╝██║  ██║██║  ██║   ╚██████╗╚██████╔╝██████╔╝███████╗",
        "╚══════╝╚═╝╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═╝    ╚═════╝ ╚═════╝ ╚═════╝ ╚══════╝",
    ];

    let value_width = area.width.saturating_sub(13) as usize;
    let path = truncate_middle(&view.cwd.display().to_string(), value_width.max(8));
    let mut lines = vec![];

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

    if lines.len() > area.height as usize {
        lines.truncate(area.height as usize);
    }

    Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .render(area, buf);
}

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

fn gradient_color(x: usize, y: usize, time: f64) -> Color {
    let palette = theme::animation::welcome_gradient();
    let phase = gradient_phase(x, y, time);

    sample_gradient(&palette, phase)
}

fn gradient_phase(x: usize, y: usize, time: f64) -> f64 {
    let x = x as f64;
    let y = y as f64;

    let base_phase = x * 0.024 + y * 0.075 + time * 0.14;
    let shimmer = ((x * 0.12) - (y * 0.18) + time * 1.1).sin() * 0.045;
    let drift = (x * 0.03 + y * 0.015 + time * 0.42).cos() * 0.02;
    (base_phase + shimmer + drift).rem_euclid(1.0)
}

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

fn rgb_components(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

fn kv_line(label: &str, value: &str, value_style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<10}"),
            theme::text::subtle().add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_string(), value_style),
    ])
}

fn centered_canvas(area: Rect) -> Rect {
    let width = 96u16.min(area.width.saturating_sub(2));
    let height = 20u16.min(area.height);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

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

    #[test]
    fn centered_welcome_panel_stays_within_area() {
        let rect = centered_canvas(Rect::new(0, 0, 120, 30));
        assert!(rect.width <= 120);
        assert!(rect.height <= 30);
        assert!(rect.x <= 120);
        assert!(rect.y <= 30);
    }

    #[test]
    fn welcome_gradient_uses_theme_palette_range() {
        let palette = theme::animation::welcome_gradient();
        let color = gradient_color(12, 3, 1.75);

        assert!(palette.contains(&sample_gradient(&palette, 0.0)));
        assert!(matches!(color, Color::Rgb(_, _, _)));
    }

    #[test]
    fn gradient_changes_over_time_for_same_character() {
        let first = gradient_color(18, 2, 0.0);
        let later = gradient_color(18, 2, 0.5);
        assert_ne!(first, later);
    }

    #[test]
    fn gradient_stays_continuous_across_long_runtime() {
        let before = gradient_phase(18, 2, 31.99);
        let after = gradient_phase(18, 2, 32.01);
        let diff = (after - before).abs();
        let wrapped_diff = diff.min(1.0 - diff);
        assert!(wrapped_diff < 0.01, "phase jumped too far: {wrapped_diff}");
    }

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
