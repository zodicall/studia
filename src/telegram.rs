use crate::{Result, config, invalid};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub const MAX_HTTP_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

#[cfg(windows)]
pub const CURL_BIN: &str = "curl.exe";
#[cfg(not(windows))]
pub const CURL_BIN: &str = "curl";

pub fn credentials() -> Result<(String, String)> {
    credentials_for_root(&std::env::current_dir()?)
}

pub fn credentials_for_root(root: &Path) -> Result<(String, String)> {
    let cfg = config::load_telegram_config(root)?;
    Ok((cfg.token, cfg.chat_id))
}

fn quote_curl_str(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn telegram_request(token: &str, method: &str, fields: &[(&str, &str)]) -> Result<String> {
    if token.trim().is_empty()
        || method.trim().is_empty()
        || token.chars().any(char::is_control)
        || method.chars().any(char::is_control)
    {
        return Err(invalid(
            "Telegram token or method contains invalid control characters.",
        ));
    }
    let mut cfg = format!(
        "url = \"https://api.telegram.org/bot{}/{}\"\n",
        quote_curl_str(token.trim()),
        quote_curl_str(method.trim())
    );
    cfg.push_str("connect-timeout = 10\n");
    cfg.push_str("max-time = 70\n");
    cfg.push_str(&format!("max-filesize = {MAX_HTTP_BYTES}\n"));
    let document = method == "sendDocument";
    for (key, value) in fields {
        if key.chars().any(char::is_control) || value.contains('\0') {
            return Err(invalid(
                "Telegram field contains invalid control character.",
            ));
        }
        if !document {
            cfg.push_str(&format!(
                "data-urlencode = \"{}={}\"\n",
                quote_curl_str(key),
                quote_curl_str(value)
            ));
        } else if *key == "document" {
            let normalized = value.replace('\\', "/");
            if normalized.contains(['\0', '\r', '\n', '"', ';', ',']) {
                return Err(invalid("Document path contains unsafe characters."));
            }
            cfg.push_str(&format!(
                "form = \"{}\"\n",
                quote_curl_str(&format!("{key}=@\"{normalized}\""))
            ));
        } else {
            cfg.push_str(&format!(
                "form-string = \"{}={}\"\n",
                quote_curl_str(key),
                quote_curl_str(value)
            ));
        }
    }
    Ok(cfg)
}

pub fn build_http_post_config(
    url: &str,
    headers: &[(&str, &str)],
    body_file: &Path,
    timeout_secs: u64,
) -> Result<String> {
    let valid_url = config::validate_http_url(url)?;
    let path_str = body_file
        .to_str()
        .ok_or_else(|| invalid("HTTP body file path is not valid UTF-8."))?
        .replace('\\', "/");
    if path_str.contains(['\0', '\r', '\n', '"']) {
        return Err(invalid("HTTP body file path contains unsafe characters."));
    }
    let mut cfg = format!("url = \"{}\"\n", quote_curl_str(&valid_url));
    cfg.push_str("connect-timeout = 10\n");
    if timeout_secs > 0 {
        cfg.push_str(&format!("max-time = {timeout_secs}\n"));
    }
    cfg.push_str(&format!("max-filesize = {MAX_HTTP_BYTES}\n"));
    cfg.push_str("write-out = \"\\n%{http_code}\"\n");
    let mut has_content_type = false;
    for (name, value) in headers {
        if name.trim().is_empty()
            || name.chars().any(char::is_control)
            || value.chars().any(char::is_control)
            || name.contains(':')
        {
            return Err(invalid("HTTP header contains unsafe characters."));
        }
        if name.eq_ignore_ascii_case("content-type") {
            has_content_type = true;
        }
        cfg.push_str(&format!(
            "header = \"{}: {}\"\n",
            quote_curl_str(name.trim()),
            quote_curl_str(value)
        ));
    }
    if !has_content_type {
        cfg.push_str("header = \"Content-Type: application/json\"\n");
    }
    cfg.push_str(&format!(
        "data-binary = \"@{}\"\n",
        quote_curl_str(&path_str)
    ));
    Ok(cfg)
}

pub fn parse_http_status_and_body(raw: &str) -> Result<(u16, String)> {
    let (body, code_str) = raw
        .rsplit_once('\n')
        .ok_or_else(|| invalid("HTTP response missing status code."))?;
    let code: u16 = code_str
        .trim()
        .parse()
        .map_err(|_| invalid("HTTP response status code invalid."))?;
    Ok((code, body.to_string()))
}

pub fn http_post_json(
    url: &str,
    headers: &[(&str, &str)],
    body_file: &Path,
    timeout_secs: u64,
    secrets: &[&str],
) -> Result<(u16, String)> {
    let cfg = build_http_post_config(url, headers, body_file, timeout_secs)?;
    let mut cmd = Command::new(CURL_BIN);
    cmd.args(["-sS", "-K", "-"]);
    let timeout = (timeout_secs > 0).then(|| Duration::from_secs(timeout_secs.saturating_add(5)));
    let raw = command_output_with_timeout(&mut cmd, &cfg, timeout, MAX_HTTP_BYTES, secrets)
        .map_err(|err| invalid(config::redact_secrets(&err.to_string(), secrets)))?;
    parse_http_status_and_body(&raw)
}

fn terminate_child(child: &mut Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let pid = child.id().to_string();
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid])
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub fn command_output_with_timeout(
    command: &mut Command,
    input: &str,
    timeout: Option<Duration>,
    max_bytes: usize,
    secrets: &[&str],
) -> Result<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| invalid(config::redact_secrets(&err.to_string(), secrets)))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| invalid("CLI stdin eksik / missing."))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid("CLI stdout eksik / missing."))?;

    let input = input.to_owned();
    let writer = thread::spawn(move || match stdin.write_all(input.as_bytes()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(err),
    });

    let (tx, rx) = mpsc::sync_channel::<io::Result<(Vec<u8>, bool)>>(1);
    let _reader = thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut out = Vec::new();
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(Ok((out, false)));
                    break;
                }
                Ok(n) => {
                    if out.len().saturating_add(n) > max_bytes {
                        let _ = tx.send(Ok((out, true)));
                        break;
                    }
                    out.extend_from_slice(&buf[..n]);
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    let _ = tx.send(Err(err));
                    break;
                }
            }
        }
    });

    let deadline = timeout.map(|d| Instant::now() + d);
    let mut step = Duration::from_millis(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if let Some(limit) = deadline
                    && Instant::now() >= limit
                {
                    terminate_child(&mut child);
                    return Err(invalid(
                        "Alt süreç zaman aşımına uğradı / Subprocess timed out.",
                    ));
                }
                thread::sleep(step);
                step = (step * 2).min(Duration::from_millis(40));
            }
            Err(err) => {
                terminate_child(&mut child);
                return Err(invalid(config::redact_secrets(&err.to_string(), secrets)));
            }
        }
    };

    writer
        .join()
        .map_err(|_| invalid("CLI stdin yazılamadı."))?
        .map_err(|err| invalid(config::redact_secrets(&err.to_string(), secrets)))?;

    let (bytes, truncated) = rx
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| invalid("CLI stdout okunamadı / timed out reading stdout."))?
        .map_err(|err| invalid(config::redact_secrets(&err.to_string(), secrets)))?;

    if truncated {
        return Err(invalid(
            "Alt süreç çıktısı boyut sınırını aştı / Subprocess output exceeded size cap.",
        ));
    }
    if !status.success() {
        return Err(invalid(
            "CLI çalıştırılamadı / Subprocess exited with failure.",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|err| invalid(config::redact_secrets(&err.to_string(), secrets)))
}

fn parse_response(output: Result<String>) -> Result<Value> {
    // Never propagate curl output, URLs, API descriptions, or parse error details.
    let raw = output.map_err(|_| invalid("Telegram isteği başarısız."))?;
    let response: Value =
        serde_json::from_str(&raw).map_err(|_| invalid("Telegram yanıtı geçerli JSON değil."))?;
    if response["ok"] != true {
        return Err(invalid("Telegram API hatası."));
    }
    Ok(response)
}

pub fn telegram_call_with_token(
    token: &str,
    method: &str,
    fields: &[(&str, &str)],
) -> Result<Value> {
    let cfg = telegram_request(token, method, fields)?;
    let mut cmd = Command::new(CURL_BIN);
    cmd.args(["-sS", "-K", "-"]);
    parse_response(command_output_with_timeout(
        &mut cmd,
        &cfg,
        Some(Duration::from_secs(75)),
        MAX_HTTP_BYTES,
        &[token],
    ))
}

pub fn telegram_call(method: &str, fields: &[(&str, &str)]) -> Result<Value> {
    let (token, _) = credentials()?;
    telegram_call_with_token(&token, method, fields)
}

pub fn accepted(update: &Value, chat_id: &str) -> bool {
    let (message, from) = if let Some(callback) = update.get("callback_query") {
        (&callback["message"], &callback["from"])
    } else {
        (&update["message"], &update["message"]["from"])
    };
    let matches = |id: &Value| {
        id.as_i64()
            .is_some_and(|id| chat_id.parse::<i64>() == Ok(id))
    };
    matches(&message["chat"]["id"]) && matches(&from["id"]) && message["chat"]["type"] == "private"
}

pub fn open_listener_lock(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock().map_err(|err| match err {
        TryLockError::WouldBlock => {
            invalid("Dinleyici kilidi zaten alınmış / Listener lock already held.")
        }
        TryLockError::Error(io_err) => io_err.into(),
    })?;
    Ok(file)
}

pub fn listener_running(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
        return false;
    };
    matches!(file.try_lock(), Err(TryLockError::WouldBlock))
}

pub fn log(root: &Path, message: &str) {
    log_file(root, message);
    eprintln!("{message}");
}

pub fn log_file(root: &Path, message: &str) {
    // Callers supply fixed messages only; never log incoming text, tokens, or process output.
    let data_dir = root.join("data");
    if fs::create_dir_all(&data_dir).is_err() {
        return;
    }
    let log_path = data_dir.join("bot.log");
    if let Ok(meta) = fs::metadata(&log_path)
        && meta.len() >= MAX_LOG_BYTES
    {
        let backup = data_dir.join("bot.log.1");
        let _ = fs::remove_file(&backup);
        let _ = fs::rename(&log_path, &backup);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = writeln!(file, "{} {message}", chrono::Utc::now().to_rfc3339());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_quotes_stdin_config_and_preserves_field_whitespace() {
        let config =
            telegram_request("secret", "sendMessage", &[("text", " a\\\"\n\r\tb ")]).unwrap();
        assert!(config.starts_with("url = \"https://api.telegram.org/botsecret/sendMessage\"\n"));
        assert!(config.contains("data-urlencode = \"text= a\\\\\\\"\\n\\r\\tb \"\n"));
        assert_eq!(
            config
                .lines()
                .filter(|line| line.starts_with("data-urlencode"))
                .count(),
            1
        );
    }

    #[test]
    fn send_document_uses_multipart_form() {
        let config = telegram_request(
            "t",
            "sendDocument",
            &[
                ("chat_id", "42"),
                ("document", "C:/x/a.txt"),
                ("caption", "c"),
            ],
        )
        .unwrap();
        assert!(config.contains("form-string = \"chat_id=42\"\n"));
        assert!(config.contains("form = \"document=@\\\"C:/x/a.txt\\\"\"\n"));
        assert!(!config.contains("data-urlencode"));
    }

    #[test]
    fn spec_sec_2_curl_injection_and_unsafe_paths_rejected() {
        assert!(telegram_request("tok\nevil", "sendMessage", &[]).is_err());
        assert!(telegram_request("tok", "sendMessage", &[("text", "ok\0")]).is_err());
        assert!(
            telegram_request(
                "tok",
                "sendDocument",
                &[("document", r"C:\tmp\evil;type=text/html")]
            )
            .is_err()
        );
        assert!(
            build_http_post_config("file:///etc/passwd", &[], Path::new("a.json"), 30).is_err()
        );
        assert!(
            build_http_post_config(
                "https://api.openai.com/v1/chat/completions",
                &[("Authorization", "Bearer bad\r\nevil")],
                Path::new("a.json"),
                30,
            )
            .is_err()
        );
        let ok_cfg = build_http_post_config(
            "https://api.openai.com/v1/chat/completions",
            &[("Authorization", "Bearer sk-ok")],
            Path::new(r"C:\Temp\body.json"),
            30,
        )
        .unwrap();
        assert!(ok_cfg.contains("data-binary = \"@C:/Temp/body.json\""));
        assert!(ok_cfg.contains("connect-timeout = 10"));
        assert!(ok_cfg.contains("max-filesize = 16777216"));
    }

    #[test]
    fn spec_lock_1_and_stab_3_cross_platform_try_lock_probe() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("listen.lock");
        assert!(!listener_running(&lock_path));
        let guard = open_listener_lock(&lock_path).unwrap();
        assert!(listener_running(&lock_path));
        assert!(open_listener_lock(&lock_path).is_err());
        drop(guard);
        assert!(!listener_running(&lock_path));
        assert!(open_listener_lock(&lock_path).is_ok());
    }

    #[test]
    fn spec_stab_2_log_rotation_creates_backup_and_caps_size() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("data")).unwrap();
        let big = vec![b'x'; (MAX_LOG_BYTES as usize) + 64];
        fs::write(dir.path().join("data/bot.log"), big).unwrap();
        log_file(dir.path(), "rotated event");
        assert!(dir.path().join("data/bot.log.1").exists());
        let active = fs::read_to_string(dir.path().join("data/bot.log")).unwrap();
        assert!(active.contains("rotated event"));
        assert!(active.len() < 1024);
    }

    #[test]
    fn spec_tx_2_and_sec_1_timeout_and_large_stdout_do_not_deadlock_or_leak() {
        let start = Instant::now();
        #[cfg(windows)]
        let mut hung = {
            let mut c = Command::new("powershell");
            c.args([
                "-NoProfile",
                "-Command",
                "1..2000 | % { 'x' * 80 }; Start-Sleep -Seconds 15",
            ]);
            c
        };
        #[cfg(not(windows))]
        let mut hung = {
            let mut c = Command::new("sh");
            c.args(["-c", "yes 1234567890 | head -n 4000; sleep 15"]);
            c
        };
        let err = command_output_with_timeout(
            &mut hung,
            "",
            Some(Duration::from_millis(500)),
            1024 * 1024,
            &["TOP_SECRET_KEY"],
        )
        .unwrap_err()
        .to_string();
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(350) && elapsed < Duration::from_secs(6),
            "unexpected timeout duration: {elapsed:?}"
        );
        assert!(err.contains("zaman aşımı") || err.contains("timed out"));
        assert!(!err.contains("TOP_SECRET_KEY"));

        // Verify happy path with >64KB stdout completes cleanly and ignores BrokenPipe on short readers.
        #[cfg(windows)]
        let mut fast = {
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-Command", "1..1200 | % { 'a' * 80 }"]);
            c
        };
        #[cfg(not(windows))]
        let mut fast = {
            let mut c = Command::new("sh");
            c.args(["-c", "yes 1234567890 | head -n 8000"]);
            c
        };
        let ok = command_output_with_timeout(
            &mut fast,
            &"unused-stdin\n".repeat(4000),
            Some(Duration::from_secs(10)),
            2 * 1024 * 1024,
            &[],
        )
        .unwrap();
        assert!(ok.len() > 70_000);
    }

    #[test]
    fn spec_tx_1_parse_http_status_and_body_handles_multiline() {
        let (code, body) = parse_http_status_and_body("{\"a\":1}\nnext\n200").unwrap();
        assert_eq!(code, 200);
        assert_eq!(body, "{\"a\":1}\nnext");
        assert!(parse_http_status_and_body("no-newline").is_err());
    }

    #[test]
    fn token_is_absent_from_every_response_error() {
        let token = "PRIVATE_TOKEN_SENTINEL";
        for output in [
            Err(invalid(format!("https://api.telegram.org/bot{token}"))),
            Ok(format!("{token} bad JSON")),
            Ok(json!({"ok":false,"description":token}).to_string()),
        ] {
            assert!(
                !parse_response(output)
                    .unwrap_err()
                    .to_string()
                    .contains(token)
            );
        }
    }

    #[test]
    fn private_owner_only_for_messages_and_callbacks() {
        let message = json!({"chat":{"id":42,"type":"private"},"from":{"id":42},"text":"hello"});
        let valid = [
            json!({"message":message}),
            json!({"callback_query":{"from":{"id":42},"message":message}}),
        ];
        for update in valid {
            assert!(accepted(&update, "42"));
            let prefix = if update.get("callback_query").is_some() {
                "/callback_query/message"
            } else {
                "/message"
            };
            let from = if prefix.starts_with("/callback") {
                "/callback_query/from/id".to_string()
            } else {
                format!("{prefix}/from/id")
            };
            for (path, value) in [
                (format!("{prefix}/chat/id"), json!(7)),
                (format!("{prefix}/chat/type"), json!("group")),
                (from, json!(7)),
            ] {
                let mut bad = update.clone();
                *bad.pointer_mut(&path).unwrap() = value;
                assert!(!accepted(&bad, "42"));
            }
        }
        assert!(!accepted(&json!({}), "42"));
    }
}
