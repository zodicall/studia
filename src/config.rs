use crate::{Result, i18n::Lang, invalid};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub const DEFAULT_AI_TIMEOUT_SECS: u64 = 180;
pub const DEFAULT_GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com";
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
pub const MAX_ENV_FILE_BYTES: u64 = 1024 * 1024;

pub const SENSITIVE_ENV_VARS: [&str; 8] = [
    "STUDIA_TELEGRAM_TOKEN",
    "STUDIA_TELEGRAM_CHAT_ID",
    "GEMINI_API_KEY",
    "STUDIA_GEMINI_API_KEY",
    "OPENAI_API_KEY",
    "STUDIA_OPENAI_API_KEY",
    "STUDIA_AI_API_KEY",
    "ANTHROPIC_API_KEY",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiProvider {
    Auto,
    GeminiApi,
    OpenAiApi,
    Agy,
    Claude,
    Codex,
    Custom,
}

impl AiProvider {
    pub const ALL: [Self; 7] = [
        Self::Auto,
        Self::GeminiApi,
        Self::OpenAiApi,
        Self::Agy,
        Self::Claude,
        Self::Codex,
        Self::Custom,
    ];

    pub fn as_env_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::GeminiApi => "gemini",
            Self::OpenAiApi => "openai",
            Self::Agy => "agy",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Custom => "custom",
        }
    }

    pub fn display_name(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Auto, Lang::Tr) => "Otomatik (auto)",
            (Self::Auto, Lang::En) => "Automatic (auto)",
            (Self::GeminiApi, _) => "Gemini API",
            (Self::OpenAiApi, Lang::Tr) => "OpenAI / Uyumlu API",
            (Self::OpenAiApi, Lang::En) => "OpenAI / Compatible API",
            (Self::Agy, _) => "Antigravity (agy CLI)",
            (Self::Claude, _) => "Claude CLI",
            (Self::Codex, _) => "Codex CLI",
            (Self::Custom, Lang::Tr) => "Özel Komut (custom)",
            (Self::Custom, Lang::En) => "Custom Command (custom)",
        }
    }

    pub fn suggested_models(self) -> &'static [&'static str] {
        match self {
            Self::GeminiApi => &[
                "gemini-3.8-flash",
                "gemini-4-hydrogen",
                "gemini-4-argon",
                "gemini-4-carbon",
                "gemini-3.5-flash-lite",
                "gemini-3.1-pro-preview",
                "gemini-2.5-pro",
            ],
            Self::OpenAiApi => &[
                "gpt-6.1-terra",
                "gpt-6.1-sol",
                "gpt-6-astra",
                "gpt-6-luna",
                "gpt-5-mini",
                "gpt-5",
                "o4-mini",
                "qwen3.5:9b",
                "gemma3:12b",
                "llama4",
            ],
            Self::Claude => &[
                "claude-sonnet-5-5",
                "claude-opus-5-5",
                "claude-haiku-5-5",
                "claude-sonnet-4-6",
                "claude-opus-4-6",
            ],
            Self::Codex => &["gpt-6.1-sol", "gpt-6-astra", "gpt-5.3-codex", "o4-mini"],
            Self::Agy => &[
                "gemini-4-hydrogen",
                "gemini-4-argon",
                "gemini-3.8-flash",
                "claude-sonnet-5-5",
                "claude-opus-5-5",
                "gpt-oss-120b-medium",
            ],
            Self::Auto | Self::Custom => &[],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiEffort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl AiEffort {
    pub const ALL: [Self; 6] = [
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    pub fn as_env_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn display_name(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Minimal, Lang::Tr) => "minimal (en az)",
            (Self::Minimal, Lang::En) => "minimal",
            (Self::Low, Lang::Tr) => "low (hızlı)",
            (Self::Low, Lang::En) => "low",
            (Self::Medium, Lang::Tr) => "medium (dengeli)",
            (Self::Medium, Lang::En) => "medium",
            (Self::High, Lang::Tr) => "high (derin)",
            (Self::High, Lang::En) => "high",
            (Self::XHigh, Lang::Tr) => "xhigh (çok derin)",
            (Self::XHigh, Lang::En) => "xhigh",
            (Self::Max, Lang::Tr) => "max (azami)",
            (Self::Max, Lang::En) => "max",
        }
    }
}

pub fn parse_effort(raw: &str) -> Result<Option<AiEffort>> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "default" | "auto" | "none" => Ok(None),
        "minimal" | "min" => Ok(Some(AiEffort::Minimal)),
        "low" => Ok(Some(AiEffort::Low)),
        "medium" | "med" => Ok(Some(AiEffort::Medium)),
        "high" => Ok(Some(AiEffort::High)),
        "xhigh" | "extra_high" | "extra-high" => Ok(Some(AiEffort::XHigh)),
        "max" => Ok(Some(AiEffort::Max)),
        other => Err(invalid(format!("Unknown AI effort level: {other}"))),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiConfig {
    pub provider: AiProvider,
    pub model: Option<String>,
    pub effort: Option<AiEffort>,
    pub base_url: Option<String>,
    pub gemini_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub custom_cmd: Option<String>,
    pub timeout_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TelegramConfig {
    pub token: String,
    pub chat_id: String,
    pub morning: String,
}

#[derive(Clone, Debug)]
pub struct EnvLookup {
    values: BTreeMap<String, String>,
}

impl EnvLookup {
    /// Precedence: OS env > `<root>/.env` > `<root>/data/.env`.
    pub fn from_root(root: &Path) -> Self {
        let mut values = BTreeMap::new();
        for path in [root.join(".env"), root.join("data/.env")] {
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if !meta.is_file() || meta.len() > MAX_ENV_FILE_BYTES {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&path) {
                for (k, v) in parse_dotenv(&content) {
                    values.entry(k).or_insert(v);
                }
            }
        }
        Self { values }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        std::env::var(key)
            .ok()
            .filter(|v| !v.trim().is_empty() && !v.contains('\0'))
            .or_else(|| {
                self.values
                    .get(key)
                    .filter(|v| !v.trim().is_empty() && !v.contains('\0'))
                    .cloned()
            })
    }

    fn first(&self, keys: &[&str]) -> Option<String> {
        keys.iter().find_map(|key| self.get(key))
    }
}

fn parse_quoted_value(rest: &str, quote: char) -> Option<String> {
    let mut out = String::new();
    let mut chars = rest[quote.len_utf8()..].char_indices();
    while let Some((idx, ch)) = chars.next() {
        if quote == '"' && ch == '\\' {
            if let Some((_, next)) = chars.next() {
                match next {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    '"' | '\\' => out.push(next),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
                continue;
            }
            return None;
        }
        if ch == quote {
            let tail = rest[quote.len_utf8() + idx + ch.len_utf8()..].trim();
            if tail.is_empty() || tail.starts_with('#') {
                return Some(out);
            }
            return None;
        }
        out.push(ch);
    }
    None
}

fn parse_dotenv_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(quote) = trimmed.chars().next().filter(|&c| c == '"' || c == '\'')
        && let Some(parsed) = parse_quoted_value(trimmed, quote)
    {
        return parsed;
    }
    // Strip unquoted trailing comments only after whitespace so `#` inside tokens/URLs isn't broken accidentally.
    let mut prev_ws = false;
    for (idx, ch) in trimmed.char_indices() {
        if ch == '#' && prev_ws {
            return trimmed[..idx].trim_end().to_string();
        }
        prev_ws = ch.is_whitespace();
    }
    trimmed.to_string()
}

pub fn parse_dotenv(content: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in content.lines() {
        if line.contains('\0') {
            continue;
        }
        let mut trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(after) = trimmed.strip_prefix("export")
            && after.starts_with(char::is_whitespace)
        {
            trimmed = after.trim_start();
        }
        let Some((raw_key, raw_val)) = trimmed.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        if key.is_empty() || key.bytes().any(|b| b.is_ascii_whitespace()) {
            continue;
        }
        let val = parse_dotenv_value(raw_val);
        map.entry(key.to_string()).or_insert(val);
    }
    map
}

pub fn parse_provider(raw: &str) -> Result<AiProvider> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => Ok(AiProvider::Auto),
        "gemini" | "gemini-api" => Ok(AiProvider::GeminiApi),
        "openai" | "openai-api" => Ok(AiProvider::OpenAiApi),
        "agy" => Ok(AiProvider::Agy),
        "claude" => Ok(AiProvider::Claude),
        "codex" => Ok(AiProvider::Codex),
        "custom" => Ok(AiProvider::Custom),
        other => Err(invalid(format!("Unknown AI provider: {other}"))),
    }
}

pub fn load_ai_config(root: &Path) -> Result<AiConfig> {
    let env = EnvLookup::from_root(root);
    let provider = match env.get("STUDIA_AI_PROVIDER") {
        Some(raw) => parse_provider(&raw)?,
        None => AiProvider::Auto,
    };
    let model = match env.get("STUDIA_AI_MODEL") {
        Some(m) => {
            let trimmed = m.trim();
            validate_model_identifier(trimmed)?;
            Some(trimmed.to_string())
        }
        None => None,
    };
    let base_url = match env.get("STUDIA_AI_BASE_URL") {
        Some(url) => Some(validate_http_url(&url)?),
        None => None,
    };
    let fallback_key = env.get("STUDIA_AI_API_KEY");
    let gemini_api_key = env
        .first(&["GEMINI_API_KEY", "STUDIA_GEMINI_API_KEY"])
        .or_else(|| {
            (provider == AiProvider::GeminiApi)
                .then(|| fallback_key.clone())
                .flatten()
        });
    let openai_api_key = env
        .first(&["OPENAI_API_KEY", "STUDIA_OPENAI_API_KEY"])
        .or_else(|| {
            (provider == AiProvider::OpenAiApi)
                .then(|| fallback_key.clone())
                .flatten()
        });
    let custom_cmd = env.get("STUDIA_AI_CMD");
    let effort = match env.get("STUDIA_AI_EFFORT") {
        Some(raw) => parse_effort(&raw)?,
        None => None,
    };
    let timeout_secs = match env.get("STUDIA_AI_TIMEOUT_SECS") {
        Some(raw) => raw
            .trim()
            .parse::<u64>()
            .map_err(|_| invalid("STUDIA_AI_TIMEOUT_SECS must be a non-negative integer."))?,
        None => DEFAULT_AI_TIMEOUT_SECS,
    };
    Ok(AiConfig {
        provider,
        model,
        effort,
        base_url,
        gemini_api_key,
        openai_api_key,
        custom_cmd,
        timeout_secs,
    })
}

pub fn load_telegram_config(root: &Path) -> Result<TelegramConfig> {
    let env = EnvLookup::from_root(root);
    let token = env
        .get("STUDIA_TELEGRAM_TOKEN")
        .ok_or_else(|| invalid("STUDIA_TELEGRAM_TOKEN eksik / missing."))?;
    let token_trimmed = token.trim().to_string();
    if token_trimmed.chars().any(char::is_control) {
        return Err(invalid(
            "STUDIA_TELEGRAM_TOKEN contains control characters.",
        ));
    }
    let chat_id = env
        .get("STUDIA_TELEGRAM_CHAT_ID")
        .ok_or_else(|| invalid("STUDIA_TELEGRAM_CHAT_ID eksik / missing."))?;
    let chat_trimmed = chat_id.trim().to_string();
    if chat_trimmed.parse::<i64>().is_err() {
        return Err(invalid(
            "STUDIA_TELEGRAM_CHAT_ID geçersiz / must be a numeric Telegram user ID.",
        ));
    }
    let morning = env
        .get("STUDIA_MORNING")
        .unwrap_or_else(|| "08:30".to_string());
    Ok(TelegramConfig {
        token: token_trimmed,
        chat_id: chat_trimmed,
        morning,
    })
}

pub fn validate_model_identifier(model: &str) -> Result<&str> {
    if model.is_empty()
        || model.starts_with('-')
        || model.contains("..")
        || model.chars().any(|c| c.is_control() || c.is_whitespace())
        || model.contains([
            '/', '\\', '?', '#', '&', '%', '|', '>', '<', '^', '!', ';', '`', '$',
        ])
    {
        return Err(invalid("AI model identifier contains invalid characters."));
    }
    Ok(model)
}

pub fn validate_http_url(url: &str) -> Result<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(invalid(
            "URL must not be empty or contain spaces/control characters.",
        ));
    }
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .ok_or_else(|| invalid("Only http:// or https:// URLs are permitted."))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err(invalid(
            "URL host is missing or contains embedded userinfo.",
        ));
    }
    Ok(trimmed.trim_end_matches('/').to_string())
}

pub fn is_insecure_non_loopback_http(url: &str) -> bool {
    let Some(rest) = url.trim().strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host_part)| host_part);
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        match bracketed.split_once(']') {
            Some((inner, tail)) if tail.is_empty() || tail.starts_with(':') => inner,
            _ => "",
        }
    } else if host_port.matches(':').count() > 1 {
        ""
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    !matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

#[allow(dead_code)]
pub fn env_sensitive_values(root: &Path) -> Vec<String> {
    let lookup = EnvLookup::from_root(root);
    let mut secrets = Vec::new();
    for var in SENSITIVE_ENV_VARS {
        if let Some(val) = lookup.get(var) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                secrets.push(trimmed.to_string());
            }
        }
        if let Some(val) = lookup.values.get(var) {
            let trimmed = val.trim();
            if !trimmed.is_empty() && !trimmed.contains('\0') {
                secrets.push(trimmed.to_string());
            }
        }
    }
    secrets.sort();
    secrets.dedup();
    secrets
}

pub fn ensure_private_dir(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn set_private_file_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    ensure_private_dir(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().flush()?;
    set_private_file_permissions(temp.path())?;
    temp.persist(path).map_err(|err| err.error)?;
    Ok(())
}

pub fn redact_secrets(text: &str, secrets: &[&str]) -> String {
    let mut sorted: Vec<&str> = secrets
        .iter()
        .map(|s| s.trim())
        .filter(|s| s.len() >= 3)
        .collect();
    sorted.sort_by_key(|s| std::cmp::Reverse(s.len()));
    sorted.dedup();
    let mut out = text.to_string();
    for secret in sorted {
        if out.contains(secret) {
            out = out.replace(secret, "[REDACTED]");
        }
    }
    out
}

/// Redacts configured/environment secrets and workspace paths from an error message.
pub fn redact_error(root: &Path, err: &dyn std::fmt::Display) -> String {
    let owned_secrets = env_sensitive_values(root);
    let secret_refs: Vec<&str> = owned_secrets.iter().map(String::as_str).collect();
    let mut msg = redact_secrets(&err.to_string(), &secret_refs);

    let mut prefixes = Vec::new();
    for candidate in [Some(root.to_path_buf()), fs::canonicalize(root).ok()]
        .into_iter()
        .flatten()
    {
        let s = candidate.to_string_lossy().into_owned();
        if s.len() <= 1 || s == "." || s == ".." {
            continue;
        }
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            let clean = stripped.to_string();
            let clean_forward = clean.replace('\\', "/");
            if clean_forward != clean {
                prefixes.push(clean_forward);
            }
            prefixes.push(clean);
        }
        let forward = s.replace('\\', "/");
        if forward != s {
            prefixes.push(forward);
        }
        prefixes.push(s);
    }
    prefixes.sort_by_key(|p| std::cmp::Reverse(p.len()));
    prefixes.dedup();
    for prefix in prefixes {
        if msg.contains(&prefix) {
            msg = msg.replace(&prefix, "<workspace>");
        }
    }
    msg
}

pub fn find_cli_in_path(name: &str) -> Option<PathBuf> {
    let path_os = std::env::var_os("PATH")?;
    find_cli_in_dirs(name, std::env::split_paths(&path_os))
}

pub fn find_cli_in_dirs(name: &str, dirs: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    for dir in dirs {
        #[cfg(windows)]
        {
            for ext in ["exe", "cmd", "bat"] {
                let candidate = dir.join(format!("{name}.{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let plain = dir.join(name);
            if plain.is_file() {
                return Some(plain);
            }
        }
    }
    None
}

pub fn insecure_env_files(root: &Path) -> Vec<PathBuf> {
    #[cfg(unix)]
    let mut flagged = Vec::new();
    #[cfg(not(unix))]
    let flagged = Vec::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [root.join(".env"), root.join("data/.env")] {
            if let Ok(meta) = fs::metadata(&path)
                && meta.is_file()
                && (meta.permissions().mode() & 0o077 != 0)
            {
                flagged.push(path);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = root;
    flagged
}

pub const AI_ENV_VARS: [&str; 10] = [
    "STUDIA_AI_PROVIDER",
    "STUDIA_AI_MODEL",
    "STUDIA_AI_EFFORT",
    "STUDIA_AI_BASE_URL",
    "GEMINI_API_KEY",
    "STUDIA_GEMINI_API_KEY",
    "OPENAI_API_KEY",
    "STUDIA_OPENAI_API_KEY",
    "STUDIA_AI_CMD",
    "STUDIA_AI_TIMEOUT_SECS",
];

pub fn env_overrides() -> Vec<&'static str> {
    AI_ENV_VARS
        .into_iter()
        .filter(|key| {
            std::env::var(key)
                .ok()
                .is_some_and(|v| !v.trim().is_empty() && !v.contains('\0'))
        })
        .collect()
}

pub fn mask_secret_tail(secret: &str) -> String {
    let trimmed = secret.trim();
    let count = trimmed.chars().count();
    if count <= 4 {
        return "••••".to_string();
    }
    let tail: String = trimmed.chars().skip(count.saturating_sub(4)).collect();
    format!("••••{tail}")
}

fn dotenv_key_of_line(line: &str) -> Option<&str> {
    let mut trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    if let Some(after) = trimmed.strip_prefix("export")
        && after.starts_with(char::is_whitespace)
    {
        trimmed = after.trim_start();
    }
    let (raw_key, _) = trimmed.split_once('=')?;
    let key = raw_key.trim();
    if key.is_empty() || key.bytes().any(|b| b.is_ascii_whitespace()) {
        return None;
    }
    Some(key)
}

fn format_dotenv_value(value: &str) -> Result<String> {
    if value.contains(['\0', '\r', '\n']) {
        return Err(invalid(
            "Environment value cannot contain NUL or newline characters.",
        ));
    }
    let needs_quotes = value.is_empty()
        || value != value.trim()
        || value.chars().any(char::is_whitespace)
        || value.contains(['#', '"', '\'', '\\']);
    if !needs_quotes {
        return Ok(value.to_string());
    }
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!("\"{escaped}\""))
}

/// Atomically updates `<root>/.env` keys while preserving comments, blank lines, and unrelated entries.
pub fn save_dotenv_updates(root: &Path, updates: &[(&str, Option<&str>)]) -> Result<PathBuf> {
    let path = root.join(".env");
    let existing = match fs::metadata(&path) {
        Ok(meta) => {
            if !meta.is_file() {
                return Err(invalid(".env path is not a regular file."));
            }
            if meta.len() > MAX_ENV_FILE_BYTES {
                return Err(invalid(".env exceeds 1 MiB limit."));
            }
            fs::read_to_string(&path)?
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err.into()),
    };

    let mut formatted: BTreeMap<&str, Option<String>> = BTreeMap::new();
    for &(key, val) in updates {
        if key.trim().is_empty() || key.bytes().any(|b| b.is_ascii_whitespace() || b == b'=') {
            return Err(invalid(format!("Invalid env key: {key:?}")));
        }
        let next = match val {
            Some(v) => Some(format_dotenv_value(v)?),
            None => None,
        };
        formatted.insert(key, next);
    }

    let mut seen: BTreeMap<&str, bool> = BTreeMap::new();
    let mut lines: Vec<String> = Vec::new();
    for line in existing.lines() {
        if let Some(key) = dotenv_key_of_line(line)
            && let Some(desired) = formatted.get(key)
        {
            match desired {
                Some(val) if !seen.contains_key(key) => {
                    lines.push(format!("{key}={val}"));
                    seen.insert(key, true);
                }
                Some(_) | None => {}
            }
            continue;
        }
        lines.push(line.to_string());
    }

    for &(key, _) in updates {
        if !seen.contains_key(key)
            && let Some(Some(val)) = formatted.get(key)
        {
            lines.push(format!("{key}={val}"));
            seen.insert(key, true);
        }
    }

    let mut content = lines.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    atomic_write_private(&path, content.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_cfg_1_and_3_dotenv_parsing_and_root_precedence() {
        let parsed = parse_dotenv(
            r#"
            # comment
            export OPENAI_API_KEY="sk-abc" # inline comment
            SINGLE='val # keep inside quotes'
            UNQUOTED=hello # tail comment
            MISMATCHED="oops'
            INVALID_NUL=bad
            "#,
        );
        assert_eq!(
            parsed.get("OPENAI_API_KEY").map(String::as_str),
            Some("sk-abc")
        );
        assert_eq!(
            parsed.get("SINGLE").map(String::as_str),
            Some("val # keep inside quotes")
        );
        assert_eq!(parsed.get("UNQUOTED").map(String::as_str), Some("hello"));
        assert_eq!(
            parsed.get("MISMATCHED").map(String::as_str),
            Some("\"oops'")
        );

        let bad_nul = parse_dotenv("SECRET=abc\0def\nOK=1\n");
        assert!(!bad_nul.contains_key("SECRET"));
        assert_eq!(bad_nul.get("OK").map(String::as_str), Some("1"));

        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("data")).unwrap();
        fs::write(
            root.path().join(".env"),
            "STUDIA_LANG=en\nSTUDIA_AI_MODEL=root-model\n",
        )
        .unwrap();
        fs::write(
            root.path().join("data/.env"),
            "STUDIA_LANG=tr\nSTUDIA_TELEGRAM_CHAT_ID=12345\n",
        )
        .unwrap();

        let lookup = EnvLookup::from_root(root.path());
        assert_eq!(lookup.get("STUDIA_LANG").as_deref(), Some("en"));
        assert_eq!(lookup.get("STUDIA_AI_MODEL").as_deref(), Some("root-model"));
        assert_eq!(
            lookup.get("STUDIA_TELEGRAM_CHAT_ID").as_deref(),
            Some("12345")
        );
    }

    #[test]
    fn spec_cfg_2_unrecognized_env_variables_are_ignored() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join(".env"),
            "LEGACY_STUDY_TELEGRAM_TOKEN=legacy-tok\nLEGACY_TELEGRAM_CHAT_ID=99\n",
        )
        .unwrap();
        assert!(load_telegram_config(root.path()).is_err());

        fs::write(
            root.path().join(".env"),
            "STUDIA_TELEGRAM_TOKEN=12345:valid-token\nSTUDIA_TELEGRAM_CHAT_ID=99\n",
        )
        .unwrap();
        let loaded = load_telegram_config(root.path()).unwrap();
        assert_eq!(loaded.token, "12345:valid-token");
        assert_eq!(loaded.chat_id, "99");
    }

    #[test]
    fn spec_cfg_4_provider_url_and_secret_redaction() {
        assert_eq!(parse_provider("auto").unwrap(), AiProvider::Auto);
        assert_eq!(parse_provider("gemini").unwrap(), AiProvider::GeminiApi);
        assert_eq!(parse_provider("openai").unwrap(), AiProvider::OpenAiApi);
        assert_eq!(parse_provider("agy").unwrap(), AiProvider::Agy);
        assert!(parse_provider("gemini-cli").is_err());
        assert!(parse_provider("unknown-provider").is_err());

        assert!(validate_http_url("https://api.openai.com/v1").is_ok());
        assert!(validate_http_url("http://localhost:11434/v1").is_ok());
        assert!(validate_http_url("file:///etc/passwd").is_err());
        assert!(validate_http_url("https://example.com\r\nevil").is_err());

        assert!(!is_insecure_non_loopback_http("https://api.openai.com/v1"));
        assert!(!is_insecure_non_loopback_http("http://localhost:11434/v1"));
        assert!(!is_insecure_non_loopback_http("http://127.0.0.1:8080/v1"));
        assert!(is_insecure_non_loopback_http(
            "http://remote.example.com/v1"
        ));

        let redacted = redact_secrets(
            "failed calling key=sk-secret-999 and tok=bot123",
            &["sk-secret-999", "bot123"],
        );
        assert!(!redacted.contains("sk-secret-999"));
        assert!(!redacted.contains("bot123"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn spec_sec_url_userinfo_ipv6_model_env_cap_and_private_fs() {
        assert!(SENSITIVE_ENV_VARS.contains(&"ANTHROPIC_API_KEY"));

        assert!(validate_http_url("http://user@localhost/v1").is_err());
        assert!(validate_http_url("https://localhost@evil.com/v1").is_err());
        assert!(validate_http_url("http://[::1]:8080/v1").is_ok());

        assert!(is_insecure_non_loopback_http(
            "http://localhost@evil.com/v1"
        ));
        assert!(is_insecure_non_loopback_http(
            "http://127.0.0.1@evil.com/v1"
        ));
        assert!(!is_insecure_non_loopback_http("http://[::1]:8080/v1"));
        assert!(!is_insecure_non_loopback_http("http://[::1]/v1"));
        assert!(is_insecure_non_loopback_http("http://[::1]evil.com/v1"));
        assert!(is_insecure_non_loopback_http("http://[::2]:8080/v1"));

        for good in [
            "gpt-4o-mini",
            "gemini-2.5-flash",
            "llama3:8b",
            "claude-3.5-sonnet",
        ] {
            assert_eq!(validate_model_identifier(good).unwrap(), good);
        }
        for bad in [
            "",
            "-flag",
            "a..b",
            "a/b",
            r"a\b",
            "a?x",
            "a#x",
            "a&x",
            "a%x",
            "a|x",
            "a>x",
            "a<x",
            "a^x",
            "a!x",
            "a;x",
            "a`x",
            "a$x",
            "bad\nmodel",
            "bad model",
        ] {
            assert!(
                validate_model_identifier(bad).is_err(),
                "expected invalid model error for {bad:?}"
            );
        }

        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("data")).unwrap();
        fs::write(
            root.path().join(".env"),
            "ANTHROPIC_API_KEY=sk-ant-test-secret\nSTUDIA_AI_MODEL=-bad-flag\n",
        )
        .unwrap();
        assert!(load_ai_config(root.path()).is_err());

        fs::write(
            root.path().join(".env"),
            "ANTHROPIC_API_KEY=sk-ant-test-secret\nSTUDIA_AI_MODEL=claude-3.5-sonnet\n",
        )
        .unwrap();
        let ai_cfg = load_ai_config(root.path()).unwrap();
        assert_eq!(ai_cfg.model.as_deref(), Some("claude-3.5-sonnet"));
        let env_secrets = env_sensitive_values(root.path());
        assert!(env_secrets.iter().any(|s| s == "sk-ant-test-secret"));

        // Oversized .env (> 1 MiB) is safely skipped by EnvLookup::from_root.
        let oversized = vec![b'A'; (MAX_ENV_FILE_BYTES as usize) + 64];
        fs::write(root.path().join("data/.env"), oversized).unwrap();
        let lookup = EnvLookup::from_root(root.path());
        assert_eq!(
            lookup.get("STUDIA_AI_MODEL").as_deref(),
            Some("claude-3.5-sonnet")
        );

        let huge_root = tempfile::tempdir().unwrap();
        let mut big_env = b"ANTHROPIC_API_KEY=should-be-skipped\n".to_vec();
        big_env.resize((MAX_ENV_FILE_BYTES as usize) + 128, b' ');
        fs::write(huge_root.path().join(".env"), big_env).unwrap();
        assert!(
            EnvLookup::from_root(huge_root.path())
                .get("ANTHROPIC_API_KEY")
                .is_none()
        );

        let private_dir = root.path().join("private");
        let private_file = private_dir.join("secret.txt");
        atomic_write_private(&private_file, b"hello").unwrap();
        assert_eq!(fs::read(&private_file).unwrap(), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir_mode = fs::metadata(&private_dir).unwrap().permissions().mode() & 0o777;
            let file_mode = fs::metadata(&private_file).unwrap().permissions().mode() & 0o777;
            assert_eq!(dir_mode, 0o700);
            assert_eq!(file_mode, 0o600);
        }

        let canon = fs::canonicalize(root.path()).unwrap();
        let canon_str = canon.to_string_lossy();
        let clean_root = canon_str.strip_prefix(r"\\?\").unwrap_or(&canon_str);
        let forward_root = clean_root.replace('\\', "/");
        let err_msg = format!(
            "failed at {} and {canon_str} and {forward_root}/data with sk-ant-test-secret.",
            root.path().display()
        );
        let redacted = redact_error(root.path(), &err_msg);
        assert!(!redacted.contains("sk-ant-test-secret"));
        assert!(!redacted.contains(clean_root));
        assert!(!redacted.contains(&forward_root));
        assert!(redacted.contains("<workspace>/data"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redact_error(Path::new("."), &"network error.").contains("<workspace>."));
    }

    #[test]
    fn save_dotenv_updates_preserves_comments_and_telegram_while_updating_ai() {
        let root = tempfile::tempdir().unwrap();
        let env_path = root.path().join(".env");
        fs::write(
            &env_path,
            "# keep comment\nSTUDIA_TELEGRAM_TOKEN=12345:bot-tok\nexport STUDIA_AI_PROVIDER=auto\nSTUDIA_AI_MODEL=old-model\n",
        )
        .unwrap();

        save_dotenv_updates(
            root.path(),
            &[
                ("STUDIA_AI_PROVIDER", Some("openai")),
                ("STUDIA_AI_MODEL", None),
                ("STUDIA_AI_BASE_URL", Some("http://localhost:11434/v1")),
                ("STUDIA_AI_CMD", Some("my-cli --prompt {prompt_file}")),
            ],
        )
        .unwrap();

        let raw = fs::read_to_string(&env_path).unwrap();
        assert!(raw.contains("# keep comment"));
        assert!(raw.contains("STUDIA_TELEGRAM_TOKEN=12345:bot-tok"));
        let lookup = EnvLookup::from_root(root.path());
        assert_eq!(lookup.get("STUDIA_AI_PROVIDER").as_deref(), Some("openai"));
        assert_eq!(lookup.get("STUDIA_AI_MODEL"), None);
        assert_eq!(
            lookup.get("STUDIA_AI_BASE_URL").as_deref(),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(
            lookup.get("STUDIA_AI_CMD").as_deref(),
            Some("my-cli --prompt {prompt_file}")
        );

        assert!(
            save_dotenv_updates(root.path(), &[("STUDIA_AI_MODEL", Some("bad\nmodel"))]).is_err()
        );
        assert_eq!(mask_secret_tail("sk-test-1234"), "••••1234");
        assert_eq!(mask_secret_tail("abc"), "••••");
        for provider in AiProvider::ALL {
            for model in provider.suggested_models() {
                assert_eq!(validate_model_identifier(model).unwrap(), *model);
            }
        }
        for effort in AiEffort::ALL {
            assert_eq!(parse_effort(effort.as_env_str()).unwrap(), Some(effort));
        }
        assert_eq!(parse_effort("default").unwrap(), None);
        assert!(parse_effort("ultra").is_err());
    }
}
