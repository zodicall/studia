use crate::{Result, invalid};
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
    GeminiCli,
    Custom,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiConfig {
    pub provider: AiProvider,
    pub model: Option<String>,
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
        "gemini-cli" => Ok(AiProvider::GeminiCli),
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
        assert_eq!(parse_provider("gemini-cli").unwrap(), AiProvider::GeminiCli);
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
    }
}
