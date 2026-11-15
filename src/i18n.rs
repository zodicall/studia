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
}
