use crate::{
    Result, ai,
    config::{self, AiConfig, AiEffort, AiProvider},
    course::{DayConfig, TaskConfig},
    i18n::Lang,
    invalid,
};

#[derive(Clone, Debug)]
pub enum Modal {
    PlanEdit(PlanEditModal),
    AiSettings(Box<AiSettingsModal>),
}

#[derive(Clone, Debug)]
pub enum PlanEditPrompt {
    DayTitle {
        is_new: bool,
        text: String,
    },
    TaskForm {
        is_new: bool,
        field: usize,
        cmd_kind: usize,
        arg: String,
        label: String,
    },
}

pub const TASK_CMD_KINDS: [&str; 6] = ["", "/anlat", "/quiz", "/kelime", "/tekrar", "/deneme"];

impl PlanEditPrompt {
    pub fn from_task(task: &TaskConfig, is_new: bool) -> Self {
        let trimmed = task.command.trim();
        let (head, arg) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));
        let cmd_kind = match head {
            "/anlat" | "/lesson" | "/explain" => 1,
            "/quiz" | "/kazanim" | "/practice" => 2,
            "/kelime" | "/cards" => 3,
            "/tekrar" | "/review" => 4,
            "/deneme" | "/exam" => 5,
            _ => 0,
        };
        Self::TaskForm {
            is_new,
            field: 0,
            cmd_kind,
            arg: arg.trim().to_string(),
            label: task.label.clone(),
        }
    }

    pub fn build_task(cmd_kind: usize, arg: &str, label: &str, lang: Lang) -> TaskConfig {
        let prefix = TASK_CMD_KINDS.get(cmd_kind).copied().unwrap_or("");
        let arg = arg.trim();
        let command = match cmd_kind {
            0 => String::new(),
            4 | 5 if arg.is_empty() => prefix.to_string(),
            _ if arg.is_empty() => prefix.to_string(),
            _ => format!("{prefix} {arg}"),
        };
        let label = label.trim();
        let final_label = if !label.is_empty() {
            label.to_string()
        } else {
            match cmd_kind {
                1 => format!("{}: {arg}", lang.pick("Ders", "Lesson")),
                2 => format!("Quiz: {arg}"),
                3 => format!("{}: {arg}", lang.pick("Kart", "Cards")),
                4 => lang
                    .pick("Tekrar: Vadesi gelenler", "Review: Due items")
                    .into(),
                5 => lang.pick("Deneme sınavı", "Mock exam").into(),
                _ => lang.pick("Okuma / Çalışma", "Reading / Study").into(),
            }
        };
        TaskConfig {
            command,
            label: final_label,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlanEditModal {
    pub days: Vec<DayConfig>,
    pub day_idx: usize,
    pub task_idx: usize,
    pub prompt: Option<PlanEditPrompt>,
    pub error: Option<String>,
}

impl PlanEditModal {
    pub fn from_plan(existing: &[DayConfig], lang: Lang) -> Self {
        let days = if existing.is_empty() {
            vec![DayConfig {
                day: 1,
                title: lang.pick("Gün 1", "Day 1").to_string(),
                tasks: Vec::new(),
            }]
        } else {
            existing.to_vec()
        };
        Self {
            days,
            day_idx: 0,
            task_idx: 0,
            prompt: None,
            error: None,
        }
    }

    pub fn renumber_days(&mut self) {
        for (idx, day) in self.days.iter_mut().enumerate() {
            day.day = idx + 1;
        }
        self.day_idx = self.day_idx.min(self.days.len().saturating_sub(1));
        self.clamp_task();
    }

    pub fn clamp_task(&mut self) {
        let len = self.days.get(self.day_idx).map_or(0, |d| d.tasks.len());
        self.task_idx = self.task_idx.min(len.saturating_sub(1));
    }
}

#[derive(Clone, Debug)]
pub struct AiSettingsModal {
    pub provider_idx: usize,
    pub model: String,
    pub effort_idx: usize,
    pub base_url: String,
    pub gemini_key_input: String,
    pub gemini_key_dirty: bool,
    pub gemini_key_masked: Option<String>,
    pub openai_key_input: String,
    pub openai_key_dirty: bool,
    pub openai_key_masked: Option<String>,
    pub custom_cmd: String,
    pub timeout_secs: String,
    pub field: usize,
    pub statuses: Vec<ai::ProviderStatus>,
    pub env_overrides: Vec<&'static str>,
    pub test_result: Option<(bool, String)>,
    pub error: Option<String>,
    existing_cfg: AiConfig,
}

impl AiSettingsModal {
    pub fn load(cfg: AiConfig, lang: Lang) -> Self {
        let provider_idx = AiProvider::ALL
            .iter()
            .position(|&p| p == cfg.provider)
            .unwrap_or(0);
        let effort_idx = cfg
            .effort
            .and_then(|eff| AiEffort::ALL.iter().position(|&e| e == eff))
            .map_or(0, |i| i + 1);
        let statuses = ai::probe_providers(&cfg, lang);
        let gemini_key_masked = cfg.gemini_api_key.as_deref().map(config::mask_secret_tail);
        let openai_key_masked = cfg.openai_api_key.as_deref().map(config::mask_secret_tail);
        Self {
            provider_idx,
            model: cfg.model.clone().unwrap_or_default(),
            effort_idx,
            base_url: cfg.base_url.clone().unwrap_or_default(),
            gemini_key_input: String::new(),
            gemini_key_dirty: false,
            gemini_key_masked,
            openai_key_input: String::new(),
            openai_key_dirty: false,
            openai_key_masked,
            custom_cmd: cfg.custom_cmd.clone().unwrap_or_default(),
            timeout_secs: cfg.timeout_secs.to_string(),
            field: 0,
            statuses,
            env_overrides: config::env_overrides(),
            test_result: None,
            error: None,
            existing_cfg: cfg,
        }
    }

    pub fn provider(&self) -> AiProvider {
        AiProvider::ALL[self.provider_idx % AiProvider::ALL.len()]
    }

    pub fn cycle_provider(&mut self, delta: isize, lang: Lang) {
        let len = AiProvider::ALL.len() as isize;
        self.provider_idx = ((self.provider_idx as isize + delta).rem_euclid(len)) as usize;
        if let Ok(draft) = self.build_config() {
            self.statuses = ai::probe_providers(&draft, lang);
        }
        self.error = None;
    }

    pub fn effort(&self) -> Option<AiEffort> {
        if self.effort_idx == 0 {
            None
        } else {
            AiEffort::ALL.get(self.effort_idx - 1).copied()
        }
    }

    pub fn cycle_effort(&mut self, delta: isize) {
        let total = (AiEffort::ALL.len() + 1) as isize;
        self.effort_idx = ((self.effort_idx as isize + delta).rem_euclid(total)) as usize;
        self.error = None;
    }

    pub fn cycle_suggested_model(&mut self, delta: isize) {
        let presets = self.provider().suggested_models();
        if presets.is_empty() {
            return;
        }
        // Index 0 represents "(default)" (empty string), 1..=presets.len() are preset models.
        let total = (presets.len() + 1) as isize;
        let current = presets
            .iter()
            .position(|&m| m == self.model.trim())
            .map_or(0, |i| i + 1) as isize;
        let next = (current + delta).rem_euclid(total) as usize;
        self.model = if next == 0 {
            String::new()
        } else {
            presets[next - 1].to_string()
        };
        self.error = None;
    }

    pub fn build_config(&self) -> Result<AiConfig> {
        let model_trimmed = self.model.trim();
        let model = if model_trimmed.is_empty() {
            None
        } else {
            Some(config::validate_model_identifier(model_trimmed)?.to_string())
        };
        let base_trimmed = self.base_url.trim();
        let base_url = if base_trimmed.is_empty() {
            None
        } else {
            Some(config::validate_http_url(base_trimmed)?)
        };
        let custom_trimmed = self.custom_cmd.trim();
        let custom_cmd = if custom_trimmed.is_empty() {
            None
        } else {
            ai::parse_custom_command(custom_trimmed)?;
            Some(custom_trimmed.to_string())
        };
        let timeout_secs = self
            .timeout_secs
            .trim()
            .parse::<u64>()
            .map_err(|_| invalid("Timeout must be a non-negative integer."))?;
        let gemini_api_key = if self.gemini_key_dirty {
            let k = self.gemini_key_input.trim();
            (!k.is_empty()).then(|| k.to_string())
        } else {
            self.existing_cfg.gemini_api_key.clone()
        };
        let openai_api_key = if self.openai_key_dirty {
            let k = self.openai_key_input.trim();
            (!k.is_empty()).then(|| k.to_string())
        } else {
            self.existing_cfg.openai_api_key.clone()
        };
        Ok(AiConfig {
            provider: self.provider(),
            model,
            effort: self.effort(),
            base_url,
            gemini_api_key,
            openai_api_key,
            custom_cmd,
            timeout_secs,
        })
    }

    pub fn save_to_dotenv(&self, root: &std::path::Path) -> Result<()> {
        let cfg = self.build_config()?;
        let provider_val = cfg.provider.as_env_str();
        let effort_val = cfg.effort.map(AiEffort::as_env_str);
        let timeout_val = cfg.timeout_secs.to_string();
        let mut updates: Vec<(&str, Option<&str>)> = vec![
            ("STUDIA_AI_PROVIDER", Some(provider_val)),
            ("STUDIA_AI_MODEL", cfg.model.as_deref()),
            ("STUDIA_AI_EFFORT", effort_val),
            ("STUDIA_AI_BASE_URL", cfg.base_url.as_deref()),
            ("STUDIA_AI_CMD", cfg.custom_cmd.as_deref()),
            ("STUDIA_AI_TIMEOUT_SECS", Some(timeout_val.as_str())),
        ];
        if self.gemini_key_dirty {
            let val = cfg.gemini_api_key.as_deref();
            updates.push(("GEMINI_API_KEY", val));
            if val.is_none() {
                updates.push(("STUDIA_GEMINI_API_KEY", None));
            }
        }
        if self.openai_key_dirty {
            let val = cfg.openai_api_key.as_deref();
            updates.push(("OPENAI_API_KEY", val));
            if val.is_none() {
                updates.push(("STUDIA_OPENAI_API_KEY", None));
            }
        }
        config::save_dotenv_updates(root, &updates)?;
        Ok(())
    }
}
