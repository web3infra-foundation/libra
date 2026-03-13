use std::{path::Path, time::Duration};

use ratatui::{
    prelude::*,
    widgets::{Paragraph, Wrap},
};

use super::theme;

pub(crate) struct WelcomeView<'a> {
    pub elapsed: Duration,
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
    let cols = Layout::horizontal([
        Constraint::Percentage(55),
        Constraint::Length(3),
        Constraint::Percentage(45),
    ])
    .split(frame);

    render_luminance_scan(cols[0], buf, view.elapsed.as_secs_f32());
    render_info_panel(cols[2], buf, &view);
}

fn render_luminance_scan(area: Rect, buf: &mut Buffer, t: f32) {
    if area.width < 2 || area.height < 2 {
        return;
    }

    let scan_area = Rect::new(
        area.x + area.width / 18,
        area.y + 1,
        area.width.saturating_mul(11) / 16,
        area.height.saturating_sub(2),
    );
    if scan_area.width < 2 || scan_area.height < 2 {
        return;
    }

    let w = scan_area.width as usize;
    let h = scan_area.height as usize;
    let mut lines = Vec::with_capacity(h);

    for y in 0..h {
        let ny = y as f32 / (h.saturating_sub(1).max(1) as f32);
        let mut spans = Vec::with_capacity(w);

        for x in 0..w {
            let nx = (x as f32 / (w.saturating_sub(1).max(1) as f32)) * 2.0 - 1.0;
            let luminance = base_luminance(nx, ny, t);
            let mask = noise_mask(nx, ny, t);
            let threshold = visibility_threshold(nx, ny, t);
            let combined = (luminance * 0.82 + mask * 0.30).clamp(0.0, 1.0);
            let visible = combined > threshold;
            let glyph = scanline_glyph(nx, ny, luminance, mask, visible, x, y, t);
            spans.push(Span::styled(
                glyph.to_string(),
                glyph_style(luminance, mask, glyph, visible),
            ));
        }

        lines.push(Line::from(spans));
    }

    Paragraph::new(Text::from(lines)).render(scan_area, buf);
}

fn base_luminance(nx: f32, ny: f32, t: f32) -> f32 {
    let drift = (ny * 3.1 - t * 0.08).sin() * 0.09 + (ny * 8.7 + t * 0.05).cos() * 0.04;
    let core_x = drift + (0.40 - ny).clamp(0.0, 0.40) * 0.08;

    let top = soft_blob(nx, ny, core_x, 0.18, 0.42, 0.11, 1.15);
    let body = soft_blob(nx, ny, core_x + 0.02, 0.40, 0.38, 0.15, 1.08);
    let mid = soft_blob(nx, ny, core_x + 0.04, 0.57, 0.30, 0.15, 0.92);
    let tail = soft_blob(nx, ny, core_x + 0.05, 0.77, 0.18, 0.17, 0.58);
    let halo = soft_blob(nx, ny, core_x + 0.01, 0.44, 0.55, 0.26, 0.26);

    let structure = top + body + mid + tail + halo;
    let vertical_fade = (1.0 - (ny - 0.46).abs() * 1.12).clamp(0.0, 1.0);
    let ripple = ((nx * 6.8) + ny * 18.0 - t * 0.12).sin() * 0.04
        + ((nx * 3.4) - ny * 11.0 + t * 0.07).cos() * 0.03;

    ((structure * vertical_fade) + ripple).clamp(0.0, 1.0)
}

fn noise_mask(nx: f32, ny: f32, t: f32) -> f32 {
    let slow = fbm(nx * 2.4 + t * 0.05, ny * 3.1 - t * 0.04, 3);
    let fine = fbm(nx * 7.5 - t * 0.10, ny * 11.0 + t * 0.07, 2);
    let grain = hash2(nx * 21.0 + t * 0.16, ny * 29.0 - t * 0.09);
    (slow * 0.58 + fine * 0.27 + grain * 0.15).clamp(0.0, 1.0)
}

fn visibility_threshold(nx: f32, ny: f32, t: f32) -> f32 {
    let radial = ((nx * 0.82).powi(2) + ((ny - 0.48) * 1.22).powi(2)).sqrt();
    let scan = ((ny * 32.0) + t * 0.18).sin() * 0.035;
    let edge = (radial - 0.28).max(0.0) * 0.42;
    (0.34 + edge + scan).clamp(0.18, 0.82)
}

fn scanline_glyph(
    nx: f32,
    ny: f32,
    luminance: f32,
    mask: f32,
    visible: bool,
    x: usize,
    y: usize,
    t: f32,
) -> char {
    let xi = x as i32;
    let yi = y as i32;
    let line_phase = (((ny * 26.0) + t * 0.06).sin() * 3.0).round() as i32;
    let dash_gate = (xi + line_phase).rem_euclid(10);
    let dot_gate = (xi * 2 + yi + line_phase).rem_euclid(5);
    let core_gate = (xi + yi + line_phase).rem_euclid(6);

    if !visible {
        if mask > 0.80 && dot_gate == 0 {
            return '.';
        }
        if star_gate(x, y, t) {
            return if hash2(x as f32 * 0.7 + t * 0.1, y as f32 * 0.9) > 0.8 {
                '*'
            } else {
                '.'
            };
        }
        return ' ';
    }

    if luminance > 0.86 {
        return if core_gate < 2 { '#' } else { '*' };
    }
    if luminance > 0.70 {
        return if dash_gate < 6 { '*' } else { '+' };
    }
    if luminance > 0.56 {
        return if dash_gate < 7 { '+' } else { '=' };
    }
    if luminance > 0.42 {
        return if dash_gate < 7 { '=' } else { '-' };
    }
    if luminance > 0.28 {
        return if dash_gate < 5 { '-' } else { '.' };
    }
    if mask > 0.62 && dot_gate == 0 {
        return '.';
    }
    if (nx * 8.0 + t * 0.08).sin() > 0.75 && dash_gate < 4 {
        return '-';
    }
    '.'
}

fn glyph_style(luminance: f32, mask: f32, glyph: char, visible: bool) -> Style {
    if glyph == ' ' {
        return Style::default().fg(Color::Reset);
    }

    if !visible {
        if glyph == '*' {
            return theme::text::primary();
        }
        if glyph == '.' {
            return theme::text::subtle();
        }
    }

    if luminance > 0.82 {
        return theme::text::primary().add_modifier(Modifier::BOLD);
    }
    if luminance > 0.62 {
        return theme::badge::workspace().add_modifier(Modifier::BOLD);
    }
    if luminance > 0.36 {
        return theme::tool::explore();
    }
    if mask > 0.58 {
        return theme::interactive::accent();
    }
    theme::text::subtle()
}

fn soft_blob(nx: f32, ny: f32, cx: f32, cy: f32, rx: f32, ry: f32, weight: f32) -> f32 {
    let dx = (nx - cx) / rx.max(0.001);
    let dy = (ny - cy) / ry.max(0.001);
    (-(dx * dx + dy * dy) * 1.7).exp() * weight
}

fn fbm(x: f32, y: f32, octaves: usize) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 0.5;
    let mut frequency = 1.0;
    let mut norm = 0.0;

    for _ in 0..octaves {
        total += value_noise(x * frequency, y * frequency) * amplitude;
        norm += amplitude;
        amplitude *= 0.5;
        frequency *= 2.03;
    }

    if norm <= f32::EPSILON {
        0.0
    } else {
        total / norm
    }
}

fn value_noise(x: f32, y: f32) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let xf = x - x0;
    let yf = y - y0;

    let v00 = hash2(x0, y0);
    let v10 = hash2(x0 + 1.0, y0);
    let v01 = hash2(x0, y0 + 1.0);
    let v11 = hash2(x0 + 1.0, y0 + 1.0);

    let sx = smoothstep(xf);
    let sy = smoothstep(yf);
    let ix0 = lerp(v00, v10, sx);
    let ix1 = lerp(v01, v11, sx);
    lerp(ix0, ix1, sy)
}

fn star_gate(x: usize, y: usize, t: f32) -> bool {
    let seed = hash2(x as f32 * 0.37 + t * 0.07, y as f32 * 0.53 - t * 0.04);
    seed > 0.993
}

fn hash2(x: f32, y: f32) -> f32 {
    let v = (x * 127.1 + y * 311.7).sin() * 43_758.547;
    v.fract().abs()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
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
        kv_line("shader", "luma-noise-scan", theme::status::warning()),
        Line::styled("single welcome animation", theme::text::subtle()),
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
    let width = 132u16.min(area.width.saturating_sub(1));
    let height = 30u16.min(area.height);
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
    use crate::internal::tui::theme;

    #[test]
    fn glyph_style_uses_theme_palette() {
        assert_eq!(
            glyph_style(0.9, 0.2, '#', true),
            theme::text::primary().add_modifier(Modifier::BOLD)
        );
        assert_eq!(glyph_style(0.5, 0.2, '=', true), theme::tool::explore());
        assert_eq!(
            glyph_style(0.1, 0.7, '.', true),
            theme::interactive::accent()
        );
        assert_eq!(glyph_style(0.1, 0.2, '.', false), theme::text::subtle());
    }

    #[test]
    fn info_panel_uses_theme_colors() {
        let view = WelcomeView {
            elapsed: Duration::from_secs(0),
            welcome_message: "hello",
            model_name: "gpt-test",
            provider_name: "openai",
            cwd: Path::new("/tmp/demo"),
        };

        let path = truncate_middle(&view.cwd.display().to_string(), 32);
        let lines = vec![
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
}
