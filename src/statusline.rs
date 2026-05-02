//! Claude Code statusline — port of scripts/statusline.py.
//!
//! Reads Claude Code session JSON on stdin, scans the obsidian-memory plugin
//! usage log + main transcript, attributes plugin token cost via
//! injection×turns-alive, and renders one colored line.
//!
//! Self-disables when:
//!   - the plugin is uninstalled (no `enabledPlugins.obsidian-memory@*: true`)
//!   - `OBSIDIAN_MEMORY_STATUSLINE_ENABLED=false` (via env or config.env)
//!
//! Both produce *zero* output, matching Python's `return` path.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;

const CHARS_KINDS: &[&str] = &["session_start", "gate_inject"];

pub fn run() -> Result<i32> {
    if !plugin_installed() {
        return Ok(0);
    }
    if !config_flag("OBSIDIAN_MEMORY_STATUSLINE_ENABLED", true) {
        return Ok(0);
    }

    let mut payload_text = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload_text);
    let payload: Value = match serde_json::from_str(&payload_text) {
        Ok(v) => v,
        Err(_) => {
            render(None, None);
            return Ok(0);
        }
    };

    let session_id = payload.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let transcript = payload.get("transcript_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cwd = payload
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());

    let project = project_tag(&cwd);

    if session_id.is_empty() {
        render(None, project.as_deref());
        return Ok(0);
    }

    let safe_id: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect();
    let usage_dir = std::env::var("MEMORY_USAGE_DIR").unwrap_or_else(|_| "/tmp/claude-memory-usage".into());
    let plugin_log = PathBuf::from(usage_dir).join(format!("{safe_id}.jsonl"));

    let (api_sum, chars_events) = read_plugin_log(&plugin_log);
    let (main_sum, main_msgs_ts) = read_main_transcript(&transcript);

    // Attribution: each chars-event injected `tok` tokens into `turns_alive` turns.
    let n_msgs = main_msgs_ts.len();
    let mut plugin_main_attr: u64 = 0;
    for (tok, ts) in &chars_events {
        let turns_alive = match ts {
            Some(ts) => main_msgs_ts.iter().filter(|mt| *mt >= ts).count(),
            None => n_msgs,
        };
        plugin_main_attr += (*tok) * (turns_alive as u64);
    }

    let plugin_total = plugin_main_attr + api_sum;
    let total_session = main_sum + api_sum;

    if total_session == 0 || plugin_total == 0 {
        render(None, project.as_deref());
        return Ok(0);
    }

    let share = (plugin_total as f64) / (total_session as f64) * 100.0;
    // Color by magnitude — passive cost signal.
    let color = if share >= 25.0 {
        "\x1b[31m" // red
    } else if share >= 10.0 {
        "\x1b[33m" // yellow
    } else {
        "\x1b[2m" // dim
    };
    let body = format!(
        "{} tok · {color}{share:.1}%\x1b[0m",
        fmt_tok(plugin_total),
    );
    render(Some(&body), project.as_deref());
    Ok(0)
}

/// True iff `~/.claude/settings.json`'s `enabledPlugins` has any
/// `obsidian-memory@<ver>` key with truthy value.
fn plugin_installed() -> bool {
    let Some(home) = dirs::home_dir() else { return false };
    let settings_file = home.join(".claude/settings.json");
    if !settings_file.is_file() {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(&settings_file) else { return false };
    let Ok(data): std::result::Result<Value, _> = serde_json::from_str(&text) else { return false };
    let Some(enabled) = data.get("enabledPlugins").and_then(|v| v.as_object()) else { return false };
    enabled
        .iter()
        .any(|(k, v)| k.starts_with("obsidian-memory@") && truthy(v))
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().map(|x| x != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Read a boolean flag from config.env. Honors env override first.
/// Parses minimal `KEY=value` and `export KEY="value"` lines.
fn config_flag(name: &str, default: bool) -> bool {
    if let Ok(env_val) = std::env::var(name) {
        return matches!(env_val.trim().to_lowercase().as_str(), "true" | "1" | "yes" | "on");
    }
    let config_path = match std::env::var("OBSIDIAN_MEMORY_CONFIG_FILE") {
        Ok(p) => PathBuf::from(p),
        Err(_) => match dirs::home_dir() {
            Some(h) => h.join(".config/obsidian-memory/config.env"),
            None => return default,
        },
    };
    if !config_path.is_file() {
        return default;
    }
    let Ok(text) = std::fs::read_to_string(&config_path) else { return default };
    for raw in text.lines() {
        let mut line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("export ") {
            line = rest.trim_start();
        }
        let Some(eq) = line.find('=') else { continue };
        let (k, v) = line.split_at(eq);
        if k.trim() != name {
            continue;
        }
        let val = v[1..].trim();
        // Strip matching quote pair if present.
        let unquoted = if val.len() >= 2 {
            let first = val.as_bytes()[0] as char;
            let last = val.as_bytes()[val.len() - 1] as char;
            if (first == '\'' || first == '"') && first == last {
                &val[1..val.len() - 1]
            } else {
                val
            }
        } else {
            val
        };
        return matches!(unquoted.to_lowercase().as_str(), "true" | "1" | "yes" | "on");
    }
    default
}

/// Resolve a registered+enabled project name for `cwd`'s git repo, or `None`.
fn project_tag(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(cwd).args(["rev-parse", "--show-toplevel"]);
    let Ok(out) = cmd.output() else { return None };
    if !out.status.success() {
        return None;
    }
    let project_root = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if project_root.is_empty() {
        return None;
    }
    let projects_file = match std::env::var("OBSIDIAN_MEMORY_PROJECTS_FILE") {
        Ok(p) => PathBuf::from(p),
        Err(_) => match dirs::home_dir() {
            Some(h) => h.join(".config/obsidian-memory/projects.json"),
            None => return None,
        },
    };
    let Ok(text) = std::fs::read_to_string(&projects_file) else { return None };
    let Ok(data): std::result::Result<Value, _> = serde_json::from_str(&text) else { return None };
    // Python: str(Path(project_root).resolve()) — symlink-resolved absolute.
    let resolved_key = crate::vault::walk::absolute(Path::new(&project_root))
        .to_string_lossy()
        .into_owned();
    let entry = data
        .get("projects")
        .and_then(|v| v.as_object())
        .and_then(|m| m.get(&resolved_key))?;
    if !truthy(entry.get("enabled").unwrap_or(&Value::Bool(false))) {
        return None;
    }
    let project = entry.get("project").and_then(|v| v.as_str()).unwrap_or("");
    if project.is_empty() {
        None
    } else {
        Some(project.to_string())
    }
}

type CharsEvent = (u64, Option<DateTime<Utc>>);

fn read_plugin_log(path: &Path) -> (u64, Vec<CharsEvent>) {
    let mut api_sum: u64 = 0;
    let mut chars_events: Vec<CharsEvent> = Vec::new();
    let Ok(text) = std::fs::read_to_string(path) else { return (api_sum, chars_events); };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        let kind = e.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if e.get("mode").and_then(|v| v.as_str()) == Some("api") {
            let u = e.get("usage").cloned().unwrap_or(Value::Null);
            api_sum += token_total(&u);
        } else if CHARS_KINDS.contains(&kind) {
            let tok = e.get("approx_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let ts = e.get("ts").and_then(|v| v.as_str()).and_then(parse_ts);
            chars_events.push((tok, ts));
        }
    }
    (api_sum, chars_events)
}

fn read_main_transcript(path: &str) -> (u64, Vec<DateTime<Utc>>) {
    let mut main_sum: u64 = 0;
    let mut msgs_ts: Vec<DateTime<Utc>> = Vec::new();
    if path.is_empty() { return (main_sum, msgs_ts); }
    let p = Path::new(path);
    if !p.is_file() { return (main_sum, msgs_ts); }
    let Ok(text) = std::fs::read_to_string(p) else { return (main_sum, msgs_ts); };
    let mut seen = std::collections::HashSet::<String>::new();
    for raw in text.lines() {
        let Ok(e) = serde_json::from_str::<Value>(raw) else { continue };
        let msg = e.get("message").cloned().unwrap_or(Value::Null);
        let usage = msg.get("usage").cloned().unwrap_or(Value::Null);
        if usage.is_null() { continue; }
        if let Some(mid) = msg.get("id").and_then(|v| v.as_str()) {
            if !seen.insert(mid.to_string()) {
                continue;
            }
        }
        if let Some(ts) = e.get("timestamp").and_then(|v| v.as_str()).and_then(parse_ts) {
            msgs_ts.push(ts);
        }
        main_sum += token_total(&usage);
    }
    (main_sum, msgs_ts)
}

fn token_total(usage: &Value) -> u64 {
    let g = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    g("input_tokens") + g("output_tokens") + g("cache_read_input_tokens") + g("cache_creation_input_tokens")
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    if s.is_empty() { return None; }
    // Python: replace trailing "Z" with "+00:00", then fromisoformat.
    let normalized = if let Some(stripped) = s.strip_suffix('Z') {
        format!("{stripped}+00:00")
    } else {
        s.to_string()
    };
    DateTime::parse_from_rfc3339(&normalized).ok().map(|dt| dt.with_timezone(&Utc))
}

fn fmt_tok(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn render(text: Option<&str>, project: Option<&str>) {
    // Truecolor obsidian-purple #A78BFA prefix.
    let mut prefix = String::from("\x1b[38;2;167;139;250mobsidian-memory\x1b[0m");
    if let Some(p) = project {
        if !p.is_empty() {
            prefix.push_str(&format!(" \x1b[2m•\x1b[0m {p}"));
        }
    }
    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    let body = match text {
        Some(t) if !t.is_empty() => format!("{prefix} {t}\n"),
        _ => format!("{prefix} \x1b[2m—\x1b[0m\n"),
    };
    let _ = h.write_all(body.as_bytes());
}
