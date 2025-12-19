use anyhow::{Result, anyhow};
use chrono::{DateTime, Duration, NaiveDate, Utc};

/// Parse date strings used by `log` filters.
/// Supports absolute dates (`YYYY-MM-DD`), full timestamps with timezone,
/// unix timestamps, and relative forms like `1 week ago`.
pub fn parse_date(input: &str) -> Result<i64> {
    let trimmed = input.trim();

    if let Some(ts) = parse_relative_date(trimmed) {
        return Ok(ts);
    }

    if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        let datetime = date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow!("invalid date: {trimmed}"))?
            .and_utc();
        return Ok(datetime.timestamp());
    }

    if let Ok(datetime) = DateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S %z") {
        return Ok(datetime.timestamp());
    }

    if let Ok(datetime) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(datetime.timestamp());
    }

    if let Ok(timestamp) = trimmed.parse::<i64>() {
        return Ok(timestamp);
    }

    Err(anyhow!("invalid date format: {input}"))
}

fn parse_relative_date(input: &str) -> Option<i64> {
    let lower = input.to_lowercase();
    if !lower.contains("ago") {
        return None;
    }

    let mut parts = lower.split_whitespace();
    let value: i64 = parts.next()?.parse().ok()?;
    let unit = parts.next().unwrap_or_default();

    let now = Utc::now();
    let ts = match unit {
        u if u.starts_with("week") => now - Duration::weeks(value),
        u if u.starts_with("day") => now - Duration::days(value),
        u if u.starts_with("hour") => now - Duration::hours(value),
        u if u.starts_with("min") => now - Duration::minutes(value),
        _ => return None,
    };

    Some(ts.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_absolute_date() {
        let ts = parse_date("2024-01-01").unwrap();
        assert!(ts > 0);
    }

    #[test]
    fn parse_relative_week() {
        let ts = parse_date("1 week ago").unwrap();
        assert!(ts < Utc::now().timestamp());
    }
}
