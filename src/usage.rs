//! Aggregate the current session's plugin usage JSONL —
//! port of `scripts/usage.sh` (its embedded Python).
//!
//! Output is heavily formatted with ANSI bold/dim. Match the Python original
//! line-for-line so the parity harness can byte-diff.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::hook::safe_session_id;

const CHARS_KINDS: &[&str] = &["session_start", "gate_inject"];

/// Display blocks in stable order. (kind, category) — category drives layout.
const BLOCK_ORDER: &[(&str, &str)] = &[
    ("session_start", "injected"),
    ("gate_inject",   "injected"),
    ("gate_call",     "api"),
    ("review_call",   "api"),
];

pub fn run() -> Result<i32> {
    let usage_dir = std::env::var("MEMORY_USAGE_DIR")
        .unwrap_or_else(|_| "/tmp/claude-memory-usage".into());
    let usage_dir = PathBuf::from(usage_dir);

    if !usage_dir.is_dir() {
        println!("No usage data yet — {} does not exist.", usage_dir.display());
        println!("(The plugin records per-session usage events as the SessionStart, gate, and SessionEnd hooks fire.)");
        return Ok(0);
    }

    let target = match locate_target(&usage_dir) {
        Some(p) => p,
        None => {
            println!("No usage data yet for any session.");
            println!("(Looked in {}. The SessionStart hook writes the first event of each session.)", usage_dir.display());
            return Ok(0);
        }
    };

    let events = parse_events(&target)?;
    if events.is_empty() {
        println!("No parseable events in {}", target.display());
        return Ok(0);
    }

    print_summary(&target, &events);
    Ok(0)
}

fn locate_target(usage_dir: &Path) -> Option<PathBuf> {
    if let Ok(sid) = std::env::var("CLAUDE_SESSION_ID") {
        if !sid.is_empty() {
            let safe = safe_session_id(&sid);
            let p = usage_dir.join(format!("{safe}.jsonl"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // Newest *.jsonl by mtime.
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(usage_dir).ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .filter_map(|e| {
            let mt = e.metadata().ok()?.modified().ok()?;
            Some((mt, e.path()))
        })
        .collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.0));
    entries.into_iter().next().map(|(_, p)| p)
}

fn parse_events(path: &Path) -> Result<Vec<Value>> {
    let text = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            out.push(v);
        }
    }
    Ok(out)
}

fn print_summary(target: &Path, events: &[Value]) {
    let session_id = target.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();

    println!("\x1b[1mObsidian-memory plugin token usage\x1b[0m");
    println!("  session: {session_id}");
    println!("  events:  {}", events.len());
    println!();

    // Bucket by kind.
    let mut by_kind: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for e in events {
        let k = e.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        by_kind.entry(k).or_default().push(e);
    }

    let mut inj_tok_total: u64 = 0;
    let mut api_in: u64 = 0;
    let mut api_out: u64 = 0;
    let mut api_cache_r: u64 = 0;
    let mut api_cache_w: u64 = 0;

    for (kind, category) in BLOCK_ORDER {
        let items = by_kind.get(*kind).cloned().unwrap_or_default();
        let n = items.len();
        let suffix = if n == 1 { "" } else { "s" };
        if *category == "injected" {
            if n == 0 {
                println!("  \x1b[2m{kind:<14}\x1b[0m [injected]  0 events");
                continue;
            }
            let chars: u64 = items.iter().map(|e| e.get("chars").and_then(|v| v.as_u64()).unwrap_or(0)).sum();
            let tok: u64 = items.iter().map(|e| e.get("approx_tokens").and_then(|v| v.as_u64()).unwrap_or(0)).sum();
            inj_tok_total += tok;
            println!("  {kind:<14} [injected]  {n} event{suffix}, {} chars (~{} tok)",
                fmt_int(chars), fmt_int(tok));
        } else {
            // api
            if n == 0 {
                let note = if *kind == "review_call" { "fires at SessionEnd" } else { "no calls yet" };
                println!("  \x1b[2m{kind:<14}\x1b[0m [api]       0 calls ({note})");
                continue;
            }
            let in_tok: u64 = items.iter().map(|e| e.get("usage").and_then(|u| u.get("input_tokens")).and_then(|v| v.as_u64()).unwrap_or(0)).sum();
            let out_tok: u64 = items.iter().map(|e| e.get("usage").and_then(|u| u.get("output_tokens")).and_then(|v| v.as_u64()).unwrap_or(0)).sum();
            let cache_r: u64 = items.iter().map(|e| e.get("usage").and_then(|u| u.get("cache_read_input_tokens")).and_then(|v| v.as_u64()).unwrap_or(0)).sum();
            let cache_w: u64 = items.iter().map(|e| e.get("usage").and_then(|u| u.get("cache_creation_input_tokens")).and_then(|v| v.as_u64()).unwrap_or(0)).sum();
            api_in += in_tok;
            api_out += out_tok;
            api_cache_r += cache_r;
            api_cache_w += cache_w;
            println!("  {kind:<14} [api]       {n} call{suffix}");
            println!("    input:    {}", fmt_int(in_tok));
            println!("    output:   {}", fmt_int(out_tok));
            println!("    cache_r:  {}", fmt_int(cache_r));
            println!("    cache_w:  {}", fmt_int(cache_w));
        }
    }
    println!();

    // Totals block.
    println!("\x1b[1mTotals\x1b[0m");
    println!("  injected  (re-sent each turn — mostly cache_read after first turn):");
    println!("    {} tok per turn", fmt_int(inj_tok_total));
    println!("  api       (one-time consumption from plugin's claude -p calls):");
    println!("    input:    {}", fmt_int(api_in));
    println!("    output:   {}", fmt_int(api_out));
    println!("    cache_r:  {}", fmt_int(api_cache_r));
    println!("    cache_w:  {}", fmt_int(api_cache_w));
    let plugin_separate = api_in + api_out + api_cache_r + api_cache_w;
    println!("    sum:      {}", fmt_int(plugin_separate));
    println!();

    // Session share section — depends on the main transcript.
    println!("\x1b[1mSession share\x1b[0m  (this session's tokens vs plugin overhead)");
    let transcript = find_transcript(&session_id);

    if let Some(tx) = transcript {
        // Aggregate the main transcript with .message.id dedup.
        let mut main_msgs_ts: Vec<DateTime<Utc>> = Vec::new();
        let mut main_input: u64 = 0;
        let mut main_output: u64 = 0;
        let mut main_cache_r: u64 = 0;
        let mut main_cache_w: u64 = 0;
        let mut seen: HashSet<String> = HashSet::new();
        if let Ok(text) = std::fs::read_to_string(&tx) {
            for line in text.lines() {
                let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
                let msg = e.get("message").cloned().unwrap_or(Value::Null);
                let usage = msg.get("usage").cloned().unwrap_or(Value::Null);
                if usage.is_null() { continue; }
                if let Some(mid) = msg.get("id").and_then(|v| v.as_str()) {
                    if !seen.insert(mid.to_string()) { continue; }
                }
                if let Some(t) = e.get("timestamp").and_then(|v| v.as_str()).and_then(parse_ts) {
                    main_msgs_ts.push(t);
                }
                main_input += usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                main_output += usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                main_cache_r += usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                main_cache_w += usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            }
        }
        let main_sum = main_input + main_output + main_cache_r + main_cache_w;
        let n_msgs = main_msgs_ts.len();

        let mut plugin_main_attr: u64 = 0;
        for kind in CHARS_KINDS {
            for ev in by_kind.get(*kind).cloned().unwrap_or_default() {
                let tok = ev.get("approx_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let ts = ev.get("ts").and_then(|v| v.as_str()).and_then(parse_ts);
                let turns_alive = match ts {
                    None => n_msgs as u64,
                    Some(ts) => main_msgs_ts.iter().filter(|mt| **mt >= ts).count() as u64,
                };
                plugin_main_attr += tok * turns_alive;
            }
        }

        let plugin_total = plugin_main_attr + plugin_separate;
        let total_session = main_sum + plugin_separate;
        let share = if total_session > 0 {
            (plugin_total as f64) / (total_session as f64) * 100.0
        } else { 0.0 };

        println!("  main session:    {} tok ({n_msgs} assistant msgs)", fmt_int(main_sum));
        println!("    plugin-attributable (injection × turns): ~{}", fmt_int(plugin_main_attr));
        println!("  plugin separate: {} tok (gate + review)", fmt_int(plugin_separate));
        println!("  ─────────");
        println!("  total session:   {} tok", fmt_int(total_session));
        println!("  \x1b[1mplugin share:    {share:.1}%\x1b[0m");
        println!();
        println!("\x1b[2m  Multiply plugin share by Claude's /usage % to estimate the\x1b[0m");
        println!("\x1b[2m  plugin's pts of your rate-limit quota for this session.\x1b[0m");
    } else {
        println!("  (transcript not found at ~/.claude/projects/*/{session_id}.jsonl —");
        println!("   could not compute session share)");
    }
    println!();
    println!("\x1b[2m  Tokens meter against your Claude rate-limit pool, not your wallet —\x1b[0m");
    println!("\x1b[2m  subscriptions cover usage within rate limits.\x1b[0m");
}

fn find_transcript(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() { return None; }
    let home = dirs::home_dir()?;
    let projects_dir = home.join(".claude/projects");
    if !projects_dir.is_dir() { return None; }
    let target_name = format!("{session_id}.jsonl");
    for entry in std::fs::read_dir(&projects_dir).ok()? {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().ok()?.is_dir() { continue; }
        let candidate = entry.path().join(&target_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Match Python's `datetime.fromisoformat` with the trailing-Z workaround,
/// plus naive-as-local fallback.
fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    if s.is_empty() { return None; }
    let normalized = if let Some(stripped) = s.strip_suffix('Z') {
        format!("{stripped}+00:00")
    } else {
        s.to_string()
    };
    if let Ok(dt) = DateTime::parse_from_rfc3339(&normalized) {
        return Some(dt.with_timezone(&Utc));
    }
    // Naive datetime — Python: "treat as local, convert to UTC".
    // chrono::NaiveDateTime → local → UTC.
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        if let chrono::LocalResult::Single(local_dt) = chrono::Local.from_local_datetime(&naive) {
            return Some(local_dt.with_timezone(&Utc));
        }
    }
    None
}

/// Python `f"{n:,}"`.
fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

use chrono::TimeZone;
