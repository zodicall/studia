use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Tr,
    En,
}

impl Lang {
    pub fn parse(raw: &str) -> Option<Self> {
        let lower = raw.trim().to_ascii_lowercase();
        if lower == "tr" || lower.starts_with("tr_") || lower.starts_with("tr-") {
            Some(Self::Tr)
        } else if lower == "en" || lower.starts_with("en_") || lower.starts_with("en-") {
            Some(Self::En)
        } else {
            None
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Tr => "tr",
            Self::En => "en",
        }
    }

    pub fn from_locale(raw: &str) -> Self {
        let lower = raw.trim().to_ascii_lowercase();
        if lower == "tr" || lower.starts_with("tr_") || lower.starts_with("tr-") {
            Self::Tr
        } else {
            Self::En
        }
    }

    pub fn detect(
        cli: Option<Self>,
        stored: Option<Self>,
        env_lang: Option<&str>,
        lc_all: Option<&str>,
        lang_env: Option<&str>,
    ) -> Self {
        if let Some(lang) = cli {
            return lang;
        }
        if let Some(lang) = stored {
            return lang;
        }
        if let Some(lang) = env_lang.and_then(Self::parse) {
            return lang;
        }
        for locale in [lc_all, lang_env].into_iter().flatten() {
            let trimmed = locale.trim();
            if !trimmed.is_empty() {
                return Self::from_locale(trimmed);
            }
        }
        Self::En
    }

    pub fn detect_system(cli: Option<Self>, stored: Option<Self>, env_lang: Option<&str>) -> Self {
        let lc_all = std::env::var("LC_ALL").ok();
        let lang_env = std::env::var("LANG").ok();
        Self::detect(
            cli,
            stored,
            env_lang,
            lc_all.as_deref(),
            lang_env.as_deref(),
        )
    }

    pub fn pick<'a>(self, tr: &'a str, en: &'a str) -> &'a str {
        match self {
            Self::Tr => tr,
            Self::En => en,
        }
    }

    /// Returns the default native-language display name corresponding to this UI language.
    pub fn default_native_name(self) -> &'static str {
        match self {
            Self::Tr => "Türkçe",
            Self::En => "English",
        }
    }

    pub fn cli_usage(self) -> &'static str {
        self.pick(
            "Kullanım: studia [--root <dizin>] [--lang <tr|en>] [tui|listen|index|doctor]\n\nSeçenekler:\n  --root <dizin>    Çalışma kök dizini (varsayılan: geçerli dizin)\n  --lang <tr|en>    Arayüz ve AI dili\n  -h, --help        Yardımı göster\n  -V, --version     Sürümü göster",
            "Usage: studia [--root <dir>] [--lang <tr|en>] [tui|listen|index|doctor]\n\nOptions:\n  --root <dir>      Workspace root directory (default: current directory)\n  --lang <tr|en>    UI and AI language\n  -h, --help        Show help\n  -V, --version     Show version",
        )
    }
}

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Common native language presets selectable in Settings via `←/→`.
pub const NATIVE_LANGUAGE_PRESETS: &[&str] = &[
    "Türkçe",
    "English",
    "Deutsch",
    "Français",
    "Español",
    "Italiano",
    "العربية",
    "Русский",
    "日本語",
    "한국어",
    "中文",
];

/// Sanitizes and normalizes a user-chosen native language label.
pub fn normalize_native_lang(raw: &str) -> String {
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let lower = trimmed.to_ascii_lowercase();
    if matches!(lower.as_str(), "tr" | "turkish" | "türkçe" | "turkce") {
        "Türkçe".into()
    } else if matches!(lower.as_str(), "en" | "english" | "ingilizce") {
        "English".into()
    } else {
        trimmed.into()
    }
}

/// Lightweight heuristic that returns `true` when `text` likely belongs to a foreign
/// language relative to `native_lang`, used solely to highlight the `[t]` translate hint.
pub fn looks_foreign_for_native(text: &str, native_lang: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.chars().count() < 24 {
        return false;
    }
    let norm = normalize_native_lang(native_lang).to_ascii_lowercase();
    if norm == "türkçe" || norm == "turkish" {
        let has_tr_char = trimmed.chars().any(|c| {
            matches!(
                c,
                'ç' | 'ğ' | 'ı' | 'ö' | 'ş' | 'ü' | 'Ç' | 'Ğ' | 'İ' | 'Ö' | 'Ş' | 'Ü'
            )
        });
        if has_tr_char {
            return false;
        }
        let lower = format!(" {} ", trimmed.to_ascii_lowercase());
        if lower.contains("correct answer:") {
            return true;
        }
        return [
            " the ",
            " is ",
            " are ",
            " because ",
            " answer ",
            " with ",
            " that ",
            " this ",
            " which ",
        ]
        .iter()
        .any(|kw| lower.contains(kw));
    }
    if norm == "english" {
        return trimmed.chars().any(|c| c.is_alphabetic() && !c.is_ascii());
    }
    // For other native languages, emphasize if text appears purely ASCII-alphabetic English.
    let mut has_alpha = false;
    let mut all_ascii = true;
    for ch in trimmed.chars() {
        if ch.is_alphabetic() {
            has_alpha = true;
            if !ch.is_ascii() {
                all_ascii = false;
                break;
            }
        }
    }
    has_alpha && all_ascii
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_i18n_1_detection_precedence_and_parsing() {
        assert_eq!(Lang::parse("tr"), Some(Lang::Tr));
        assert_eq!(Lang::parse("EN"), Some(Lang::En));
        assert_eq!(Lang::parse("tr_TR.UTF-8"), Some(Lang::Tr));
        assert_eq!(Lang::parse("en-US"), Some(Lang::En));
        assert_eq!(Lang::parse("fr"), None);

        // CLI flag beats stored, env, and locale.
        assert_eq!(
            Lang::detect(
                Some(Lang::En),
                Some(Lang::Tr),
                Some("tr"),
                Some("tr_TR.UTF-8"),
                Some("tr_TR")
            ),
            Lang::En
        );
        // Stored DB choice beats env and locale.
        assert_eq!(
            Lang::detect(None, Some(Lang::Tr), Some("en"), Some("en_US"), None),
            Lang::Tr
        );
        // STUDIA_LANG beats OS locale.
        assert_eq!(
            Lang::detect(None, None, Some("tr"), Some("en_US.UTF-8"), None),
            Lang::Tr
        );
        // OS locale tr* defaults to Turkish, others default to English.
        assert_eq!(
            Lang::detect(None, None, None, Some("tr_TR.UTF-8"), None),
            Lang::Tr
        );
        assert_eq!(
            Lang::detect(None, None, None, None, Some("en_US.UTF-8")),
            Lang::En
        );
        assert_eq!(Lang::detect(None, None, None, None, None), Lang::En);
    }

    #[test]
    fn spec_i18n_2_pick_returns_expected_language_text() {
        assert_eq!(Lang::Tr.pick("Merhaba", "Hello"), "Merhaba");
        assert_eq!(Lang::En.pick("Merhaba", "Hello"), "Hello");
    }

    #[test]
    fn spec_i18n_3_native_lang_normalization_and_foreign_heuristic() {
        assert_eq!(normalize_native_lang("  tr\n "), "Türkçe");
        assert_eq!(normalize_native_lang("english"), "English");
        assert_eq!(
            normalize_native_lang("  Brazilian\x00 Portuguese  "),
            "Brazilian Portuguese"
        );
        assert_eq!(normalize_native_lang("   \t\n "), "");

        assert!(looks_foreign_for_native(
            "Correct answer: 'however'. This sentence shows a contrast between two clauses.",
            "Türkçe"
        ));
        assert!(!looks_foreign_for_native(
            "Doğru cevap: 'however'. Bu cümlede iki yargı arasında zıtlık vardır.",
            "Türkçe"
        ));
        assert!(!looks_foreign_for_native(
            "Correct answer: 'however' — it’s used for contrast ✅.",
            "English"
        ));
        assert!(looks_foreign_for_native(
            "Bu açıklama Türkçe olarak yazılmıştır ve bağlaç kullanımını açıklar.",
            "English"
        ));
        assert!(looks_foreign_for_native(
            "The correct answer uses passive voice because the subject receives the action.",
            "Deutsch"
        ));
    }
}
