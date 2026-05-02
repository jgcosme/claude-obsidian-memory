//! Slim a Claude Code session transcript for SessionEnd review.
//!
//! Mirrors `scripts/_slim_transcript.py`. Reads JSONL transcript, emits
//! human-readable dialogue with tool_use noise stripped. Output is consumed by
//! the SessionEnd review prompt, so byte-for-byte parity with Python matters.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::cli::SlimTranscriptArgs;

const TEXT_PREVIEW_CHARS: usize = 4000;

#[derive(Debug, Default)]
pub struct SlimStats {
    pub events_in: u64,
    pub events_out: u64,
    pub user: u64,
    pub assistant: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub tool_calls: u64,
}

pub fn run(args: SlimTranscriptArgs) -> Result<i32> {
    let p = Path::new(&args.transcript);
    if !p.is_file() {
        eprintln!("error: transcript not found: {}", p.display());
        return Ok(2);
    }

    let stats = if let Some(out_path) = &args.out {
        let mut f = File::create(out_path)
            .with_context(|| format!("create {}", out_path.display()))?;
        slim(p, &mut f)?
    } else {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        slim(p, &mut handle)?
    };

    if args.stats && stats.bytes_in > 0 {
        let ratio = (stats.bytes_out as f64) / (stats.bytes_in as f64);
        eprintln!(
            "events: {} → {} (user={}, assistant={}, tools={})  bytes: {} → {} ({:.1}%)",
            stats.events_in,
            stats.events_out,
            stats.user,
            stats.assistant,
            stats.tool_calls,
            comma(stats.bytes_in),
            comma(stats.bytes_out),
            ratio * 100.0,
        );
    }
    Ok(0)
}

/// Public wrapper around `slim` for callers that just need the output (e.g.
/// the SessionEnd hook) and don't need stats. Returns `Err` on I/O failure.
pub fn slim_to_writer<W: Write>(transcript: &Path, out: &mut W) -> Result<()> {
    slim(transcript, out)?;
    Ok(())
}

fn slim<W: Write>(transcript: &Path, out: &mut W) -> Result<SlimStats> {
    let mut stats = SlimStats::default();
    let f = File::open(transcript).with_context(|| format!("open {}", transcript.display()))?;
    let reader = BufReader::new(f);

    for raw_line in reader.lines() {
        let line = raw_line?;
        // Python counts `len(line)` on a *str* (codepoints), not bytes —
        // so multi-byte chars like `—` count as 1 even though they're 3 bytes
        // on disk. The statistic is mislabeled "bytes" but we replicate the
        // count exactly. `+1` adds back the newline BufReader stripped.
        stats.bytes_in += (line.chars().count() as u64) + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        stats.events_in += 1;
        let Ok(e) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        let etype = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if etype != "user" && etype != "assistant" {
            continue;
        }

        let msg = e.get("message").cloned().unwrap_or(Value::Null);
        let ts = fmt_ts(e.get("timestamp").and_then(|v| v.as_str()).unwrap_or(""));

        if etype == "user" {
            let text = slim_user_content(msg.get("content").unwrap_or(&Value::Null));
            if text.is_empty() {
                continue;
            }
            let line_out = format!("[{ts}] USER: {text}\n\n");
            out.write_all(line_out.as_bytes())?;
            stats.bytes_out += line_out.chars().count() as u64;
            stats.user += 1;
            stats.events_out += 1;
        } else {
            let (text, tools) = extract_text(msg.get("content").unwrap_or(&Value::Null));
            let mut pieces: Vec<String> = Vec::new();
            if !text.is_empty() {
                pieces.push(format!("[{ts}] ASSISTANT: {text}"));
            }
            if !tools.is_empty() {
                pieces.push(format!("[{ts}] ASSISTANT used: {}", tools.join(", ")));
                stats.tool_calls += tools.len() as u64;
            }
            if pieces.is_empty() {
                continue;
            }
            let line_out = pieces.join("\n") + "\n\n";
            out.write_all(line_out.as_bytes())?;
            stats.bytes_out += line_out.chars().count() as u64;
            stats.assistant += 1;
            stats.events_out += 1;
        }
    }

    Ok(stats)
}

/// Trim a fractional ISO timestamp to seconds: `...T12:24:51.697Z` → `...T12:24:51Z`.
fn fmt_ts(ts: &str) -> String {
    if ts.is_empty() {
        return String::new();
    }
    if let Some(dot) = ts.find('.') {
        let head = &ts[..dot];
        let tail = &ts[dot + 1..];
        if tail.is_empty() {
            return head.to_string();
        }
        return format!("{head}Z");
    }
    ts.to_string()
}

/// Extract concatenated `text` block content + a list of tool names. Drops
/// `thinking` blocks (verbose, not user-visible) and tool_use *inputs* (we
/// only keep names). `tool_result` blocks live on user messages and are
/// dropped by `slim_user_content`.
fn extract_text(content: &Value) -> (String, Vec<String>) {
    if let Some(s) = content.as_str() {
        return (s.to_string(), Vec::new());
    }
    let Some(arr) = content.as_array() else { return (String::new(), Vec::new()); };

    let mut text_parts: Vec<String> = Vec::new();
    let mut tools: Vec<String> = Vec::new();
    for block in arr {
        let Some(obj) = block.as_object() else { continue; };
        let bt = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match bt {
            "text" => {
                let t = obj.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if !t.is_empty() {
                    if t.chars().count() > TEXT_PREVIEW_CHARS {
                        // Python: t[:N] + "…" — slice by *chars*, not bytes,
                        // since "…" itself is multi-byte.
                        let truncated: String = t.chars().take(TEXT_PREVIEW_CHARS).collect();
                        text_parts.push(format!("{truncated}…"));
                    } else {
                        text_parts.push(t.to_string());
                    }
                }
            }
            "thinking" => {}
            "tool_use" => {
                let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                tools.push(name.to_string());
            }
            _ => {}
        }
    }

    (text_parts.join("\n").trim().to_string(), tools)
}

fn slim_user_content(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.trim().to_string();
    }
    let Some(arr) = content.as_array() else { return String::new(); };
    let mut parts: Vec<String> = Vec::new();
    for block in arr {
        let Some(obj) = block.as_object() else { continue; };
        if obj.get("type").and_then(|v| v.as_str()) == Some("text") {
            let t = obj.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if !t.is_empty() {
                parts.push(t);
            }
        }
    }
    parts.join("\n")
}

/// Format an integer with comma thousands separators (Python's `f"{n:,}"`).
fn comma(n: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_ts_strips_fractional() {
        assert_eq!(fmt_ts("2026-04-29T12:24:51.697Z"), "2026-04-29T12:24:51Z");
        assert_eq!(fmt_ts("2026-04-29T12:24:51Z"), "2026-04-29T12:24:51Z");
        assert_eq!(fmt_ts(""), "");
    }

    #[test]
    fn comma_matches_python() {
        assert_eq!(comma(0), "0");
        assert_eq!(comma(999), "999");
        assert_eq!(comma(1_000), "1,000");
        assert_eq!(comma(1_234_567), "1,234,567");
    }
}
