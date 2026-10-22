//! Small formatting and time helpers with no external state.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

/// Seconds since the Unix epoch, saturating at 0 for clocks before 1970.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Format a Unix timestamp in the machine's local timezone.
pub fn format_timestamp(ts: i64) -> String {
    match chrono::DateTime::from_timestamp(ts, 0) {
        Some(utc) => utc
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        None => format!("@{ts}"),
    }
}

/// Compact "time ago" rendering used by `rewind log`.
pub fn time_ago(ts: i64) -> String {
    let delta = now_unix().saturating_sub(ts);
    match delta {
        d if d < 60 => "just now".to_string(),
        d if d < 3_600 => format!("{}m ago", d / 60),
        d if d < 86_400 => format!("{}h ago", d / 3_600),
        d if d < 2_592_000 => format!("{}d ago", d / 86_400),
        d => format!("{}mo ago", d / 2_592_000),
    }
}

/// Human-readable byte size (base 1024).
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Parse a retention duration such as `30d`, `12h`, `90m`, `2w`.
pub fn parse_duration(input: &str) -> Result<u64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidInput("empty duration".into()));
    }
    let (number, unit) = trimmed.split_at(
        trimmed
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(trimmed.len()),
    );
    let value: u64 = number
        .parse()
        .map_err(|_| Error::InvalidInput(format!("invalid duration: {trimmed}")))?;
    let seconds_per_unit = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        _ => {
            return Err(Error::InvalidInput(format!(
                "invalid duration unit in {trimmed:?} (use s, m, h, d, or w)"
            )));
        }
    };
    value
        .checked_mul(seconds_per_unit)
        .ok_or_else(|| Error::InvalidInput(format!("duration too large: {trimmed}")))
}

/// Parse a boolean the way humans type it in a config file.
pub fn parse_bool(input: &str) -> Result<bool> {
    match input.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(Error::InvalidInput(format!(
            "invalid boolean {other:?} (use true/false)"
        ))),
    }
}

/// Quote a string for display the way GNU coreutils does in the C locale.
pub fn quote(value: &str) -> String {
    format!("'{value}'")
}

/// Shorten long argument lists for the log table.
pub fn ellipsize(value: &str, max: usize) -> String {
    let count = value.chars().count();
    if count <= max {
        return value.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = value.chars().take(keep).collect();
    out.push('…');
    out
}

/// `%Y-%m-%d` day stamp for entry directory names.
pub fn day_stamp(ts: i64) -> String {
    match chrono::DateTime::from_timestamp(ts, 0) {
        Some(utc) => utc
            .with_timezone(&chrono::Local)
            .format("%Y%m%d")
            .to_string(),
        None => "19700101".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(10 * 1024), "10 KiB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MiB");
    }

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("30d").unwrap(), 2_592_000);
        assert_eq!(parse_duration("2h").unwrap(), 7_200);
        assert_eq!(parse_duration("90").unwrap(), 90);
        assert!(parse_duration("3y").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn booleans_parse() {
        assert!(parse_bool("YES").unwrap());
        assert!(!parse_bool("off").unwrap());
        assert!(parse_bool("maybe").is_err());
    }
}
