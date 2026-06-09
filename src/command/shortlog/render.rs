use std::{
    fmt,
    io::{self, Write},
};

use crate::{
    command::shortlog::{
        ShortlogOutput,
        wrap::{self, WrapOptions},
    },
    utils::error::{CliError, CliResult, StableErrorCode, emit_warning},
};

pub(super) fn render_shortlog_output(
    output: &ShortlogOutput,
    writer: &mut impl Write,
) -> CliResult<()> {
    let max_count = output
        .authors
        .iter()
        .map(|stats| stats.count)
        .max()
        .unwrap_or(0);
    let width = std::cmp::max(4, max_count.to_string().len());

    for stats in &output.authors {
        if output.email {
            if !write_shortlog_line(
                writer,
                format_args!(
                    "{:>width$}  {} <{}>",
                    stats.count,
                    stats.name,
                    stats.email.as_deref().unwrap_or(""),
                    width = width
                ),
            )? {
                return Ok(());
            }
        } else if !write_shortlog_line(
            writer,
            format_args!("{:>width$}  {}", stats.count, stats.name, width = width),
        )? {
            return Ok(());
        }

        if !output.summary {
            for subject in &stats.subjects {
                if !write_subject(writer, subject, output.wrap)? {
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

fn write_subject(
    writer: &mut impl Write,
    subject: &str,
    wrap_options: Option<WrapOptions>,
) -> CliResult<bool> {
    let (subject, truncated) = wrap::truncate_for_human(subject);
    if truncated {
        emit_warning("shortlog subject exceeded 64 KiB and was truncated in human output");
    }

    if let Some(options) = wrap_options {
        for line in wrap::wrap_subject(subject, options) {
            if !write_shortlog_line(writer, format_args!("{line}"))? {
                return Ok(false);
            }
        }
        return Ok(true);
    }

    write_shortlog_line(writer, format_args!("      {subject}"))
}

pub(super) fn write_shortlog_line(
    writer: &mut impl Write,
    args: fmt::Arguments<'_>,
) -> CliResult<bool> {
    match writer.write_fmt(args) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => return Ok(false),
        Err(err) => return Err(shortlog_output_error(err)),
    }

    match writer.write_all(b"\n") {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(err) => Err(shortlog_output_error(err)),
    }
}

fn shortlog_output_error(err: io::Error) -> CliError {
    CliError::fatal(format!("shortlog output error: {err}"))
        .with_stable_code(StableErrorCode::IoWriteFailed)
}
