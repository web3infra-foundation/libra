use std::path::Path;

use ratatui::{
    prelude::*,
    widgets::{Paragraph, Wrap},
};

use super::theme;

pub(crate) struct WelcomeView<'a> {
    pub welcome_message: &'a str,
    pub model_name: &'a str,
    pub provider_name: &'a str,
    pub cwd: &'a Path,
}

pub(crate) fn render(area: Rect, buf: &mut Buffer, view: WelcomeView<'_>) {
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

    let value_width = area.width.saturating_sub(13) as usize;
    let path = truncate_middle(&view.cwd.display().to_string(), value_width.max(8));
    let mut lines = vec![
        Line::styled("L I B R A   C O D E", theme::interactive::title()),
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
        Line::styled(view.welcome_message, theme::text::muted()),
    ];

    if lines.len() > area.height as usize {
        lines.truncate(area.height as usize);
    }

    Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .render(area, buf);
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
    let height = 16u16.min(area.height);
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
}
