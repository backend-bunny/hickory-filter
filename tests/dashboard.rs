//! The Grafana dashboard only uses what the server provides: the metrics in the
//! README's reference (which `tests/metrics.rs` checks against `/metrics`) and
//! the documented query log fields, and it picks its datasources at runtime.

use std::collections::BTreeSet;

use hickory_filter::logging::{QUERY_FIELDS, QUERY_TARGET};
use regex::Regex;
use serde_json::Value;

mod common;
use common::{backticked, readme, readme_table};

const DASHBOARD: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/charts/hickory-filter/dashboards/hickory-filter.json"
);

fn load() -> Value {
    let text = std::fs::read_to_string(DASHBOARD).expect("the dashboard file");
    serde_json::from_str(&text).expect("the dashboard is valid JSON")
}

fn documented_metrics() -> BTreeSet<String> {
    ["### Metrics reference", "#### Recursive mode metrics"]
        .iter()
        .flat_map(|heading| readme_table(heading))
        .map(|row| backticked(&row[0])[0].clone())
        .collect()
}

/// Every `datasource` value anywhere in the dashboard.
fn datasources<'a>(value: &'a Value, found: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                if key == "datasource" {
                    found.push(inner);
                }
                datasources(inner, found);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| datasources(item, found)),
        _ => {}
    }
}

/// The expressions of a panel's targets.
fn exprs(panel: &Value) -> Vec<String> {
    panel["targets"]
        .as_array()
        .map(|targets| {
            targets
                .iter()
                .filter_map(|t| t["expr"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Everything wrong with the dashboard, one message each.
fn problems(dashboard: &Value, documented: &BTreeSet<String>) -> Vec<String> {
    let mut problems = Vec::new();
    let mut problem = |message: String| problems.push(message);

    // The file itself.
    if dashboard["title"].as_str().is_none_or(str::is_empty) {
        problem("no title".to_owned());
    }
    if dashboard["uid"] != "hickory-filter" {
        problem(format!(
            "the UID is {}, not the fixed hickory-filter",
            dashboard["uid"]
        ));
    }
    let panels = dashboard["panels"].as_array().cloned().unwrap_or_default();
    if panels.iter().all(|p| p["type"] == "row") {
        problem("no panels".to_owned());
    }
    if dashboard.get("__inputs").is_some() {
        problem("it has import inputs (`__inputs`)".to_owned());
    }

    // Datasources come from variables, never hard-coded.
    let variable = Regex::new(r"^\$\{[a-z_]+\}$").unwrap();
    let mut found = Vec::new();
    datasources(dashboard, &mut found);
    for datasource in found {
        match datasource {
            Value::Object(map) => {
                let uid = map.get("uid").and_then(Value::as_str).unwrap_or("");
                if !variable.is_match(uid) {
                    problem(format!("a hard-coded datasource uid {uid:?}"));
                }
            }
            other => problem(format!(
                "a datasource that is not a variable reference: {other}"
            )),
        }
    }
    let variables = dashboard["templating"]["list"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (name, kind) in [("ds_prometheus", "prometheus"), ("ds_loki", "loki")] {
        let defined = variables
            .iter()
            .any(|v| v["name"] == name && v["type"] == "datasource" && v["query"] == kind);
        if !defined {
            problem(format!("no datasource variable {name} for {kind}"));
        }
    }

    // Only stable panel types.
    for panel in &panels {
        let kind = panel["type"].as_str().unwrap_or("");
        if !["row", "stat", "timeseries", "table", "bargauge", "text"].contains(&kind) {
            problem(format!(
                "panel type {kind:?} is not one of the stable types"
            ));
        }
    }

    // Sections: the overview needs only Prometheus, the usage section only Loki.
    let metric = Regex::new(r"hickory_(?:filter|resolver|recursor)_[a-z_]+").unwrap();
    let label_filter = Regex::new(r"\|\s*([a-z_]+)\s*(?:=~|!~|!=|=)").unwrap();
    let grouping = Regex::new(r"by \(([^)]*)\)").unwrap();
    let mut section = String::new();
    let mut variable_exprs: Vec<String> = variables
        .iter()
        .filter_map(|v| v["definition"].as_str().map(str::to_owned))
        .collect();
    for panel in &panels {
        if panel["type"] == "row" {
            section = panel["title"].as_str().unwrap_or("").to_owned();
            continue;
        }
        let datasource_kind = panel["datasource"]["type"].as_str().unwrap_or("none");
        let title = panel["title"].as_str().unwrap_or("?");
        if section.starts_with("Overview") && datasource_kind != "prometheus" {
            problem(format!("overview panel {title:?} is not backed by metrics"));
        }
        if section.starts_with("Usage") && !["loki", "none"].contains(&datasource_kind) {
            problem(format!(
                "usage panel {title:?} is not backed by the query log"
            ));
        }
        for expr in exprs(panel) {
            if datasource_kind == "prometheus" {
                variable_exprs.push(expr);
            } else if datasource_kind == "loki" {
                if !expr.contains("| json") || !expr.contains(&format!("target=\"{QUERY_TARGET}\""))
                {
                    problem(format!(
                        "{title:?} does not select query lines by target: {expr}"
                    ));
                }
                if !expr.contains("${log_selector}") {
                    problem(format!(
                        "{title:?} does not use the log stream selector: {expr}"
                    ));
                }
                let mut fields: Vec<String> = label_filter
                    .captures_iter(&expr)
                    .map(|c| c[1].to_owned())
                    .collect();
                for group in grouping.captures_iter(&expr) {
                    fields.extend(group[1].split(',').map(|f| f.trim().to_owned()));
                }
                for field in fields {
                    if field != "target" && !QUERY_FIELDS.contains(&field.as_str()) {
                        problem(format!(
                            "{title:?} uses the log field {field}, which is not a documented query log field"
                        ));
                    }
                }
            }
        }
    }

    // Every metric a query uses is a documented (and so exported) metric.
    let mut used = BTreeSet::new();
    for expr in variable_exprs {
        for name in metric.find_iter(&expr) {
            used.insert(name.as_str().to_owned());
        }
    }
    for name in &used {
        let base = ["_bucket", "_sum", "_count"]
            .iter()
            .find_map(|suffix| name.strip_suffix(suffix))
            .filter(|base| documented.contains(*base));
        if !documented.contains(name) && base.is_none() {
            problem(format!(
                "it queries the metric {name}, which the server does not export"
            ));
        }
    }
    if used.is_empty() {
        problem("no panel queries any metric".to_owned());
    }
    problems
}

#[test]
fn the_dashboard_uses_only_exported_metrics_and_documented_log_fields() {
    let found = problems(&load(), &documented_metrics());
    assert!(found.is_empty(), "{found:#?}");
}

/// The dashboard text with `from` replaced by `to`, parsed again.
fn mutated(from: &str, to: &str) -> Value {
    let text = std::fs::read_to_string(DASHBOARD).expect("the dashboard file");
    assert!(
        text.contains(from),
        "the dashboard no longer contains {from:?}"
    );
    serde_json::from_str(&text.replace(from, to)).expect("valid JSON")
}

fn mentions(found: &[String], needle: &str) -> bool {
    found.iter().any(|p| p.contains(needle))
}

#[test]
fn a_misspelled_metric_fails_the_check_and_names_it() {
    let found = problems(
        &mutated(
            "hickory_filter_queries_total",
            "hickory_filter_queries_totl",
        ),
        &documented_metrics(),
    );
    assert!(
        mentions(&found, "hickory_filter_queries_totl"),
        "{found:#?}"
    );
}

#[test]
fn a_metric_that_is_not_documented_fails_the_check() {
    let found = problems(
        &mutated(
            "hickory_resolver_cache_hit_total",
            "hickory_resolver_cache_warm_total",
        ),
        &documented_metrics(),
    );
    assert!(
        mentions(&found, "hickory_resolver_cache_warm_total"),
        "{found:#?}"
    );
}

#[test]
fn an_unknown_query_log_field_fails_the_check_and_names_it() {
    let found = problems(
        &mutated("sum by (client)", "sum by (clientip)"),
        &documented_metrics(),
    );
    assert!(mentions(&found, "clientip"), "{found:#?}");
    let found = problems(
        &mutated("| outcome=\\\"blocked\\\"", "| verdict=\\\"blocked\\\""),
        &documented_metrics(),
    );
    assert!(mentions(&found, "verdict"), "{found:#?}");
}

#[test]
fn a_loki_panel_must_select_query_lines_by_target() {
    let found = problems(
        &mutated(
            "target=\\\"hickory_filter::query\\\"",
            "target=\\\"something::else\\\"",
        ),
        &documented_metrics(),
    );
    assert!(
        mentions(&found, "does not select query lines by target"),
        "{found:#?}"
    );
}

#[test]
fn a_hard_coded_datasource_or_an_import_input_fails_the_check() {
    let found = problems(&mutated("${ds_loki}", "abc123"), &documented_metrics());
    assert!(mentions(&found, "hard-coded datasource uid"), "{found:#?}");

    let mut dashboard = load();
    dashboard["__inputs"] = serde_json::json!([]);
    assert!(mentions(
        &problems(&dashboard, &documented_metrics()),
        "__inputs"
    ));
}

#[test]
fn an_unstable_panel_type_or_a_missing_uid_fails_the_check() {
    let found = problems(
        &mutated("\"type\": \"bargauge\"", "\"type\": \"piechart\""),
        &documented_metrics(),
    );
    assert!(mentions(&found, "piechart"), "{found:#?}");
    let found = problems(
        &mutated("\"uid\": \"hickory-filter\"", "\"uid\": \"other\""),
        &documented_metrics(),
    );
    assert!(mentions(&found, "fixed hickory-filter"), "{found:#?}");
}

#[test]
fn the_usage_section_says_it_needs_the_query_log() {
    let dashboard = load();
    let panels = dashboard["panels"].as_array().expect("panels");
    let usage = panels
        .iter()
        .position(|p| {
            p["type"] == "row" && p["title"].as_str().is_some_and(|t| t.starts_with("Usage"))
        })
        .expect("a usage section");
    let note = panels[usage + 1..]
        .iter()
        .find(|p| p["type"] == "text")
        .expect("a text panel in the usage section");
    let content = note["options"]["content"].as_str().expect("content");
    assert!(content.contains("query_log"), "{content}");
    // The overview comes first and has no Loki panel at all.
    assert!(
        panels[..usage]
            .iter()
            .all(|p| p["datasource"]["type"] != "loki")
    );
}

#[test]
fn the_list_table_shows_rules_skipped_lines_age_and_last_result() {
    let dashboard = load();
    let table = dashboard["panels"]
        .as_array()
        .expect("panels")
        .iter()
        .find(|p| p["type"] == "table")
        .expect("a table panel");
    let all: String = exprs(table).join("\n");
    for metric in [
        "hickory_filter_list_rules",
        "hickory_filter_list_skipped_lines",
        "hickory_filter_list_last_success_timestamp_seconds",
        "hickory_filter_list_last_refresh_success",
    ] {
        assert!(all.contains(metric), "the list table lacks {metric}");
    }
}

#[test]
fn the_readme_section_names_only_real_metrics_and_the_dashboard_path() {
    let readme = readme();
    let start = readme
        .find("### Grafana dashboard")
        .expect("a dashboard section");
    let section = &readme[start..];
    let section = &section[..section.find("\n## ").unwrap_or(section.len())];
    assert!(section.contains("charts/hickory-filter/dashboards/hickory-filter.json"));
    assert!(section.contains(QUERY_TARGET));
    let documented = documented_metrics();
    let named: Vec<String> = backticked(section)
        .into_iter()
        .filter(|word| word.starts_with("hickory_") && !word.contains("::"))
        .collect();
    assert!(named.len() >= 8, "{named:?}");
    for name in named {
        assert!(
            documented.contains(&name),
            "the README section names {name}, which is not documented"
        );
    }
}
