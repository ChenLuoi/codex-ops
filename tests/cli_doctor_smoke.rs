mod common;

use common::{
    assert_check_status, assert_contains, assert_json_eq, assert_no_secrets, assert_not_contains,
    assert_success, parse_json, run_codex_ops, Sandbox,
};
use std::fs;

#[test]
fn doctor_json_reports_core_checks_and_pricing_source() {
    let sandbox = Sandbox::new();

    let doctor = run_codex_ops(
        [
            "doctor",
            "--auth-file",
            sandbox.auth_file.to_str().unwrap(),
            "--codex-home",
            sandbox.codex_home.to_str().unwrap(),
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
            "--json",
        ],
        &sandbox,
    );
    assert_success(&doctor, "doctor json");
    assert_no_secrets(&doctor, "doctor json");
    let doctor_json = parse_json(&doctor.stdout, "doctor json");
    assert_json_eq(&doctor_json["summary"]["errors"], 0, "doctor errors");
    assert_json_eq(&doctor_json["summary"]["warnings"], 0, "doctor warnings");
    assert!(
        !doctor_json
            .as_object()
            .expect("doctor json object")
            .contains_key("cycleFile"),
        "doctor json should not include cycleFile: {doctor_json}"
    );
    assert_check_status(&doctor_json, "Auth file", "ok");
    assert_check_status(&doctor_json, "Recent usage", "ok");
    assert_check_status(&doctor_json, "Recent rate limits", "ok");
    assert_check_status(&doctor_json, "Pricing", "ok");

    let rate_limits = doctor_json["checks"]
        .as_array()
        .expect("doctor checks array")
        .iter()
        .find(|check| check["name"] == "Recent rate limits")
        .expect("recent rate limits check");
    let rate_limit_details = rate_limits["details"]
        .as_array()
        .expect("recent rate limits details");
    assert!(
        rate_limit_details
            .iter()
            .any(|detail| detail == "Samples: 11"),
        "rate-limit sample count detail missing: {rate_limit_details:?}"
    );
    assert!(
        rate_limit_details
            .iter()
            .any(|detail| detail == "5h samples: 6"),
        "5h sample count detail missing: {rate_limit_details:?}"
    );
    assert!(
        rate_limit_details
            .iter()
            .any(|detail| detail == "7d samples: 5"),
        "7d sample count detail missing: {rate_limit_details:?}"
    );
    assert!(
        rate_limit_details
            .iter()
            .any(|detail| detail == "Latest observed at: 2026-05-12T13:05:00.000Z"),
        "latest observed detail missing: {rate_limit_details:?}"
    );

    let pricing = doctor_json["checks"]
        .as_array()
        .expect("doctor checks array")
        .iter()
        .find(|check| check["name"] == "Pricing")
        .expect("pricing check");
    let details = pricing["details"].as_array().expect("pricing details");
    assert!(
        details
            .iter()
            .any(|detail| detail == "Source: OpenAI Help Center Codex rate card"),
        "pricing source detail missing: {details:?}"
    );
    assert!(
        details.iter().any(|detail| detail == "Checked: 2026-08-04"),
        "pricing checked_at detail missing: {details:?}"
    );
    assert!(
        details
            .iter()
            .any(|detail| detail == "Credits: 25 credits = $1"),
        "pricing credit conversion detail missing: {details:?}"
    );

    let doctor_table = run_codex_ops(
        [
            "doctor",
            "--auth-file",
            sandbox.auth_file.to_str().unwrap(),
            "--codex-home",
            sandbox.codex_home.to_str().unwrap(),
            "--sessions-dir",
            sandbox.sessions_dir.to_str().unwrap(),
        ],
        &sandbox,
    );
    assert_success(&doctor_table, "doctor table");
    assert_not_contains(&doctor_table.stdout, "Cycle file", "doctor table");
    assert_not_contains(&doctor_table.stdout, "Cycle store", "doctor table");
}

#[test]
fn doctor_json_warns_when_recent_rate_limits_are_unobserved() {
    let sandbox = Sandbox::new();
    let empty_sessions = sandbox.home.join("empty-sessions");
    fs::create_dir_all(&empty_sessions).expect("create empty sessions dir");

    let doctor = run_codex_ops(
        [
            "doctor",
            "--auth-file",
            sandbox.auth_file.to_str().unwrap(),
            "--codex-home",
            sandbox.codex_home.to_str().unwrap(),
            "--sessions-dir",
            empty_sessions.to_str().unwrap(),
            "--json",
        ],
        &sandbox,
    );
    assert_success(&doctor, "doctor no rate limits json");
    let doctor_json = parse_json(&doctor.stdout, "doctor no rate limits json");
    assert_check_status(&doctor_json, "Recent rate limits", "warn");

    let rate_limits = doctor_json["checks"]
        .as_array()
        .expect("doctor checks array")
        .iter()
        .find(|check| check["name"] == "Recent rate limits")
        .expect("recent rate limits check");
    assert_json_eq(
        &rate_limits["message"],
        "No observed rate limits found in the last 7 days",
        "recent rate limits warning",
    );
    assert_contains(
        &doctor.stdout,
        "Samples: 0",
        "doctor no rate limits details",
    );
}
