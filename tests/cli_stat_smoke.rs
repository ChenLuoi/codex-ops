mod common;

use chrono::{Datelike, Duration, Local, NaiveDate};
use common::{
    assert_array, assert_contains, assert_failure_contains, assert_fast_candidate_row_schema,
    assert_fast_candidates_report_schema, assert_json_eq, assert_json_local_day_end,
    assert_json_local_day_start, assert_limit_usage_diagnostics_schema,
    assert_limit_usage_row_schema, assert_no_limit_source_leakage,
    assert_no_source_paths_by_default, assert_success, assert_usage_diagnostics_schema,
    assert_usage_totals_schema, fixed_now_utc, parse_csv, parse_json, run_codex_ops, Sandbox,
    FIXED_NOW,
};
use serde_json::Value;
use std::fs;

#[test]
fn stat_json_schema_and_account_attribution_are_stable() {
    let sandbox = Sandbox::new();

    let stat_json = run_codex_ops(
        [
            "stat",
            "--all",
            "--format",
            "json",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&stat_json, "stat all json schema");
    let stat = parse_json(&stat_json.stdout, "stat all json schema");
    common::assert_has_keys(
        &stat,
        &[
            "start",
            "end",
            "groupBy",
            "rows",
            "totals",
            "warnings",
            "diagnostics",
        ],
        "stat schema",
    );
    assert!(
        !assert_array(&stat["rows"], "stat rows").is_empty(),
        "stat rows should not be empty for all-usage fixture"
    );
    assert_usage_totals_schema(&stat["totals"], "stat totals");
    assert_usage_diagnostics_schema(&stat["diagnostics"], "stat diagnostics");
    assert_json_eq(&stat["totals"]["sessions"], 2, "stat total sessions");
    assert_json_eq(&stat["totals"]["calls"], 3, "stat total calls");
    assert_json_eq(
        &stat["totals"]["usage"]["totalTokens"],
        3600,
        "stat total tokens",
    );
    assert_json_eq(
        &stat["diagnostics"]["includedUsageEvents"],
        3,
        "stat included events",
    );

    let stat_account = run_codex_ops(
        [
            "stat",
            "--all",
            "--group-by",
            "account",
            "--format",
            "json",
            "--auth-file",
            sandbox.auth_file.to_str().unwrap(),
            "--account-history-file",
            sandbox.account_history_file.to_str().unwrap(),
            "--codex-home",
            sandbox.codex_home.to_str().unwrap(),
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&stat_account, "stat account json");
    let stat_account_json = parse_json(&stat_account.stdout, "stat account json");
    assert_json_eq(
        &stat_account_json["rows"][0]["key"],
        "account-fixture",
        "stat account row",
    );
}

#[test]
fn stat_prices_new_models_and_reports_spark_as_known_unpriced() {
    let sandbox = Sandbox::new();
    let sessions_dir = sandbox.home.join("new-model-sessions/2026/05/10");
    fs::create_dir_all(&sessions_dir).expect("create new model sessions dir");
    let models = [
        ("astra", "gpt-6-astra", "2026-05-10T09:00:00.000Z"),
        (
            "daybreak-blue",
            "gpt-daybreak-blue-latest",
            "2026-05-10T10:00:00.000Z",
        ),
        (
            "daybreak-red",
            "gpt-daybreak-red-latest",
            "2026-05-10T11:00:00.000Z",
        ),
        ("spark", "gpt-5.3-codex-spark", "2026-05-10T12:00:00.000Z"),
    ];

    for (session_id, model, timestamp) in models {
        let session_meta = serde_json::json!({
            "timestamp": timestamp,
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "model": model,
                "cwd": "/workspace/new-models"
            }
        });
        let turn_context = serde_json::json!({
            "timestamp": timestamp,
            "type": "turn_context",
            "payload": { "model": model }
        });
        let token_count = serde_json::json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": 1_000_000,
                        "cached_input_tokens": 0,
                        "output_tokens": 0,
                        "reasoning_output_tokens": 0,
                        "total_tokens": 1_000_000
                    },
                    "total_token_usage": {
                        "input_tokens": 1_000_000,
                        "cached_input_tokens": 0,
                        "output_tokens": 0,
                        "reasoning_output_tokens": 0,
                        "total_tokens": 1_000_000
                    }
                }
            }
        });
        fs::write(
            sessions_dir.join(format!("rollout-{session_id}.jsonl")),
            [session_meta, turn_context, token_count]
                .into_iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .expect("write new model session");
    }

    let root_sessions_dir = sandbox.home.join("new-model-sessions");
    let result = run_codex_ops(
        [
            "stat",
            "--all",
            "--group-by",
            "model",
            "--json",
            "--sessions-dir",
            root_sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&result, "stat new models json");
    let report = parse_json(&result.stdout, "stat new models json");
    let rows = assert_array(&report["rows"], "new model rows");

    for (model, expected_credits) in [
        ("gpt-6-astra", 250.0),
        ("gpt-daybreak-blue-latest", 100.0),
        ("gpt-daybreak-red-latest", 312.5),
    ] {
        let row = rows
            .iter()
            .find(|row| row["key"] == model)
            .unwrap_or_else(|| panic!("missing priced row for {model}: {rows:?}"));
        assert_json_f64(&row["credits"], expected_credits, model);
        assert_json_eq(&row["pricedCalls"], 1, &format!("{model} priced calls"));
        assert_json_eq(&row["unpricedCalls"], 0, &format!("{model} unpriced calls"));
    }

    let spark = rows
        .iter()
        .find(|row| row["key"] == "gpt-5.3-codex-spark")
        .expect("Spark model row");
    assert_json_f64(&spark["credits"], 0.0, "Spark credits");
    assert_json_eq(&spark["pricedCalls"], 0, "Spark priced calls");
    assert_json_eq(&spark["unpricedCalls"], 1, "Spark unpriced calls");

    assert_json_f64(
        &report["totals"]["credits"],
        662.5,
        "new model total credits",
    );
    assert_json_eq(
        &report["totals"]["pricedCalls"],
        3,
        "new model priced calls",
    );
    assert_json_eq(
        &report["totals"]["unpricedCalls"],
        1,
        "new model unpriced calls",
    );
    let unpriced = assert_array(&report["unpricedModels"], "new model unpriced models");
    assert_eq!(unpriced.len(), 1);
    assert_json_eq(
        &unpriced[0]["pricingKey"],
        "gpt-5.3-codex-spark",
        "Spark pricing key",
    );
    assert_json_eq(
        &unpriced[0]["note"],
        "research preview; uses a separate usage limit and has no token credit rate",
        "Spark unpriced note",
    );
}

#[test]
fn stat_time_ranges_use_fixed_now_and_local_date_bounds() {
    let sandbox = Sandbox::new();

    let stat_last = run_codex_ops(
        [
            "stat",
            "--last",
            "12h",
            "--format",
            "json",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&stat_last, "stat fixed-now range");
    let stat_last_json = parse_json(&stat_last.stdout, "stat fixed-now range");
    assert_json_eq(
        &stat_last_json["start"],
        "2026-05-16T12:00:00.000Z",
        "stat fixed-now start",
    );
    assert_json_eq(&stat_last_json["end"], FIXED_NOW, "stat fixed-now end");
    assert_json_eq(&stat_last_json["groupBy"], "hour", "stat fixed-now group");

    let fixed_local_date = fixed_now_utc().with_timezone(&Local).date_naive();
    let stat_today = run_codex_ops(
        [
            "stat",
            "--today",
            "--format",
            "json",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&stat_today, "stat today fixed-now range");
    let today = parse_json(&stat_today.stdout, "stat today fixed-now range");
    assert_json_local_day_start(&today["start"], fixed_local_date, "stat today start");
    assert_json_eq(&today["end"], FIXED_NOW, "stat today end");

    let stat_yesterday = run_codex_ops(
        [
            "stat",
            "--yesterday",
            "--format",
            "json",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&stat_yesterday, "stat yesterday fixed-now range");
    let yesterday = parse_json(&stat_yesterday.stdout, "stat yesterday fixed-now range");
    assert_json_local_day_start(
        &yesterday["start"],
        fixed_local_date - Duration::days(1),
        "stat yesterday start",
    );
    assert_json_local_day_end(
        &yesterday["end"],
        fixed_local_date - Duration::days(1),
        "stat yesterday end",
    );

    let stat_month = run_codex_ops(
        [
            "stat",
            "--month",
            "--format",
            "json",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&stat_month, "stat month fixed-now range");
    let month = parse_json(&stat_month.stdout, "stat month fixed-now range");
    assert_json_local_day_start(
        &month["start"],
        NaiveDate::from_ymd_opt(fixed_local_date.year(), fixed_local_date.month(), 1)
            .expect("month start"),
        "stat month start",
    );
    assert_json_eq(&month["end"], FIXED_NOW, "stat month end");
    assert_json_eq(&month["groupBy"], "day", "stat month group");

    let stat_explicit = run_codex_ops(
        [
            "stat",
            "--start",
            "2026-05-10",
            "--end=2026-05-11",
            "--format",
            "json",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&stat_explicit, "stat explicit range");
    let explicit = parse_json(&stat_explicit.stdout, "stat explicit range");
    assert_json_local_day_start(
        &explicit["start"],
        NaiveDate::from_ymd_opt(2026, 5, 10).expect("explicit start"),
        "stat explicit start",
    );
    assert_json_local_day_end(
        &explicit["end"],
        NaiveDate::from_ymd_opt(2026, 5, 11).expect("explicit end"),
        "stat explicit end",
    );

    let invalid_date = run_codex_ops(
        [
            "stat",
            "--start",
            "2026-02-31",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_failure_contains(
        &invalid_date,
        2,
        "Invalid start time: 2026-02-31",
        "stat invalid date",
    );

    let huge_last = run_codex_ops(
        [
            "stat",
            "--last",
            "9223372036854775807d",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_failure_contains(
        &huge_last,
        2,
        "Invalid --last value. Duration is too large.",
        "stat huge last",
    );
}

#[test]
fn stat_format_outputs_and_sessions_view_remain_stable() {
    let sandbox = Sandbox::new();

    let stat_table = run_codex_ops(
        [
            "stat",
            "--all",
            "--format",
            "table",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&stat_table, "stat table");
    assert_contains(&stat_table.stdout, "Codex usage", "stat table title");
    assert_contains(&stat_table.stdout, "Total", "stat table total");
    assert_contains(&stat_table.stdout, "3,600", "stat table total tokens");

    let stat_csv = run_codex_ops(
        [
            "stat",
            "--all",
            "--format",
            "csv",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&stat_csv, "stat csv");
    let csv_rows = parse_csv(&stat_csv.stdout);
    assert_eq!(
        csv_rows[0].join(","),
        "Group,Sessions,Calls,Input,Cached,Output,Reasoning,Total,Credits,USD",
        "stat csv header"
    );
    assert_eq!(csv_rows.last().unwrap()[0], "Total", "stat csv total row");

    let stat_markdown = run_codex_ops(
        [
            "stat",
            "--all",
            "--format",
            "markdown",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&stat_markdown, "stat markdown");
    assert_contains(
        &stat_markdown.stdout,
        "| Group | Sessions | Calls |",
        "stat markdown header",
    );
    assert_contains(
        &stat_markdown.stdout,
        "| Total | 2 | 3 |",
        "stat markdown total",
    );

    let stat_sessions = run_codex_ops(
        [
            "stat",
            "sessions",
            "--all",
            "--top",
            "10",
            "--format",
            "json",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&stat_sessions, "stat sessions json");
    let sessions = parse_json(&stat_sessions.stdout, "stat sessions json");
    assert_eq!(
        sessions["rows"].as_array().unwrap().len(),
        2,
        "stat sessions rows"
    );
    assert_json_eq(
        &sessions["rows"][0]["sessionId"],
        "rust-run-session-alpha",
        "stat sessions first id",
    );
    assert_json_eq(
        &sessions["totals"]["usage"]["totalTokens"],
        3600,
        "stat sessions total tokens",
    );
}

#[test]
fn stat_uses_usage_mode_history_for_fast_credit_attribution() {
    let sandbox = Sandbox::new();
    let history_file = sandbox.fixture_usage_mode_history_file.to_str().unwrap();

    let stat = run_codex_ops(
        [
            "stat",
            "--all",
            "--json",
            "--verbose",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
            "--usage-mode-history-file",
            history_file,
        ],
        &sandbox,
    );
    assert_success(&stat, "stat fast history json");
    let report = parse_json(&stat.stdout, "stat fast history json");
    assert_json_eq(
        &report["totals"]["usage"]["totalTokens"],
        3600,
        "fast history leaves tokens unchanged",
    );
    assert_json_f64(&report["totals"]["credits"], 2.092188, "fast stat credits");
    let mode_history = &report["diagnostics"]["modeHistory"];
    assert_json_eq(
        &mode_history["historyFile"],
        history_file,
        "verbose json mode history path",
    );
    assert_json_eq(
        &mode_history["historyPresent"],
        true,
        "mode history present",
    );
    assert_json_eq(&mode_history["switchCount"], 2, "mode switch count");
    assert_json_eq(
        &mode_history["fastAttributedCalls"],
        2,
        "fast attributed calls",
    );
    assert_json_f64(
        &mode_history["fastAttributedCredits"],
        2.092188,
        "fast attributed credits",
    );

    let model_group = run_codex_ops(
        [
            "stat",
            "--all",
            "--group-by",
            "model",
            "--json",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
            "--usage-mode-history-file",
            history_file,
        ],
        &sandbox,
    );
    assert_success(&model_group, "stat fast history model group json");
    let model_report = parse_json(&model_group.stdout, "stat fast history model group json");
    let model_rows = assert_array(&model_report["rows"], "fast model rows");
    assert!(
        model_rows
            .iter()
            .any(|row| row["key"].as_str() == Some("gpt-5.5-fast")),
        "fast usage should be grouped under a distinct model key"
    );
    assert!(
        !model_rows
            .iter()
            .any(|row| row["key"].as_str() == Some("gpt-5.5")),
        "fast-only gpt-5.5 usage should not be merged into the normal model key"
    );

    let default_json = run_codex_ops(
        [
            "stat",
            "--all",
            "--json",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
            "--usage-mode-history-file",
            history_file,
        ],
        &sandbox,
    );
    assert_success(&default_json, "stat fast history default json");
    let default_report = parse_json(&default_json.stdout, "stat fast history default json");
    let default_mode_history = default_report["diagnostics"]["modeHistory"]
        .as_object()
        .expect("modeHistory object");
    assert!(
        !default_mode_history.contains_key("historyFile"),
        "default JSON diagnostics should not include usage mode history path"
    );

    let detail = run_codex_ops(
        [
            "stat",
            "sessions",
            "rust-run-session-alpha",
            "--all",
            "--detail",
            "--json",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
            "--usage-mode-history-file",
            history_file,
        ],
        &sandbox,
    );
    assert_success(&detail, "stat session detail fast history json");
    let detail_report = parse_json(&detail.stdout, "stat session detail fast history json");
    assert_json_f64(
        &detail_report["summary"]["credits"],
        2.092188,
        "fast session summary credits",
    );
    for row in assert_array(&detail_report["rows"], "fast detail rows") {
        assert_json_eq(&row["usageMode"], "fast", "fast detail row mode");
    }

    let limit_window = run_codex_ops(
        [
            "stat",
            "--all",
            "--limit-window",
            "7d",
            "--json",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
            "--account-history-file",
            sandbox.account_history_file.to_str().unwrap(),
            "--usage-mode-history-file",
            history_file,
        ],
        &sandbox,
    );
    assert_success(&limit_window, "stat limit-window fast history json");
    let limit_report = parse_json(&limit_window.stdout, "stat limit-window fast history json");
    let first_row = &assert_array(&limit_report["rows"], "fast limit rows")[0];
    assert_json_eq(&first_row["calls"], 2, "fast limit row calls");
    assert_json_eq(
        &first_row["usage"]["totalTokens"],
        3100,
        "fast limit row tokens",
    );
    assert_json_f64(&first_row["credits"], 2.092188, "fast limit row credits");
    assert_json_f64(
        &limit_report["diagnostics"]["usage"]["modeHistory"]["fastAttributedCredits"],
        2.092188,
        "fast limit diagnostics credits",
    );

    for format in ["table", "csv", "markdown"] {
        let result = run_codex_ops(
            [
                "stat",
                "--all",
                "--format",
                format,
                "--verbose",
                "--sessions-dir",
                sandbox.sessions_dir.to_str().unwrap(),
                "--usage-mode-history-file",
                history_file,
            ],
            &sandbox,
        );
        assert_success(&result, &format!("stat fast history {format}"));
        assert!(
            !result.stdout.contains(history_file),
            "stat fast history {format}: output leaked usage mode history path"
        );
    }
}

#[test]
fn fast_candidates_outputs_detection_only_candidates() {
    let sandbox = Sandbox::new();
    let sessions_dir = &sandbox.fast_candidate_sessions_dir;
    let session_file =
        sessions_dir.join("2026/05/10/rollout-2026-05-10T00-00-00-fast-candidates.jsonl");

    let json = run_codex_ops(
        [
            "fast",
            "candidates",
            "--start",
            "2026-05-10T00:00:00Z",
            "--end",
            "2026-05-10T04:00:00Z",
            "--json",
            "--sessions-dir",
            sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&json, "fast candidates json");
    assert!(
        !json.stdout.contains(session_file.to_str().unwrap()),
        "fast candidates default JSON leaked source path"
    );
    let report = parse_json(&json.stdout, "fast candidates json");
    assert_fast_candidates_report_schema(&report, "fast-candidates report");
    assert_json_eq(
        &report["detectionOnly"],
        true,
        "fast-candidates detectionOnly",
    );
    assert_json_eq(&report["window"], "5h", "fast-candidates window");
    assert_contains(
        report["warnings"][0].as_str().unwrap_or_default(),
        "Detection-only",
        "fast-candidates JSON detection note",
    );
    assert_no_source_paths_by_default(&report, "fast candidates default json");

    let candidates = assert_array(&report["candidates"], "fast candidates");
    assert_eq!(candidates.len(), 1, "fast candidate count");
    for (index, candidate) in candidates.iter().enumerate() {
        assert_fast_candidate_row_schema(candidate, &format!("fast candidate row {index}"));
    }
    let candidate = &candidates[0];
    assert_json_eq(
        &candidate["sessionId"],
        "fast-candidate",
        "fast-candidate session id",
    );
    assert_json_eq(&candidate["calls"], 6, "fast-candidate calls");
    assert_json_eq(&candidate["samplePairs"], 6, "fast-candidate sample pairs");
    assert_json_f64(
        &candidate["effectiveMultiplier"],
        2.25,
        "fast-candidate effective multiplier",
    );
    assert_json_f64(
        &candidate["expectedFastMultiplier"],
        2.25,
        "fast-candidate expected multiplier",
    );
    assert_json_eq(
        &candidate["reason"],
        "mixedModelsWeightedExpectedMultiplier",
        "fast-candidate mixed reason",
    );
    assert_json_eq(
        &candidate["confidence"],
        "high",
        "fast-candidate confidence",
    );
    assert_contains(
        candidate["suggestedFastOnCommand"]
            .as_str()
            .unwrap_or_default(),
        "codex-ops fast on --at",
        "fast-candidates on command",
    );
    assert_contains(
        candidate["suggestedFastOffCommand"]
            .as_str()
            .unwrap_or_default(),
        "codex-ops fast off --at",
        "fast-candidates off command",
    );
    assert_json_eq(
        &candidate["segmentEnd"],
        "2026-05-10T02:45:00.000Z",
        "fast-candidate segment end",
    );
    assert_json_eq(
        &candidate["suggestedFastOffCommand"],
        "codex-ops fast off --at 2026-05-10T02:45:00.001Z",
        "fast-candidate off command is exclusive",
    );

    let verbose_json = run_codex_ops(
        [
            "fast",
            "candidates",
            "--start",
            "2026-05-10T00:00:00Z",
            "--end",
            "2026-05-10T04:00:00Z",
            "--json",
            "--verbose",
            "--sessions-dir",
            sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&verbose_json, "fast candidates verbose json");
    assert_contains(
        &verbose_json.stdout,
        session_file.to_str().unwrap(),
        "verbose fast-candidates JSON includes source path",
    );

    for format in ["table", "csv", "markdown"] {
        let result = run_codex_ops(
            [
                "fast",
                "candidates",
                "--start",
                "2026-05-10T00:00:00Z",
                "--end",
                "2026-05-10T04:00:00Z",
                "--format",
                format,
                "--sessions-dir",
                sessions_dir.to_str().unwrap(),
            ],
            &sandbox,
        );
        assert_success(&result, &format!("fast candidates {format}"));
        assert_contains(
            &result.stdout,
            "Detection-only",
            &format!("fast candidates {format} detection note"),
        );
        assert_contains(
            &result.stdout,
            "candidate",
            &format!("fast candidates {format} candidate label"),
        );
        assert_contains(
            &result.stdout,
            "fast on",
            &format!("fast candidates {format} command hint"),
        );
        assert!(
            !result.stdout.contains(session_file.to_str().unwrap()),
            "fast candidates {format} leaked source path"
        );
    }

    let table = run_codex_ops(
        [
            "fast",
            "candidates",
            "--start",
            "2026-05-10T00:00:00Z",
            "--end",
            "2026-05-10T04:00:00Z",
            "--sessions-dir",
            sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&table, "fast candidates table default");
    assert_contains(
        &table.stdout,
        "fast off --at 2026-05-10T02:45:00.001Z",
        "fast candidates table exclusive off command",
    );
    for header in [
        "Segment End",
        "Session",
        "Model",
        "Calls",
        "Delta%",
        "Multiplier",
        "Guess",
        "Confidence",
    ] {
        assert_contains(&table.stdout, header, "fast-candidates table header");
    }

    let singular_alias = run_codex_ops(
        [
            "fast",
            "candidate",
            "--start",
            "2026-05-10T00:00:00Z",
            "--end",
            "2026-05-10T04:00:00Z",
            "--json",
            "--sessions-dir",
            sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&singular_alias, "fast candidate alias json");
}

#[test]
fn fast_candidates_account_filter_populates_missing_account_history() {
    let sandbox = Sandbox::new();
    fs::remove_file(&sandbox.account_history_file).expect("remove account history fixture");

    let json = run_codex_ops(
        [
            "fast",
            "candidates",
            "--start",
            "2026-05-10T00:00:00Z",
            "--end",
            "2026-05-10T04:00:00Z",
            "--account-id",
            "account-fixture",
            "--json",
            "--sessions-dir",
            sandbox.fast_candidate_sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );

    assert_success(&json, "fast candidates account-filter json");
    assert!(
        sandbox.account_history_file.exists(),
        "fast-candidates account filter should populate missing account history"
    );
    let report = parse_json(&json.stdout, "fast candidates account-filter json");
    let candidates = assert_array(&report["candidates"], "account-filter fast candidates");
    assert_eq!(candidates.len(), 1, "account-filter fast candidate count");
    assert_json_eq(
        &candidates[0]["accountId"],
        "account-fixture",
        "account-filter candidate account id",
    );
}

#[test]
fn fast_candidates_rejects_window_and_session_inputs() {
    let sandbox = Sandbox::new();

    let limit_window = run_codex_ops(["fast", "candidates", "--limit-window", "5h"], &sandbox);
    assert_failure_contains(
        &limit_window,
        2,
        "unexpected argument '--limit-window'",
        "fast candidates rejects limit-window",
    );

    let session = run_codex_ops(["fast", "candidates", "session-id"], &sandbox);
    assert_failure_contains(
        &session,
        2,
        "unexpected argument 'session-id'",
        "fast candidates rejects session id",
    );
}

#[test]
fn stat_limit_window_usage_outputs_real_window_rows() {
    let sandbox = Sandbox::new();

    let weekly = run_codex_ops(
        [
            "stat",
            "--all",
            "--limit-window",
            "7d",
            "--json",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
            "--account-history-file",
            sandbox.account_history_file.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&weekly, "stat limit-window 7d json");
    assert_no_limit_source_leakage(&weekly.stdout, &sandbox, "stat limit-window 7d json");
    let report = parse_json(&weekly.stdout, "stat limit-window 7d json");
    common::assert_has_keys(
        &report,
        &[
            "start",
            "end",
            "limitWindow",
            "windowMinutes",
            "groupBy",
            "rows",
            "totals",
            "warnings",
            "diagnostics",
        ],
        "limit usage report",
    );
    assert_json_eq(&report["limitWindow"], "7d", "limit usage window");
    assert_json_eq(&report["windowMinutes"], 10080, "limit usage minutes");
    assert_json_eq(&report["groupBy"], "window", "limit usage group");
    let rows = assert_array(&report["rows"], "limit usage rows");
    assert_eq!(rows.len(), 3);
    for (index, row) in rows.iter().enumerate() {
        assert_limit_usage_row_schema(row, &format!("limit usage row {index}"));
    }
    assert_json_eq(&rows[0]["resetAt"], "2026-05-11T09:00:00Z", "first reset");
    assert_json_eq(&rows[0]["observed"], true, "first observed");
    assert_json_eq(&rows[0]["calls"], 2, "first calls");
    assert_json_eq(&rows[0]["usage"]["totalTokens"], 3100, "first total tokens");
    assert_json_eq(&rows[2]["calls"], 0, "empty observed window calls");
    assert_json_eq(
        &report["totals"]["usage"]["totalTokens"],
        3600,
        "limit usage total tokens",
    );
    assert_limit_usage_diagnostics_schema(&report["diagnostics"], "limit usage diagnostics");
    assert_json_eq(
        &report["diagnostics"]["observedWindows"],
        3,
        "limit usage observed windows",
    );
    assert_json_eq(
        &report["diagnostics"]["unobservedUsageEvents"],
        0,
        "limit usage unobserved count",
    );
    assert_no_source_paths_by_default(&report, "stat limit-window 7d json");

    let five_hour = run_codex_ops(
        [
            "stat",
            "--all",
            "--limit-window",
            "5h",
            "--json",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
            "--account-history-file",
            sandbox.account_history_file.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&five_hour, "stat limit-window 5h json");
    let five_hour_report = parse_json(&five_hour.stdout, "stat limit-window 5h json");
    assert_json_eq(&five_hour_report["limitWindow"], "5h", "5h window");
    assert!(assert_array(&five_hour_report["rows"], "5h rows")
        .iter()
        .any(|row| row["window"] == "5h" && row["calls"] == 2));

    for group in ["model", "cwd", "account"] {
        let grouped = run_codex_ops(
            [
                "stat",
                "--all",
                "--limit-window",
                "7d",
                "--group-by",
                group,
                "--json",
                "--sessions-dir",
                sandbox.sessions_dir.to_str().unwrap(),
                "--account-history-file",
                sandbox.account_history_file.to_str().unwrap(),
            ],
            &sandbox,
        );
        assert_success(&grouped, &format!("stat limit-window group {group}"));
        let grouped_report = parse_json(&grouped.stdout, &format!("grouped {group}"));
        assert_json_eq(&grouped_report["groupBy"], group, "grouped groupBy");
        let grouped_rows = assert_array(&grouped_report["rows"], "grouped rows");
        assert!(!grouped_rows.is_empty(), "grouped rows should not be empty");
        assert!(grouped_rows.iter().all(|row| row["groupBy"] == group));
    }

    let csv = run_codex_ops(
        [
            "stat",
            "--all",
            "--limit-window",
            "7d",
            "--format",
            "csv",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
            "--account-history-file",
            sandbox.account_history_file.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&csv, "stat limit-window csv");
    let csv_rows = parse_csv(&csv.stdout);
    assert_eq!(csv_rows[0][0], "Window", "limit usage csv first header");
    assert!(
        !csv_rows[0].iter().any(|header| header == "Window ID"),
        "limit usage csv should omit window id"
    );
    assert!(
        !csv_rows[0].iter().any(|header| header == "Group key"),
        "limit usage csv should omit group key"
    );
    assert_eq!(
        csv_rows.last().unwrap()[0],
        "Total",
        "limit usage csv total"
    );

    let markdown = run_codex_ops(
        [
            "stat",
            "--all",
            "--limit-window",
            "7d",
            "--format",
            "markdown",
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
            "--account-history-file",
            sandbox.account_history_file.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&markdown, "stat limit-window markdown");
    assert_contains(
        &markdown.stdout,
        "| Window | Account | Plan | Limit | Window start |",
        "limit usage markdown header",
    );
    assert!(
        !markdown.stdout.contains("Window ID"),
        "limit usage markdown should omit window id"
    );
    assert!(
        !markdown.stdout.contains("Group key"),
        "limit usage markdown should omit group key"
    );
}

fn assert_json_f64(value: &Value, expected: f64, label: &str) {
    let actual = value
        .as_f64()
        .unwrap_or_else(|| panic!("{label}: expected JSON number, got {value}"));
    assert!(
        (actual - expected).abs() < 0.000001,
        "{label}: expected {expected}, got {actual}"
    );
}

#[test]
fn stat_limit_window_prefers_newer_overlap_and_shapes_rows() {
    let sandbox = Sandbox::new();
    let sessions_dir = sandbox.home.join("early-reset-sessions/2026/05/10");
    fs::create_dir_all(&sessions_dir).expect("create early reset sessions dir");
    fs::write(
        sessions_dir.join("rollout-2026-05-10T09-00-00-early-reset.jsonl"),
        [
            r#"{"timestamp":"2026-05-10T09:00:00.000Z","type":"session_meta","payload":{"id":"early-reset","model":"gpt-5.5","cwd":"/workspace/early-reset","reasoning_effort":"medium"}}"#,
            r#"{"timestamp":"2026-05-10T09:00:01.000Z","type":"event_msg","payload":{"rate_limits":{"primary":{"window_minutes":300,"used_percent":40.0,"resets_at":1778421600},"secondary":{"window_minutes":10080,"used_percent":80.0,"resets_at":1779008400},"plan_type":"pro","limit_id":"early-reset-limit"}}}"#,
            r#"{"timestamp":"2026-05-10T09:05:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":80,"cached_input_tokens":0,"output_tokens":20,"reasoning_output_tokens":0,"total_tokens":100},"total_token_usage":{"input_tokens":80,"cached_input_tokens":0,"output_tokens":20,"reasoning_output_tokens":0,"total_tokens":100}},"rate_limits":{"primary":{"window_minutes":300,"used_percent":40.0,"resets_at":1778421600},"secondary":{"window_minutes":10080,"used_percent":80.0,"resets_at":1779008400},"plan_type":"pro","limit_id":"early-reset-limit"}}}"#,
            r#"{"timestamp":"2026-05-12T12:00:00.000Z","type":"event_msg","payload":{"rate_limits":{"primary":{"window_minutes":300,"used_percent":5.0,"resets_at":1778605200},"secondary":{"window_minutes":10080,"used_percent":4.0,"resets_at":1779192000},"plan_type":"pro","limit_id":"early-reset-limit"}}}"#,
            r#"{"timestamp":"2026-05-12T12:05:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":160,"cached_input_tokens":0,"output_tokens":40,"reasoning_output_tokens":0,"total_tokens":200},"total_token_usage":{"input_tokens":240,"cached_input_tokens":0,"output_tokens":60,"reasoning_output_tokens":0,"total_tokens":300}},"rate_limits":{"primary":{"window_minutes":300,"used_percent":5.0,"resets_at":1778605200},"secondary":{"window_minutes":10080,"used_percent":4.0,"resets_at":1779192000},"plan_type":"pro","limit_id":"early-reset-limit"}}}"#,
        ]
        .join("\n"),
    )
    .expect("write early reset session");
    let root_sessions_dir = sandbox.home.join("early-reset-sessions");

    let default = run_codex_ops(
        [
            "stat",
            "--all",
            "--limit-window",
            "7d",
            "--json",
            "--sessions-dir",
            root_sessions_dir.to_str().unwrap(),
            "--account-history-file",
            sandbox.account_history_file.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&default, "stat limit-window early reset json");
    let report = parse_json(&default.stdout, "stat limit-window early reset json");
    let rows = assert_array(&report["rows"], "early reset rows");
    assert_eq!(rows.len(), 2);
    assert_json_eq(&rows[0]["resetAt"], "2026-05-17T09:00:00Z", "old reset");
    assert_json_eq(&rows[0]["calls"], 1, "old window calls");
    assert_json_eq(&rows[0]["usage"]["totalTokens"], 100, "old window tokens");
    assert_json_eq(&rows[1]["resetAt"], "2026-05-19T12:00:00Z", "new reset");
    assert_json_eq(&rows[1]["calls"], 1, "new window calls");
    assert_json_eq(&rows[1]["usage"]["totalTokens"], 200, "new window tokens");

    let shaped = run_codex_ops(
        [
            "stat",
            "--all",
            "--limit-window",
            "7d",
            "--sort",
            "tokens",
            "--limit",
            "1",
            "--json",
            "--sessions-dir",
            root_sessions_dir.to_str().unwrap(),
            "--account-history-file",
            sandbox.account_history_file.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&shaped, "stat limit-window sort limit json");
    let shaped_report = parse_json(&shaped.stdout, "stat limit-window sort limit json");
    assert_json_eq(&shaped_report["sortBy"], "tokens", "limit usage sort");
    assert_json_eq(&shaped_report["limit"], 1, "limit usage limit");
    let shaped_rows = assert_array(&shaped_report["rows"], "shaped rows");
    assert_eq!(shaped_rows.len(), 1);
    assert_json_eq(
        &shaped_rows[0]["resetAt"],
        "2026-05-19T12:00:00Z",
        "highest token row reset",
    );
    assert_json_eq(
        &shaped_rows[0]["usage"]["totalTokens"],
        200,
        "highest token row usage",
    );
}

#[test]
fn stat_limit_window_uses_pre_start_limit_sample_for_attribution() {
    let sandbox = Sandbox::new();
    let sessions_dir = sandbox.home.join("pre-start-limit-sessions/2026/05/12");
    fs::create_dir_all(&sessions_dir).expect("create pre-start limit sessions dir");
    fs::write(
        sessions_dir.join("rollout-2026-05-12T08-59-00-pre-start-limit.jsonl"),
        [
            r#"{"timestamp":"2026-05-12T08:58:00.000Z","type":"session_meta","payload":{"id":"pre-start-limit","model":"gpt-5.5","cwd":"/workspace/pre-start-limit","reasoning_effort":"medium"}}"#,
            r#"{"timestamp":"2026-05-12T08:59:00.000Z","type":"event_msg","payload":{"rate_limits":{"primary":{"window_minutes":300,"used_percent":3.0,"resets_at":1778489940},"secondary":{"window_minutes":10080,"used_percent":11.0,"resets_at":1779181140},"plan_type":"pro","limit_id":"pre-start-limit"}}}"#,
            r#"{"timestamp":"2026-05-12T09:30:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":30,"cached_input_tokens":2,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":42},"total_token_usage":{"input_tokens":30,"cached_input_tokens":2,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":42}}}}"#,
        ]
        .join("\n"),
    )
    .expect("write pre-start limit session");
    let root_sessions_dir = sandbox.home.join("pre-start-limit-sessions");

    let result = run_codex_ops(
        [
            "stat",
            "--start",
            "2026-05-12T09:00:00Z",
            "--end",
            "2026-05-12T10:00:00Z",
            "--limit-window",
            "7d",
            "--json",
            "--sessions-dir",
            root_sessions_dir.to_str().unwrap(),
            "--account-history-file",
            sandbox.account_history_file.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&result, "stat limit-window pre-start sample json");
    let report = parse_json(&result.stdout, "stat limit-window pre-start sample json");
    let rows = assert_array(&report["rows"], "pre-start rows");
    assert_eq!(rows.len(), 1);
    assert_limit_usage_row_schema(&rows[0], "pre-start row");
    assert_json_eq(&rows[0]["observed"], true, "pre-start observed flag");
    assert_json_eq(
        &rows[0]["resetAt"],
        "2026-05-19T08:59:00Z",
        "pre-start reset",
    );
    assert_json_eq(&rows[0]["calls"], 1, "pre-start calls");
    assert_json_eq(
        &rows[0]["usage"]["totalTokens"],
        42,
        "pre-start total tokens",
    );
    assert_json_eq(
        &report["diagnostics"]["observedWindows"],
        1,
        "pre-start observed windows",
    );
    assert_json_eq(
        &report["diagnostics"]["unobservedUsageEvents"],
        0,
        "pre-start unobserved usage events",
    );
}

#[test]
fn stat_limit_window_without_samples_reports_unobserved_usage() {
    let sandbox = Sandbox::new();
    let sessions_dir = sandbox.home.join("token-only-sessions/2026/05/13");
    fs::create_dir_all(&sessions_dir).expect("create token-only sessions dir");
    fs::write(
        sessions_dir.join("rollout-2026-05-13T09-00-00-token-only.jsonl"),
        [
            r#"{"timestamp":"2026-05-13T09:00:00.000Z","type":"session_meta","payload":{"id":"token-only","model":"gpt-5.5","cwd":"/workspace/token-only","reasoning_effort":"high"}}"#,
            r#"{"timestamp":"2026-05-13T09:00:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"cached_input_tokens":1,"output_tokens":2,"reasoning_output_tokens":0,"total_tokens":12},"total_token_usage":{"input_tokens":10,"cached_input_tokens":1,"output_tokens":2,"reasoning_output_tokens":0,"total_tokens":12}}}}"#,
        ]
        .join("\n"),
    )
    .expect("write token-only session");
    let root_sessions_dir = sandbox.home.join("token-only-sessions");

    let result = run_codex_ops(
        [
            "stat",
            "--all",
            "--limit-window",
            "7d",
            "--json",
            "--sessions-dir",
            root_sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&result, "stat limit-window unobserved json");
    let report = parse_json(&result.stdout, "stat limit-window unobserved json");
    let rows = assert_array(&report["rows"], "unobserved rows");
    assert_eq!(rows.len(), 1);
    assert_limit_usage_row_schema(&rows[0], "unobserved row");
    assert_json_eq(&rows[0]["windowId"], "unobserved:7d", "unobserved id");
    assert_json_eq(&rows[0]["observed"], false, "unobserved flag");
    assert_json_eq(&rows[0]["groupKey"], "unobserved", "unobserved group");
    assert_json_eq(&rows[0]["calls"], 1, "unobserved calls");
    assert_json_eq(
        &rows[0]["usage"]["totalTokens"],
        12,
        "unobserved total tokens",
    );
    assert_json_eq(
        &report["diagnostics"]["observedWindows"],
        0,
        "unobserved windows",
    );
    assert_json_eq(
        &report["diagnostics"]["unobservedUsageEvents"],
        1,
        "unobserved usage events",
    );
}
