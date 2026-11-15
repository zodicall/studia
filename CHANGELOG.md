# Changelog

All notable changes to this project will be documented in this file.

## [0.1.0] - 2026-11-15

Initial public open-source release of `studia`.

### Added
- **Bilingual UI & Prompts (`en` / `tr`)**: System language detection (`--lang > stored > STUDIA_LANG > OS locale > en`), live TUI toggle via `F4`, and persistent `/lang <tr|en>` Telegram & TUI command.
- **Generic Course Configuration (`course.json`)**: Per-course titles, AI tutor personas, configurable mock exams (`question_count`, `choices_count` 4–5, category distribution, custom rules), and validated multi-day study plans (`examples/general-course`, `examples/academic-english`).
- **Multi-Provider AI Support**: Gemini API, OpenAI API, custom OpenAI-compatible/Ollama endpoints (`STUDIA_AI_BASE_URL`), AI CLIs (`agy`, `claude`, `codex`, `gemini`), custom CLI templates (`STUDIA_AI_CMD`), and configurable timeouts (`STUDIA_AI_TIMEOUT_SECS`).
- **Terminal TUI & Telegram Bot**: Interactive quizzes, guided lessons + chat follow-ups, Leitner 5-box spaced repetition with AI variation generation, Anki `.txt` deck export, and AI exam coaching.
- **Offline Diagnostics**: `studia doctor` command checking workspace layout, `curl`, `pdftotext`, AI configuration, `.env` file permissions, and SQLite integrity (`PRAGMA quick_check`).
- **Security & Stability Hardening**: Path traversal guards, untrusted document prompt fencing, subprocess sensitive env sanitization, curl isolated config flags (`--disable -q -sS`), restrictive Unix permissions (`0600`/`0700`) on secret/data files, SQLite schema/integrity validation, and exponential backoff in Telegram polling.
- **CI/CD Workflows**: Cross-platform GitHub Actions CI (`ubuntu-latest`, `windows-latest`, `macos-latest`) with `cargo audit` and tag-triggered multi-target release packaging (`macos-15-intel` for `x86_64-apple-darwin`) with SHA-256 checksums.
