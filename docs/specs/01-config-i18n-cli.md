# SPEC-01: Configuration, Bilingual i18n, CLI, and Instance Locking

- **Scope:** `src/main.rs`, `src/config.rs`, `src/i18n.rs`, `src/telegram.rs` (`open_listener_lock`), `.env.example`
- **Status:** Draft -> Implementation Contract

---

## 1. CLI Contract (`CLI-*`)

| ID | Requirement |
| :--- | :--- |
| `CLI-1` | Binary accepts optional global flags before or after the subcommand: `--root <dir>` (default `.`), `--lang <tr\|en>`, `--help \| -h`, `--version \| -V`. |
| `CLI-2` | Supported subcommands: `tui` (default when omitted), `listen`, `index`, and `doctor`. |
| `CLI-3` | Unknown flags, missing flag values, invalid `--lang` values, or unknown subcommands exit with non-zero status and an actionable bilingual usage error. |

### Acceptance Criteria (`CLI-*`)
- **Given** arguments `["studia", "--root", "/tmp/study", "--lang", "en", "doctor"]` -> **Then** parser returns `root=/tmp/study`, `lang=Some(Lang::En)`, `action="doctor"`.
- **Given** arguments `["studia", "--lang", "fr"]` -> **Then** parser returns `Err`.
- **Given** arguments `["studia", "--help"]` or `["studia", "--version"]` -> **Then** process prints concise usage/version text cleanly and exits `0`.

---

## 2. Configuration & `.env` Loading (`CFG-*`)

| ID | Requirement |
| :--- | :--- |
| `CFG-1` | Environment lookup precedence is strictly: `1. Process env (std::env::var)` -> `2. <root>/.env` -> `3. <root>/data/.env`. Never read an unrelated CWD `.env` when `--root` points elsewhere. |
| `CFG-2` | Unrecognized/legacy env prefixes (e.g., `LEGACY_STUDY_TELEGRAM_TOKEN`, `LEGACY_TELEGRAM_CHAT_ID`) are ignored cleanly. |
| `CFG-3` | `.env` parser handles blank lines, full-line `#` comments, optional leading `export `, trimmed keys, balanced single (`'...'`) or double (`"..."`) quotes, unquoted inline `#` comments, and ignores entries containing NUL (`\0`) bytes. |
| `CFG-4` | Typed config models: `AiProvider` (`Auto`, `GeminiApi`, `OpenAiApi`, `Agy`, `Claude`, `Codex`, `GeminiCli`, `Custom`), `AiConfig`, `TelegramConfig`, and `STUDIA_AI_TIMEOUT_SECS` (default `180`, `0` = no limit). |

### Key Env Variables
- `STUDIA_LANG`: `en` | `tr`
- `STUDIA_AI_PROVIDER`: `auto` | `gemini` | `openai` | `agy` | `claude` | `codex` | `gemini-cli` | `custom`
- `STUDIA_AI_MODEL`, `STUDIA_AI_BASE_URL`, `GEMINI_API_KEY` / `STUDIA_GEMINI_API_KEY`, `OPENAI_API_KEY` / `STUDIA_OPENAI_API_KEY`, `STUDIA_AI_CMD`, `STUDIA_AI_TIMEOUT_SECS`
- `STUDIA_TELEGRAM_TOKEN`, `STUDIA_TELEGRAM_CHAT_ID`, `STUDIA_MORNING` (`HH:MM`, default `08:30`)

### Acceptance Criteria (`CFG-*`)
- **Given** `STUDIA_LANG=en` in `<root>/.env` and `STUDIA_LANG=tr` in `<root>/data/.env` -> **Then** `<root>/.env` wins.
- **Given** `STUDIA_LANG=tr` in process env -> **Then** process env overrides `<root>/.env`.
- **Given** `<root>` is tempdir without `.env`, while CWD contains a `.env` -> **Then** CWD `.env` is NOT loaded.
- **Given** `export OPENAI_API_KEY="sk-test" # comment` in `.env` -> **Then** value resolves cleanly to `"sk-test"`.

---

## 3. Bilingual Localization (`I18N-*`)

| ID | Requirement |
| :--- | :--- |
| `I18N-1` | Runtime `Lang` (`Lang::Tr \| Lang::En`) resolves in order: `--lang` CLI flag -> persisted SQLite `state("lang")` -> `STUDIA_LANG` -> OS locale (`LC_ALL`/`LANG` starting with `tr` -> `Tr`, else `En`). |
| `I18N-2` | All CLI usage/errors, `doctor` diagnostics, TUI chrome/tabs/help/onboarding/errors, Telegram menus/commands/prompts, and AI prompt templates provide first-class Turkish and English strings compiled into the binary (zero runtime asset files). |
| `I18N-3` | Switching language (`F4` in TUI or `/lang` in Telegram) immediately persists `"tr"` or `"en"` to SQLite `state("lang")` and refreshes active UI/menus. |

---

## 4. Cross-Platform Listener Lock (`LOCK-1`)

| ID | Requirement |
| :--- | :--- |
| `LOCK-1` | `open_listener_lock(path)` opens `path` with `read(true).write(true).create(true).truncate(false)` and calls `std::fs::File::try_lock()` (enabled by Cargo `rust-version = "1.89"`). The returned `File` MUST remain alive for the entire listener process lifetime. `listener_running(path)` opens the file with matching options and checks `matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))` (replacing Windows-specific `raw_os_error() == Some(32)`). |

### Acceptance Criteria (`LOCK-1`)
- **Given** test holds `let _guard = open_listener_lock(&lock_path).unwrap()` -> **Then** second `open_listener_lock(&lock_path)` fails AND `listener_running(&lock_path)` returns `true`.
- **Given** `_guard` is dropped -> **Then** `listener_running(&lock_path)` returns `false` and `open_listener_lock(&lock_path)` succeeds again.

---

## 5. Offline Diagnostics (`DOC-1`)

| ID | Requirement |
| :--- | :--- |
| `DOC-1` | `studia [--root <dir>] doctor` performs zero network calls and prints PASS/WARN/FAIL status for: resolved `--root`, `<root>/data` writability, `<root>/kaynaklar` course count, `curl` presence, optional `pdftotext` presence, detected AI provider/binary/API-key readiness (never printing keys), Telegram credential readiness (never printing token/chat ID), SQLite `<root>/data/studia.db` `PRAGMA quick_check` (if DB exists), insecure non-loopback `http://` AI base URL warning, and (on Unix) world/group-readable `.env` permission warning (`mode & 0o077 != 0`). |
