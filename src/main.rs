mod ai;
mod bot;
mod coach;
mod config;
mod course;
mod exam;
mod i18n;
mod plan;
mod sources;
mod store;
mod telegram;
mod tui;

use config::{AiProvider, EnvLookup};
use i18n::Lang;
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn invalid(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::other(message.into()).into()
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    Tui,
    Listen,
    Index,
    Doctor,
    Help,
    Version,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CliArgs {
    root: PathBuf,
    lang: Option<Lang>,
    action: Action,
}

fn parse_arguments(args: impl IntoIterator<Item = String>) -> Result<CliArgs> {
    let mut root = env::current_dir()?;
    let mut lang = None;
    let mut action = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                return Ok(CliArgs {
                    root,
                    lang,
                    action: Action::Help,
                });
            }
            "-V" | "--version" => {
                return Ok(CliArgs {
                    root,
                    lang,
                    action: Action::Version,
                });
            }
            "--root" => {
                root = PathBuf::from(
                    args.next()
                        .ok_or_else(|| invalid("--root requires a directory path."))?,
                );
            }
            "--lang" => {
                let raw = args
                    .next()
                    .ok_or_else(|| invalid("--lang requires 'tr' or 'en'."))?;
                lang = Some(Lang::parse(&raw).ok_or_else(|| {
                    invalid(format!("Invalid language '{raw}'. Expected 'tr' or 'en'."))
                })?);
            }
            "tui" if action.is_none() => action = Some(Action::Tui),
            "listen" if action.is_none() => action = Some(Action::Listen),
            "index" if action.is_none() => action = Some(Action::Index),
            "doctor" if action.is_none() => action = Some(Action::Doctor),
            _ => {
                let detected = Lang::detect_system(lang, None, None);
                return Err(invalid(detected.cli_usage()));
            }
        }
    }
    Ok(CliArgs {
        root,
        lang,
        action: action.unwrap_or(Action::Tui),
    })
}

fn resolve_cli_lang(root: &Path, cli_lang: Option<Lang>) -> Lang {
    let env_lookup = EnvLookup::from_root(root);
    let env_lang = env_lookup.get("STUDIA_LANG");
    let db_path = root.join("data/studia.db");
    let stored = if db_path.is_file() {
        store::Store::open(&db_path)
            .ok()
            .and_then(|s| s.state("lang").ok().flatten())
            .and_then(|raw| Lang::parse(&raw))
    } else {
        None
    };
    Lang::detect_system(cli_lang, stored, env_lang.as_deref())
}

fn doctor_report(root: &Path, lang: Lang) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "studia {} — {}",
        env!("CARGO_PKG_VERSION"),
        lang.pick("Sistem Kontrolü (çevrimdışı)", "Diagnostics (offline)")
    ));
    lines.push(format!(
        "{}: {}",
        lang.pick("Kök dizin", "Root directory"),
        root.display()
    ));

    let sources_dir = sources::sources_dir(root);
    if sources_dir.is_dir() {
        match sources::subjects(root) {
            Ok(subs) => lines.push(format!(
                "[OK] resources/: {} {}",
                subs.len(),
                lang.pick("ders bulundu", "course(s) found")
            )),
            Err(err) => lines.push(format!("[WARN] resources/: {}", err)),
        }
    } else {
        lines.push(format!(
            "[WARN] {}",
            lang.pick(
                "resources/ henüz yok; ilk dersinizi eklemek için resources/<ders>/ oluşturun.",
                "resources/ not found yet; create resources/<course>/ to add study materials."
            )
        ));
    }

    let curl_name = telegram::CURL_BIN.trim_end_matches(".exe");
    if config::find_cli_in_path(curl_name).is_some() {
        lines.push(format!("[OK] curl ({})", telegram::CURL_BIN));
    } else {
        lines.push(format!(
            "[FAIL] {}",
            lang.pick(
                "PATH içinde curl bulunamadı (Telegram ve HTTP AI için gerekli).",
                "curl not found in PATH (required for Telegram and HTTP AI APIs)."
            )
        ));
    }

    let pdftotext_found = config::find_cli_in_path("pdftotext").is_some() || {
        #[cfg(windows)]
        {
            Path::new(r"C:\Program Files\Git\mingw64\bin\pdftotext.exe").is_file()
        }
        #[cfg(not(windows))]
        {
            false
        }
    };
    if pdftotext_found {
        lines.push("[OK] pdftotext".into());
    } else {
        lines.push(format!(
            "[INFO] pdftotext: {}",
            lang.pick(
                "yüklü değil (isteğe bağlı fallback; gömülü PDF okuyucu aktif).",
                "not installed (optional fallback; built-in PDF extractor active)."
            )
        ));
    }

    match config::load_ai_config(root) {
        Ok(ai_cfg) => {
            let detected = match ai_cfg.provider {
                AiProvider::GeminiApi => format!(
                    "GeminiApi ({})",
                    if ai_cfg.gemini_api_key.is_some() {
                        lang.pick("API anahtarı tanımlı", "API key set")
                    } else {
                        lang.pick("API anahtarı eksik", "API key missing")
                    }
                ),
                AiProvider::OpenAiApi => format!(
                    "OpenAiApi ({})",
                    if ai_cfg.openai_api_key.is_some() {
                        lang.pick("API anahtarı tanımlı", "API key set")
                    } else {
                        lang.pick("API anahtarı eksik", "API key missing")
                    }
                ),
                AiProvider::Agy => format!(
                    "agy CLI ({})",
                    if config::find_cli_in_path("agy").is_some() {
                        "PATH OK"
                    } else {
                        "missing"
                    }
                ),
                AiProvider::Claude => format!(
                    "claude CLI ({})",
                    if config::find_cli_in_path("claude").is_some() {
                        "PATH OK"
                    } else {
                        "missing"
                    }
                ),
                AiProvider::Codex => format!(
                    "codex CLI ({})",
                    if config::find_cli_in_path("codex").is_some() {
                        "PATH OK"
                    } else {
                        "missing"
                    }
                ),
                AiProvider::Custom => format!(
                    "custom ({})",
                    if ai_cfg.custom_cmd.is_some() {
                        "STUDIA_AI_CMD set"
                    } else {
                        "STUDIA_AI_CMD missing"
                    }
                ),
                AiProvider::Auto => {
                    if ai_cfg.gemini_api_key.is_some() {
                        "auto -> GeminiApi (key set)".into()
                    } else if ai_cfg.openai_api_key.is_some() {
                        "auto -> OpenAiApi (key set)".into()
                    } else if let Some(cli) = ["agy", "claude", "codex"]
                        .into_iter()
                        .find(|bin| config::find_cli_in_path(bin).is_some())
                    {
                        format!("auto -> {cli} CLI (PATH)")
                    } else {
                        lang.pick(
                            "auto -> Sağlayıcı bulunamadı (.env veya PATH kontrol edin)",
                            "auto -> No provider detected (configure .env or CLI in PATH)",
                        )
                        .to_string()
                    }
                }
            };
            lines.push(format!("[INFO] AI: {detected}"));
            if let Some(base) = &ai_cfg.base_url
                && config::is_insecure_non_loopback_http(base)
            {
                lines.push(format!(
                    "[WARN] {}",
                    lang.pick(
                        "STUDIA_AI_BASE_URL loopback dışında şifresiz http:// kullanıyor.",
                        "STUDIA_AI_BASE_URL uses unencrypted http:// for a non-loopback host."
                    )
                ));
            }
        }
        Err(err) => lines.push(format!("[FAIL] AI config: {err}")),
    }

    match config::load_telegram_config(root) {
        Ok(_) => lines.push(format!(
            "[OK] Telegram: {}",
            lang.pick("kimlik bilgileri tanımlı", "credentials configured")
        )),
        Err(_) => lines.push(format!(
            "[INFO] Telegram: {}",
            lang.pick(
                "tanımlı değil (yalnızca `listen` için gerekli)",
                "not configured (only required for `listen`)"
            )
        )),
    }

    for insecure in config::insecure_env_files(root) {
        lines.push(format!(
            "[WARN] {}: {} (chmod 600)",
            lang.pick(
                "Ortam dosyası izinleri çok açık",
                "Env file permissions are too broad"
            ),
            insecure.display()
        ));
    }

    let db_path = root.join("data/studia.db");
    if db_path.is_file() {
        match store::Store::open(&db_path).and_then(|s| s.quick_check()) {
            Ok(true) => lines.push("[OK] SQLite PRAGMA quick_check: ok".into()),
            Ok(false) => lines.push("[FAIL] SQLite PRAGMA quick_check failed".into()),
            Err(err) => lines.push(format!("[FAIL] SQLite: {err}")),
        }
    } else {
        lines.push(format!(
            "[INFO] SQLite: {}",
            lang.pick(
                "henüz oluşturulmadı (ilk çalıştırmada oluşacak)",
                "not created yet (initialized on first run)"
            )
        ));
    }

    if fs::metadata(root).is_ok() {
        lines.push(format!(
            "[OK] {}",
            lang.pick("Kök dizin erişilebilir", "Root directory accessible")
        ));
    } else {
        lines.push(format!(
            "[WARN] {}",
            lang.pick("Kök dizin henüz yok", "Root directory does not exist yet")
        ));
    }

    lines.join("\n")
}

fn run() -> Result<()> {
    let parsed = parse_arguments(env::args().skip(1))?;
    let lang = resolve_cli_lang(&parsed.root, parsed.lang);
    match parsed.action {
        Action::Help => println!("{}", lang.cli_usage()),
        Action::Version => println!("studia {}", env!("CARGO_PKG_VERSION")),
        Action::Doctor => println!("{}", doctor_report(&parsed.root, lang)),
        Action::Index => println!(
            "{} {}",
            sources::index(&parsed.root, true)?,
            lang.pick("dosya güncellendi.", "file(s) updated.")
        ),
        Action::Listen => bot::listen_with_cli(&parsed.root, lang, parsed.lang)?,
        Action::Tui => tui::run_with(&parsed.root, lang)?,
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_global_and_defaults_to_current_directory() {
        assert_eq!(
            parse_arguments(["index".into()]).unwrap().root,
            env::current_dir().unwrap()
        );
        for args in [
            ["--root", "materials", "listen"],
            ["listen", "--root", "materials"],
        ] {
            let parsed = parse_arguments(args.map(String::from)).unwrap();
            assert_eq!(parsed.root, PathBuf::from("materials"));
            assert_eq!(parsed.action, Action::Listen);
        }
        for args in [vec!["--root"], vec!["listen", "index"], vec!["other"]] {
            assert!(parse_arguments(args.into_iter().map(String::from)).is_err());
        }
        assert_eq!(parse_arguments([]).unwrap().action, Action::Tui);
        assert_eq!(parse_arguments(["tui".into()]).unwrap().action, Action::Tui);
    }

    #[test]
    fn spec_cli_1_3_lang_help_version_and_doctor() {
        let doc =
            parse_arguments(["--root", "ws", "--lang", "tr", "doctor"].map(String::from)).unwrap();
        assert_eq!(doc.root, PathBuf::from("ws"));
        assert_eq!(doc.lang, Some(Lang::Tr));
        assert_eq!(doc.action, Action::Doctor);

        assert_eq!(
            parse_arguments(["--help".into()]).unwrap().action,
            Action::Help
        );
        assert_eq!(
            parse_arguments(["-V".into()]).unwrap().action,
            Action::Version
        );
        assert!(parse_arguments(["--lang".into(), "fr".into()]).is_err());
        assert!(parse_arguments(["--lang".into()]).is_err());
    }

    #[test]
    fn spec_doc_1_doctor_offline_diagnostics_and_no_secret_leak() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("data")).unwrap();
        fs::write(
            dir.path().join(".env"),
            "GEMINI_API_KEY=AIzaSecretSentinel999\n\
             OPENAI_API_KEY=sk-secret-sentinel-888\n\
             STUDIA_TELEGRAM_TOKEN=99999:TelegramSecretToken\n\
             STUDIA_TELEGRAM_CHAT_ID=424242\n\
             STUDIA_AI_BASE_URL=http://remote-llm.example.com/v1\n",
        )
        .unwrap();
        let store = store::Store::open(&dir.path().join("data/studia.db")).unwrap();
        assert!(store.quick_check().unwrap());
        drop(store);

        let report = doctor_report(dir.path(), Lang::En);
        assert!(report.contains("Diagnostics (offline)"));
        assert!(report.contains("SQLite PRAGMA quick_check: ok"));
        assert!(report.contains("credentials configured"));
        assert!(report.contains("unencrypted http://"));
        assert!(!report.contains("AIzaSecretSentinel999"));
        assert!(!report.contains("sk-secret-sentinel-888"));
        assert!(!report.contains("TelegramSecretToken"));
        assert!(!report.contains("424242"));
    }

    #[test]
    fn spec_ai_3_find_cli_in_dirs_windows_extension_priority() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("claude"), "#!/bin/sh\n").unwrap();
        #[cfg(windows)]
        {
            fs::write(dir.path().join("claude.cmd"), "@echo off\r\n").unwrap();
            let found = config::find_cli_in_dirs("claude", [dir.path().to_path_buf()]).unwrap();
            assert_eq!(found, dir.path().join("claude.cmd"));
        }
        #[cfg(not(windows))]
        {
            let found = config::find_cli_in_dirs("claude", [dir.path().to_path_buf()]).unwrap();
            assert_eq!(found, dir.path().join("claude"));
        }
    }
}
