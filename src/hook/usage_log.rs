//! Append a usage event to `${MEMORY_USAGE_DIR}/<safe_session_id>.jsonl`.
//!
//! Mirrors `hooks/scripts/_usage_log.sh`. Exposed as a hidden CLI subcommand
//! (`obsidian-memory hook usage-log ...`) for parity testing; called directly
//! from sibling hook modules in normal use.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use chrono::Utc;
use serde_json::{Value, json};

use crate::cli::HookUsageLogArgs;
use crate::hook::safe_session_id;

/// Append an `api` event (real `claude -p --output-format json` usage block).
/// `usage_json` is the single-line JSON object from `.usage`. `cost_usd` and
/// `duration_ms` are optional — pass `None` to record `null`.
pub fn append_api(
    session_id: &str,
    kind: &str,
    usage_json: &str,
    cost_usd: Option<&str>,
    duration_ms: Option<&str>,
) {
    let Some(out) = open_log(session_id) else { return };
    let usage = parse_usage_object(usage_json);
    let cost = cost_usd
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<f64>().ok())
        .map(Value::from)
        .unwrap_or(Value::Null);
    let duration = duration_ms
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u64>().ok())
        .map(Value::from)
        .unwrap_or(Value::Null);
    let event = json!({
        "ts": now_iso(),
        "kind": kind,
        "mode": "api",
        "usage": usage,
        "cost_usd": cost,
        "duration_ms": duration,
    });
    let _ = write_line(&out, &event);
}

/// Append a `chars` event (injected text — no API call). `bytes` is the size
/// of the injected content; `approx_tokens` is recorded as `ceil(bytes/4)`.
pub fn append_chars(session_id: &str, kind: &str, bytes: u64) {
    let Some(out) = open_log(session_id) else { return };
    let approx = bytes.div_ceil(4);
    let event = json!({
        "ts": now_iso(),
        "kind": kind,
        "mode": "chars",
        "chars": bytes,
        "approx_tokens": approx,
    });
    let _ = write_line(&out, &event);
}

fn open_log(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() {
        return None;
    }
    let dir = std::env::var("MEMORY_USAGE_DIR").unwrap_or_else(|_| "/tmp/claude-memory-usage".into());
    let dir = PathBuf::from(dir);
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    Some(dir.join(format!("{}.jsonl", safe_session_id(session_id))))
}

fn write_line(path: &PathBuf, value: &Value) -> std::io::Result<()> {
    let line = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

fn parse_usage_object(s: &str) -> Value {
    if s.trim().is_empty() {
        return Value::Object(serde_json::Map::new());
    }
    match serde_json::from_str::<Value>(s) {
        Ok(v @ Value::Object(_)) => v,
        _ => Value::Object(serde_json::Map::new()),
    }
}

fn now_iso() -> String {
    // UTC `YYYY-MM-DDTHH:MM:SSZ` to match _usage_log.sh's `date -u '+%Y-%m-%dT%H:%M:%SZ'`.
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn run_cli(args: HookUsageLogArgs) -> Result<i32> {
    if args.session_id.is_empty() || args.kind.is_empty() {
        return Ok(0);
    }
    match args.mode.as_str() {
        "api" => {
            let usage_json = args.field4.as_deref().unwrap_or("{}");
            append_api(
                &args.session_id,
                &args.kind,
                usage_json,
                args.field5.as_deref(),
                args.field6.as_deref(),
            );
        }
        "chars" => {
            let bytes = args.field4.as_deref()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            append_chars(&args.session_id, &args.kind, bytes);
        }
        _ => {}
    }
    Ok(0)
}
