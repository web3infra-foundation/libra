use crate::utils::error::{CliError, CliResult, StableErrorCode};

const DEFAULT_WIDTH_SPEC: &str = "76,6,9";
pub(super) const HUMAN_SUBJECT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
pub(super) struct WrapOptions {
    pub width: usize,
    pub indent_first: usize,
    pub indent_rest: usize,
}

pub(super) fn parse_width_arg(value: &Option<Option<String>>) -> CliResult<Option<WrapOptions>> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let spec = raw.as_deref().unwrap_or(DEFAULT_WIDTH_SPEC);
    parse_width_spec(spec).map(Some)
}

fn parse_width_spec(spec: &str) -> CliResult<WrapOptions> {
    let parts = spec.split(',').collect::<Vec<_>>();
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|part| part.is_empty()) {
        return Err(invalid_width_spec(spec));
    }

    let width = parse_positive_usize(parts[0], spec)?;
    let indent_first = match parts.get(1) {
        Some(value) => parse_usize(value, spec)?,
        None => 6,
    };
    let indent_rest = match parts.get(2) {
        Some(value) => parse_usize(value, spec)?,
        None => 9,
    };

    if indent_first >= width || indent_rest >= width {
        return Err(invalid_width_spec(spec));
    }

    Ok(WrapOptions {
        width,
        indent_first,
        indent_rest,
    })
}

fn parse_positive_usize(value: &str, spec: &str) -> CliResult<usize> {
    let parsed = parse_usize(value, spec)?;
    if parsed == 0 {
        return Err(invalid_width_spec(spec));
    }
    Ok(parsed)
}

fn parse_usize(value: &str, spec: &str) -> CliResult<usize> {
    value.parse().map_err(|_| invalid_width_spec(spec))
}

fn invalid_width_spec(spec: &str) -> CliError {
    CliError::fatal(format!("invalid shortlog -w spec '{spec}'"))
        .with_stable_code(StableErrorCode::CliInvalidArguments)
        .with_hint("use -w, -w=<width>, -w=<width>,<indent1>, or -w=<width>,<indent1>,<indent2>")
}

pub(super) fn truncate_for_human(subject: &str) -> (&str, bool) {
    if subject.len() <= HUMAN_SUBJECT_LIMIT {
        return (subject, false);
    }

    let mut end = HUMAN_SUBJECT_LIMIT;
    while end > 0 && !subject.is_char_boundary(end) {
        end -= 1;
    }
    (&subject[..end], true)
}

pub(super) fn wrap_subject(subject: &str, options: WrapOptions) -> Vec<String> {
    let words = subject.split_whitespace().collect::<Vec<_>>();
    if words.is_empty() {
        return vec![" ".repeat(options.indent_first)];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_indent = options.indent_first;

    for word in words {
        let available = options.width.saturating_sub(current_indent).max(1);
        if current.is_empty() {
            current.push_str(word);
            continue;
        }

        if current.len() + 1 + word.len() <= available {
            current.push(' ');
            current.push_str(word);
            continue;
        }

        lines.push(format!("{}{}", " ".repeat(current_indent), current));
        current.clear();
        current.push_str(word);
        current_indent = options.indent_rest;
    }

    if !current.is_empty() {
        lines.push(format!("{}{}", " ".repeat(current_indent), current));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare_width_uses_git_defaults() {
        let options = parse_width_arg(&Some(Some(DEFAULT_WIDTH_SPEC.to_string())))
            .unwrap()
            .unwrap();
        assert_eq!(options.width, 76);
        assert_eq!(options.indent_first, 6);
        assert_eq!(options.indent_rest, 9);
    }

    #[test]
    fn parse_custom_width_accepts_three_segments() {
        let options = parse_width_arg(&Some(Some("24,4,8".to_string())))
            .unwrap()
            .unwrap();
        assert_eq!(options.width, 24);
        assert_eq!(options.indent_first, 4);
        assert_eq!(options.indent_rest, 8);
    }

    #[test]
    fn parse_invalid_width_rejects_bad_segments() {
        let error = parse_width_arg(&Some(Some("abc".to_string()))).unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::CliInvalidArguments);
    }

    #[test]
    fn wrap_subject_uses_first_and_rest_indents() {
        let lines = wrap_subject(
            "one two three four five",
            WrapOptions {
                width: 16,
                indent_first: 6,
                indent_rest: 9,
            },
        );
        assert_eq!(
            lines,
            vec![
                "      one two",
                "         three",
                "         four",
                "         five"
            ]
        );
    }
}
