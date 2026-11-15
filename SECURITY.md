# Security Policy

## Local-First Trust Model & Security Boundaries

`studia` runs locally on your machine and communicates only with the AI provider and Telegram endpoints you explicitly configure:

- **Secret Hygiene**: API keys and bot tokens (`GEMINI_API_KEY`, `OPENAI_API_KEY`, `STUDIA_TELEGRAM_TOKEN`, etc.) are read from OS environment variables or `.env` / `data/.env`. They are redacted from diagnostics/error paths and explicitly stripped (`env_remove`) before spawning external AI CLI subprocesses (`agy`, `claude`, `codex`, `gemini`, `custom`).
- **HTTP Isolation**: Outbound HTTP calls via `curl` pass credentials/configs through stdin (`--config -`) alongside `--disable -q -sS` to prevent secret exposure in command-line arguments and ignore untrusted user `~/.curlrc` overrides. Remote non-loopback `http://` URLs for `STUDIA_AI_BASE_URL` trigger explicit security warnings in `studia doctor`.
- **File Permissions & Atomic Writes**: On Unix/macOS, `data/` directories (`0700`) and SQLite/Anki/config files (`0600`) use restrictive permissions, and `studia doctor` warns when `.env` files are group- or world-readable. Database writes and Telegram export files use safe temporary paths and atomic renames.
- **Path & Input Validation**: Course names, workspace subdirectories, exam filenames, and JSON/exam parsers enforce identifier/path traversal checks so inputs cannot escape `kaynaklar/` or `anki/`.
- **Prompt Injection Fencing**: Indexed user study materials (`pdf`, `docx`, `md`, `txt`) and student follow-up text are delimited inside explicit untrusted-data boundaries before inclusion in AI prompts.
- **Single-Chat Telegram Authorization**: `studia listen` strictly verifies incoming chat IDs against `STUDIA_TELEGRAM_CHAT_ID` and silently drops updates from unauthorized chats.

## Reporting a Vulnerability

If you discover a security vulnerability, **please do not open a public GitHub issue with exploit details**.

Instead, please report it privately using **GitHub Security Advisories** ("Report a vulnerability" under the repository's **Security** tab at `https://github.com/zodicall/studia/security/advisories/new`).

Please include:
- A clear description of the vulnerability and affected component/file.
- Minimal reproduction steps or proof-of-concept inputs.
- Your assessment of potential impact.

We will acknowledge reports promptly and work on a fix and coordinated disclosure before releasing details publicly.
