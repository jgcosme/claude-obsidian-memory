//! Lifecycle hook entry points + shared helpers.

pub mod ensure_statusline;
pub mod overview_cache;
pub mod session_end;
pub mod session_start;
pub mod usage_log;
pub mod user_prompt_submit;

use anyhow::Result;

use crate::cli::{HookArgs, HookCmd};

pub fn run(args: HookArgs) -> Result<i32> {
    match args.command {
        HookCmd::SessionStart => session_start::run(),
        HookCmd::SessionEnd => session_end::run(),
        HookCmd::UserPromptSubmit => user_prompt_submit::run(),
        HookCmd::UsageLog(a) => usage_log::run_cli(a),
        HookCmd::SessionEndBg { state_file } => session_end::run_bg(&state_file),
    }
}

/// Sanitize a session id into a filesystem-safe single path component.
/// Mirrors `tr -c 'A-Za-z0-9._-' '_'`.
pub fn safe_session_id(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect()
}

/// Recursion guard: when a hook spawns `claude -p` for the gate or review,
/// the subprocess fires its own SessionStart/SessionEnd/UserPromptSubmit. The
/// guard env vars short-circuit those nested fires.
pub fn recursion_guard_active() -> bool {
    std::env::var_os("CLAUDE_MEMORY_GATE").is_some()
        || std::env::var_os("CLAUDE_MEMORY_REVIEW").is_some()
}

/// Source `~/.config/obsidian-memory/config.env` into our process env (best-
/// effort). Mirrors the bash hooks' `. "$CONFIG_FILE"` line. Existing env
/// vars win — config.env only fills holes.
pub fn load_config_env() {
    let Some(home) = dirs::home_dir() else { return };
    let path = home.join(".config/obsidian-memory/config.env");
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    for raw in text.lines() {
        let mut line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if let Some(rest) = line.strip_prefix("export ") {
            line = rest.trim_start();
        }
        let Some(eq) = line.find('=') else { continue };
        let (k, v) = (line[..eq].trim(), line[eq + 1..].trim());
        if k.is_empty() { continue; }
        let val = if v.len() >= 2 {
            let first = v.as_bytes()[0] as char;
            let last = v.as_bytes()[v.len() - 1] as char;
            if (first == '\'' || first == '"') && first == last {
                &v[1..v.len() - 1]
            } else { v }
        } else { v };
        if std::env::var_os(k).is_none() {
            // SAFETY: hooks run as one-shot processes; sourcing config at
            // startup is single-threaded. Bash's `. config.env` does the
            // same.
            unsafe { std::env::set_var(k, val); }
        }
    }
}

/// Best-effort `which`: scan PATH for an executable file named `prog`. None
/// when not found. Used by hooks to locate the `claude` CLI.
pub fn which(prog: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(prog);
        if candidate.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = candidate.metadata() {
                    if meta.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
            }
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}
