# SPEC-07: Cross-Cutting Security & Runtime Stability Verification Matrix

- **Scope:** Cross-cutting invariants and adversarial verification contracts referencing `SPEC-01` .. `SPEC-06`
- **Status:** Draft -> Implementation Contract

This document avoids repeating module APIs from `01..06`; instead, it establishes the **threat boundary, failure recovery rules, zero-panic invariants, and adversarial test contracts** that every release candidate must pass.

---

## 1. Security Boundary Contracts (`SEC-*`)

| ID | Cross-Cutting Invariant | Enforced In | Adversarial Test Contract (`SPEC-SEC-*`) |
| :--- | :--- | :--- | :--- |
| `SEC-1` | **Secret Isolation:** Tokens/API keys (`STUDIA_TELEGRAM_TOKEN`, `GEMINI_API_KEY`, `OPENAI_API_KEY`, `STUDIA_*_API_KEY`) never enter CLI `argv`, external AI CLI child envs, `data/bot.log`, SQLite tables, Telegram messages, TUI status, `Err(...)` text, or `doctor` output. CLI temp dirs run inside `std::env::temp_dir()`. | `01` (`DOC-1`), `03` (`AI-2..3`, `TX-1`), `05` (`BOT-4`) | `SPEC_SEC_1_cli_env_and_error_scrub`: spawn mock CLI with secret vars set in parent env; verify child cannot see any secret env key, temp dir sits outside `<root>/data`, and simulated curl/CLI failures strip secret strings. |
| `SEC-2` | **Injection Defense:** Only `http://` / `https://` URLs permitted in HTTP transport; `\0`, `\r`, `\n` rejected in curl config values; `NamedTempFile` paths normalized; `;` / `,` rejected in `sendDocument`; `STUDIA_AI_CMD` tokenized without shell execution & rejects shell metacharacters; never interpolate raw prompts into `argv`. | `03` (`AI-3`, `TX-1`) | `SPEC_SEC_2_curl_injection_and_cmd_split`: assert `file://`, CRLF header injection, `;type=` multipart injection, and shell pipe commands (`\|`, `;`, `&&`) all fail cleanly before process spawn. |
| `SEC-3` | **Path Containment & Parser DoS Ceilings:** `validate_slug` / `validate_subject_name` reject `..`, slashes, leading `-`, NUL; canonical containment checked; `MAX_BYTES` (`25 MiB`) checked *before* `pdftotext`; DOCX zip decompressed size capped at `25 MiB`; symlink recursion depth <= `16` with cycle tracking; incremental UTF-8 counters; curl response <= `16 MiB`. | `02` (`COURSE-4`, `SRC-1..2`), `04` (`EXAM-2`) | `SPEC_SEC_3_traversal_symlink_loop_and_oversize`: assert `"../etc"`, deep circular symlink, 26 MiB file before `pdftotext`, and oversized DOCX fail or terminate cleanly within bounds without OOM/stack overflow. |
| `SEC-4` | **Authorization, Channel Safety & Supply Chain:** Private `chat_id` verification on all Telegram `message` & `callback_query` updates; `callback_data <= 64` bytes; HTML escape + plain-text fallback; CI `--locked`, `persist-credentials: false`, minimal `permissions`, `cargo audit`, static personal-trace audit. | `05` (`BOT-2..4`), `06` (`REL-4`) | `SPEC_SEC_4_telegram_auth_html_and_callback_bounds`: assert spoofed chat/group/sender IDs drop silently, malformed HTML degrades to plain text cleanly, and 120-byte UTF-8 subject generates `<= 64`-byte callback buttons. |

---

## 2. Stability & Recovery Contracts (`STAB-*`)

| ID | Cross-Cutting Invariant | Enforced In | Adversarial Test Contract (`SPEC-STAB-*`) |
| :--- | :--- | :--- | :--- |
| `STAB-1` | **Panic-Free Production Code:** Zero `.unwrap()` / unchecked `[i]` / unchecked byte-slice on runtime data in production paths of `bot.rs`, `tui/app.rs`, `store.rs`, `sources.rs`, `exam.rs`. UTF-8 truncation uses `.chars()` or `.floor_char_boundary()`. | `02`, `04`, `05` | `SPEC_STAB_1_edge_inputs_never_panic`: empty categories, `< 40`-question exam keys, out-of-range answer indices on 4-choice banks, broken UTF-8 sequences, and empty session/pending transitions return `Result` cleanly with zero panics. |
| `STAB-2` | **Bounded Subprocesses, Backoff & Log Hygiene:** Concurrent bounded `stdout` reader thread avoids pipe deadlock; `stdin` tolerates `BrokenPipe`; timeout (`STUDIA_AI_TIMEOUT_SECS`) kills child tree (`taskkill /F /T /PID` on Windows); listener retries back off (`2s..60s`); `data/bot.log` rotates at `2 MiB`; zero `eprintln!` in TUI. | `02` (`SRC-3`), `03` (`TX-2`), `05` (`BOT-4`, `TUI-4`) | `SPEC_STAB_2_timeout_backoff_and_log_rotation`: hung >64 KB stdout mock process exits via timeout without pipe deadlock; backoff doubles cleanly (`2..60s`) and resets; writing > `2 MiB` rotates `data/bot.log` to `data/bot.log.1`. |
| `STAB-3` | **Lock & Storage Integrity:** `File::try_lock()` + `TryLockError::WouldBlock` consistent across Windows/Linux/macOS; SQLite `WAL` (`pragma_update`), `busy_timeout(5s)`, atomic multi-write transactions, idempotent migrations (`sessions.kind`, `questions.is_exam`), `PRAGMA quick_check` in `doctor`. | `01` (`LOCK-1`, `DOC-1`), `04` (`STORE-1..3`) | `SPEC_STAB_3_lock_migration_and_quick_check`: lock contention & release works across handles; legacy DB auto-migrates cleanly without data loss; `Store::quick_check` passes on live DB. |
