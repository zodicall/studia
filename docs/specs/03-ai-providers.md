# SPEC-03: Multi-Provider AI Engine & Hardened Process/HTTP Transport

- **Scope:** `src/ai.rs`, `src/telegram.rs` (`http_post_json`, `command_output`, `telegram_request`)
- **Status:** Draft -> Implementation Contract

---

## 1. AI Providers & Auto-Detection (`AI-1..2`)

| ID | Requirement |
| :--- | :--- |
| `AI-1` | `AiProvider` supports: `Auto`, `GeminiApi`, `OpenAiApi`, `Agy`, `Claude`, `Codex`, `Custom`. Under `Auto`, resolution order is: `1. GEMINI_API_KEY / STUDIA_GEMINI_API_KEY` -> `2. OPENAI_API_KEY / STUDIA_OPENAI_API_KEY` -> `3. PATH` probe (`agy`, `claude`, `codex`) -> actionable bilingual error. |
| `AI-2` | HTTP providers use `telegram::http_post_json` (curl `-K -`):<br>- **Gemini API:** `POST {base}/v1beta/models/{model}:generateContent` (default base `https://generativelanguage.googleapis.com`, default model `gemini-2.5-flash`) with header `x-goog-api-key` (never in URL query string), `generationConfig.responseMimeType = "application/json"`, `responseJsonSchema`, and optional `generationConfig.thinkingConfig` (`thinkingBudget` for `gemini-2.5-*`, `thinkingLevel` normalized per-model for Gemini 3.x/4.x).<br>- **OpenAI-compatible API:** `POST {base}/chat/completions` (default base `https://api.openai.com/v1`, default model `gpt-4o-mini`) with `Authorization: Bearer ...`, optional model-normalized `reasoning_effort` (omitted on non-reasoning `gpt-4o*`), and `response_format: { type: "json_schema", json_schema: { name: "response", strict: true, schema } }`, automatically falling back once to `{ type: "json_object" }` if an endpoint returns HTTP 400 rejecting `json_schema` (or omitting `reasoning_effort` if unsupported). |

---

## 2. CLI Backends & Subprocess Execution (`AI-3`)

| ID | Requirement |
| :--- | :--- |
| `AI-3` | - **Windows PATH resolution:** On Windows, search `PATHEXT` (`.exe`, `.cmd`, `.bat`) **before** extensionless npm POSIX shims.<br>- **Isolated temp directory:** CLI runners create `tempfile::tempdir()` under `std::env::temp_dir()` (NOT `<root>/data`) so child agents stay isolated from `<root>/.env` and `studia.db`.<br>- **`custom` (`STUDIA_AI_CMD`):** Tokenized with a quote-aware splitter; rejects unescaped shell pipeline operators (`\|`, `&`, `;`, `` ` ``, `$()`, `>`, `<`); executes directly with `{prompt_file}` / `{schema_file}` token replacement and never interpolates raw prompt text onto `argv`.<br>- **Supported CLIs:** `agy` (`--output-format json --disable-slash-commands --sandbox --json-schema <schema>` + optional `--model` / `--effort`), `claude` (`-p --output-format json --json-schema <schema>` + optional `--effort` via stdin), `codex` (`exec --sandbox read-only --output-schema <schema_file> -o <out_file>` + optional `-c model_reasoning_effort=...` + `-` via stdin), all scrubbing ALL Telegram + AI key env vars (`STUDIA_TELEGRAM_TOKEN`, `STUDIA_TELEGRAM_CHAT_ID`, `GEMINI_API_KEY`, `OPENAI_API_KEY`, `STUDIA_GEMINI_API_KEY`, `STUDIA_OPENAI_API_KEY`, `STUDIA_AI_API_KEY`). |

---

## 3. Structured Outputs, Parsing & Bilingual Prompts (`AI-4..5`)

| ID | Requirement |
| :--- | :--- |
| `AI-4` | Schemas (`ANSWER_SCHEMA`, `lesson_schema`, `quiz_schema(choices_count)`, `vocab_schema`) accept `choices_count` `4..=5`. Parsers (`parse_answer`, `parse_lesson`, `parse_quiz`, `parse_vocab`) strip optional Markdown code fences (` ```json ... ``` `) before JSON decoding. `named_choice` reconciles both `"Doğru cevap: '...'"` and `"Correct answer: '...'"` case-insensitively. `anki_tsv` uses the active `subject` tag + `studia` tag. |
| `AI-5` | `prompt(...)`, `lesson_request(...)`, `quiz_request(...)`, `review_request(...)`, and `vocab_request(...)` parameterize over `Lang` (`Tr \| En`), `subject`, and `&CourseConfig` (`persona`, `exam.rules`, `choices_count`). |

---

## 4. Transport & Subprocess Stability (`TX-1..2`)

| ID | Requirement |
| :--- | :--- |
| `TX-1` | **Hardened `curl -K -` (`http_post_json` & `telegram_call`):** `#[cfg(windows)] "curl.exe"` / `#[cfg(not(windows))] "curl"`; enforce `http://` or `https://` URL scheme (reject `file://`, `gopher://`, etc.); reject control chars (`\0`, `\r`, `\n`) in URLs, headers, tokens, and model names; reject `;` and `,` in `sendDocument` form paths; normalize backslashes (`\ -> /`) or `quote()` `NamedTempFile` paths in `-K -` `data = "@..."`; enforce `connect-timeout = 10`, `max-time`, and `max-filesize` (`16 MiB`), with `--fail-with-body` (or HTTP status inspection via stderr/header) and strict redaction of tokens/keys from any error message. |
| `TX-2` | **Bounded Deadlock-Free `command_output`:** Drain `stdout` concurrently in a dedicated reader thread capped at `16 MiB`; ignore `ErrorKind::BrokenPipe` in the `stdin` writer thread; enforce configurable deadline (`STUDIA_AI_TIMEOUT_SECS`, default `180s`) via `try_wait()` loop; on timeout kill child tree (`taskkill /F /T /PID` on Windows, `child.kill()` + `wait()` on Unix) *before* joining reader thread so `.cmd` grandchild wrappers cannot hang pipes. |

### Acceptance Criteria (`AI-*`, `TX-*`)
- **Given** Gemini/OpenAI response wrapped in ` ```json\n...\n``` ` -> **Then** `parse_quiz` and `parse_lesson` parse cleanly.
- **Given** English explanation starting `"Correct answer: 'however'."` -> **Then** `reconcile_answers` sets `answer_index` to `'however'`.
- **Given** HTTP URL `"file:///etc/passwd"` or header value containing `"\r\nmax-time = 1"` -> **Then** `http_post_json` rejects with `Err` before spawning `curl`.
- **Given** a mock CLI outputting 100 KB and hanging before exit under a 1s timeout -> **Then** `command_output` aborts within timeout, kills the child cleanly without pipe-full deadlock, and returns `Err`.
