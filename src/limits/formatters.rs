use super::cli::LimitFormat;
use super::{
    LimitCurrentReport, LimitCurrentWindow, LimitReportDiagnostics, LimitResetEvent,
    LimitResetsReport, LimitSamplesReport, LimitTrendChange, LimitTrendReport, LimitWindow,
    LimitWindowsReport, RateLimitSample,
};
use crate::error::AppError;
use crate::format::{
    format_credits, format_csv, format_integer, format_markdown_table, format_plain_table,
    format_usd, to_pretty_json,
};
use chrono::{DateTime, Datelike, Local, Timelike, Utc};

pub(super) fn format_limit_current(
    report: &LimitCurrentReport,
    format: LimitFormat,
    verbose: bool,
) -> Result<String, AppError> {
    if format == LimitFormat::Json {
        return pretty_json(report);
    }

    let mut rows = vec![current_headers()];
    rows.extend(report.current.iter().map(current_row));
    format_rows(
        "Codex rate limits",
        report.status.as_str(),
        report.start,
        report.end,
        &report.sessions_dir,
        "No observed rate limits found in the current 7-day range.",
        rows,
        report.diagnostics.as_ref(),
        format,
        verbose,
    )
}

pub(super) fn format_limit_windows(
    report: &LimitWindowsReport,
    format: LimitFormat,
    verbose: bool,
) -> Result<String, AppError> {
    if format == LimitFormat::Json {
        return pretty_json(report);
    }

    let mut rows = vec![window_headers()];
    rows.extend(report.windows.iter().map(window_row));
    format_rows(
        "Codex rate limit windows",
        report.status.as_str(),
        report.start,
        report.end,
        &report.sessions_dir,
        "No observed rate limit windows found in this range.",
        rows,
        report.diagnostics.as_ref(),
        format,
        verbose,
    )
}

pub(super) fn format_limit_trend(
    report: &LimitTrendReport,
    format: LimitFormat,
    verbose: bool,
) -> Result<String, AppError> {
    if format == LimitFormat::Json {
        return pretty_json(report);
    }

    let mut rows = vec![trend_headers()];
    rows.extend(report.changes.iter().map(trend_row));
    format_rows(
        "Codex rate limit trend",
        report.status.as_str(),
        report.start,
        report.end,
        &report.sessions_dir,
        "No observed rate limit trend changes found in this range.",
        rows,
        report.diagnostics.as_ref(),
        format,
        verbose,
    )
}

pub(super) fn format_limit_resets(
    report: &LimitResetsReport,
    format: LimitFormat,
    verbose: bool,
) -> Result<String, AppError> {
    if format == LimitFormat::Json {
        return pretty_json(report);
    }

    let mut rows = vec![reset_headers()];
    rows.extend(report.resets.iter().map(reset_row));
    format_rows(
        "Codex rate limit resets",
        report.status.as_str(),
        report.start,
        report.end,
        &report.sessions_dir,
        "No rate limit reset events found in this range.",
        rows,
        report.diagnostics.as_ref(),
        format,
        verbose,
    )
}

pub(super) fn format_limit_samples(
    report: &LimitSamplesReport,
    format: LimitFormat,
    verbose: bool,
) -> Result<String, AppError> {
    if format == LimitFormat::Json {
        return pretty_json(report);
    }

    let mut rows = vec![sample_headers()];
    rows.extend(report.samples.iter().map(sample_row));
    format_rows(
        "Codex rate limit samples",
        report.status.as_str(),
        report.start,
        report.end,
        &report.sessions_dir,
        "No observed rate limit samples found in this range.",
        rows,
        report.diagnostics.as_ref(),
        format,
        verbose,
    )
}

fn pretty_json<T: serde::Serialize>(report: &T) -> Result<String, AppError> {
    Ok(format!(
        "{}\n",
        to_pretty_json(report).map_err(|error| AppError::new(error.to_string()))?
    ))
}

#[allow(clippy::too_many_arguments)]
fn format_rows(
    title: &str,
    status: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    sessions_dir: &str,
    empty_message: &str,
    rows: Vec<Vec<String>>,
    diagnostics: Option<&LimitReportDiagnostics>,
    format: LimitFormat,
    verbose: bool,
) -> Result<String, AppError> {
    if format == LimitFormat::Csv {
        return Ok(format!("{}\n", format_csv(&rows)));
    }

    if format == LimitFormat::Markdown {
        let mut lines = vec![format_markdown_table(&rows)];
        append_diagnostics(&mut lines, diagnostics, verbose);
        return Ok(format!("{}\n", lines.join("\n")));
    }

    let mut lines = vec![
        title.to_string(),
        format!("Status: {status}"),
        format!(
            "Range: {} to {}",
            format_date_time(start),
            format_date_time(end)
        ),
        format!("Sessions dir: {sessions_dir}"),
        String::new(),
    ];

    if rows.len() == 1 {
        lines.push(empty_message.to_string());
        append_diagnostics(&mut lines, diagnostics, verbose);
        return Ok(lines.join("\n"));
    }

    lines.push(format_plain_table(&rows));
    append_diagnostics(&mut lines, diagnostics, verbose);
    Ok(lines.join("\n"))
}

fn append_diagnostics(
    lines: &mut Vec<String>,
    diagnostics: Option<&LimitReportDiagnostics>,
    verbose: bool,
) {
    if !verbose {
        return;
    }
    let Some(diagnostics) = diagnostics else {
        return;
    };

    lines.push(String::new());
    lines.push("Diagnostics:".to_string());
    lines.push(format!(
        "  --full-scan requested: {}",
        if diagnostics.scan.scan_all_files {
            "yes"
        } else {
            "no"
        }
    ));
    lines.push(format!(
        "  Directories scanned: {}",
        format_integer(diagnostics.scan.scanned_directories)
    ));
    lines.push(format!(
        "  Full files read: {}",
        format_integer(diagnostics.scan.read_files)
    ));
    lines.push(format!(
        "  Files skipped by tail prefilter: {}",
        format_integer(diagnostics.scan.prefiltered_files)
    ));
    lines.push(format!(
        "  Tail rate-limit reads: {}",
        format_integer(diagnostics.scan.tail_read_files)
    ));
    lines.push(format!(
        "  Lines read: {}",
        format_integer(diagnostics.scan.read_lines)
    ));
    lines.push(format!(
        "  Rate-limit events: {}",
        format_integer(diagnostics.scan.rate_limit_events)
    ));
    lines.push(format!(
        "  Included samples: {}",
        format_integer(diagnostics.scan.included_samples)
    ));
    lines.push(format!(
        "  Null rate limits: {}",
        format_integer(diagnostics.scan.null_rate_limits)
    ));
    lines.push(format!(
        "  Rate-limit-only files: {}",
        format_integer(diagnostics.scan.rate_limit_only_files)
    ));
    lines.push(format!(
        "  Duplicate samples: {}",
        format_integer(diagnostics.duplicate_samples)
    ));
    lines.push(format!(
        "  Unknown limit samples: {}",
        format_integer(diagnostics.unknown_limit_samples)
    ));
    lines.push(format!(
        "  Unknown limit reset events: {}",
        format_integer(diagnostics.unknown_limit_reset_events)
    ));
    lines.push(format!(
        "  Ignored inactive stream samples: {}",
        format_integer(diagnostics.ignored_inactive_stream_samples)
    ));
}

fn current_headers() -> Vec<String> {
    vec![
        "Status".to_string(),
        "Window".to_string(),
        "Window minutes".to_string(),
        "Account".to_string(),
        "Plan".to_string(),
        "Limit".to_string(),
        "Used".to_string(),
        "Remaining".to_string(),
        "Resets at".to_string(),
        "Last seen".to_string(),
        "Total tokens".to_string(),
        "Credits".to_string(),
        "USD".to_string(),
    ]
}

fn current_row(row: &LimitCurrentWindow) -> Vec<String> {
    vec![
        row.status.clone(),
        row.window.clone(),
        row.window_minutes.to_string(),
        optional_text(row.account_id.as_deref()),
        optional_text(row.plan_type.as_deref()),
        optional_text(row.limit_id.as_deref()),
        optional_percent(row.used_percent),
        optional_percent(row.remaining_percent),
        optional_date_time(row.resets_at),
        optional_date_time(row.last_seen),
        format_integer(row.total_tokens),
        format_credits(row.credits),
        format_usd(row.usd),
    ]
}

fn window_headers() -> Vec<String> {
    vec![
        "Window".to_string(),
        "Account".to_string(),
        "Plan".to_string(),
        "Limit".to_string(),
        "Estimated start".to_string(),
        "Reset at".to_string(),
        "First seen".to_string(),
        "Last seen".to_string(),
        "Min used".to_string(),
        "Max used".to_string(),
        "Last used".to_string(),
        "Samples".to_string(),
        "Reset kind".to_string(),
        "Total tokens".to_string(),
        "Credits".to_string(),
        "USD".to_string(),
    ]
}

fn window_row(row: &LimitWindow) -> Vec<String> {
    vec![
        row.window.clone(),
        optional_text(row.account_id.as_deref()),
        optional_text(row.plan_type.as_deref()),
        optional_text(row.limit_id.as_deref()),
        format_date_time(row.estimated_start),
        format_date_time(row.reset_at),
        format_date_time(row.first_seen),
        format_date_time(row.last_seen),
        format_percent(row.min_used_percent),
        format_percent(row.max_used_percent),
        format_percent(row.last_used_percent),
        format_integer(row.sample_count),
        row.reset_kind.clone(),
        format_integer(row.total_tokens),
        format_credits(row.credits),
        format_usd(row.usd),
    ]
}

fn trend_headers() -> Vec<String> {
    vec![
        "At".to_string(),
        "Window".to_string(),
        "Account".to_string(),
        "Plan".to_string(),
        "Limit".to_string(),
        "Used".to_string(),
        "Remaining".to_string(),
        "Delta used".to_string(),
        "Resets at".to_string(),
        "Kind".to_string(),
    ]
}

fn trend_row(row: &LimitTrendChange) -> Vec<String> {
    vec![
        format_date_time(row.at),
        row.window.clone(),
        optional_text(row.account_id.as_deref()),
        optional_text(row.plan_type.as_deref()),
        optional_text(row.limit_id.as_deref()),
        format_percent(row.used_percent),
        format_percent(row.remaining_percent),
        optional_percent(row.delta_used_percent),
        format_date_time(row.resets_at),
        row.kind.clone(),
    ]
}

fn reset_headers() -> Vec<String> {
    vec![
        "At".to_string(),
        "Window".to_string(),
        "Account".to_string(),
        "Plan".to_string(),
        "Limit".to_string(),
        "Previous used".to_string(),
        "Next used".to_string(),
        "Previous reset".to_string(),
        "Next reset".to_string(),
        "Early by seconds".to_string(),
        "Kind".to_string(),
    ]
}

fn reset_row(row: &LimitResetEvent) -> Vec<String> {
    vec![
        format_date_time(row.at),
        row.window.clone(),
        optional_text(row.account_id.as_deref()),
        optional_text(row.plan_type.as_deref()),
        optional_text(row.limit_id.as_deref()),
        format_percent(row.previous_used_percent),
        format_percent(row.next_used_percent),
        format_date_time(row.previous_resets_at),
        format_date_time(row.next_resets_at),
        format_integer(row.early_by_seconds),
        row.kind.clone(),
    ]
}

fn sample_headers() -> Vec<String> {
    vec![
        "timestamp".to_string(),
        "sessionId".to_string(),
        "accountId".to_string(),
        "planType".to_string(),
        "limitId".to_string(),
        "window".to_string(),
        "windowMinutes".to_string(),
        "usedPercent".to_string(),
        "remainingPercent".to_string(),
        "resetsAt".to_string(),
    ]
}

fn sample_row(row: &RateLimitSample) -> Vec<String> {
    vec![
        format_date_time(row.timestamp),
        row.session_id.clone(),
        optional_text(row.account_id.as_deref()),
        optional_text(row.plan_type.as_deref()),
        optional_text(row.limit_id.as_deref()),
        row.window.clone(),
        row.window_minutes.to_string(),
        format_decimal(row.used_percent),
        format_decimal(row.remaining_percent),
        format_date_time(row.resets_at),
    ]
}

fn optional_text(value: Option<&str>) -> String {
    value.unwrap_or("unknown").to_string()
}

fn optional_percent(value: Option<f64>) -> String {
    value.map(format_percent).unwrap_or_default()
}

fn optional_date_time(value: Option<DateTime<Utc>>) -> String {
    value.map(format_date_time).unwrap_or_default()
}

fn format_date_time(date: DateTime<Utc>) -> String {
    let local = date.with_timezone(&Local);
    format!(
        "{}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.year(),
        local.month(),
        local.day(),
        local.hour(),
        local.minute(),
        local.second()
    )
}

fn format_percent(value: f64) -> String {
    format!("{}%", format_decimal(value))
}

fn format_decimal(value: f64) -> String {
    let formatted = format!("{value:.2}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn formats_samples_csv_without_source_evidence() {
        let report = LimitSamplesReport {
            status: "ok".to_string(),
            start: time(2026, 5, 10, 0, 0),
            end: time(2026, 5, 10, 1, 0),
            sessions_dir: "/tmp/sessions".to_string(),
            samples: vec![RateLimitSample {
                timestamp: time(2026, 5, 10, 0, 1),
                session_id: "fixture-session".to_string(),
                account_id: Some("account-fixture".to_string()),
                plan_type: Some("pro".to_string()),
                limit_id: Some("fixture-limit".to_string()),
                window: "7d".to_string(),
                window_minutes: 10080,
                used_percent: 12.5,
                remaining_percent: 87.5,
                resets_at: time(2026, 5, 17, 0, 0),
                source: None,
            }],
            diagnostics: None,
        };

        let csv = format_limit_samples(&report, LimitFormat::Csv, false).expect("csv");

        assert!(csv.starts_with("timestamp,sessionId,accountId"));
        assert!(csv.contains("fixture-session"));
        assert!(csv.contains(&format_date_time(time(2026, 5, 10, 0, 1))));
        assert!(!csv.contains("2026-05-10T00:01:00Z"));
        assert!(!csv.contains("source"));
        assert!(!csv.contains("lineNumber"));
    }

    fn time(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid time")
    }
}
