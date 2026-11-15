[English](README.md) · [Türkçe](README.tr.md)

# studia

`studia` is a bilingual (English / Turkish), cross-platform AI study assistant built in Rust. It indexes your local study files (`pdf`, `docx`, `md`, `txt`), organizes course-specific study plans via `course.json`, runs interactive quizzes and spaced-repetition reviews (Leitner boxes 1–5), exports Anki flashcard decks, and works identically from a terminal **TUI** or a **Telegram bot**.

## Features

- **Terminal TUI & Telegram Bot**: Study locally in a keyboard-driven terminal UI (`ratatui`) or remotely via Telegram (`studia listen`), backed by the same SQLite database (`data/studia.db`).
- **Live Bilingual UI (`en` / `tr`)**: Auto-detects OS language (`cli > stored > env > locale > en`), switchable live via `F4` in TUI or `/lang tr|en` in Telegram/TUI.
- **Course-Agnostic Design**: Drop notes under `kaynaklar/<course>/` and optionally customize persona, mock exam question counts/categories/rules, and multi-day study plans in `course.json`.
- **Flexible AI Backends**: Works with Gemini HTTP API, OpenAI (or Ollama / any OpenAI-compatible endpoint via `STUDIA_AI_BASE_URL`), AI CLIs (`agy`, `claude`, `codex`, `gemini`), or custom command templates (`STUDIA_AI_PROVIDER=custom`).
- **Spaced Repetition & Anki**: Incorrect answers enter a 5-box Leitner schedule and generate fresh AI variation questions on review; flashcards export directly to tab-separated `.txt` decks under `anki/`.
- **Offline Diagnostics**: `studia doctor` verifies `kaynaklar/`, `curl`, `pdftotext`, AI configuration, `.env` file permissions, and SQLite integrity (`PRAGMA quick_check`).

## Quickstart

### Prerequisites

- **Rust 1.89+** (`cargo build --release --locked`) or a prebuilt release binary
- **`curl`** on `PATH` (for Telegram API & HTTP AI requests)
- **`pdftotext`** (Poppler, optional fallback — a built-in PDF text extractor is included; `.md`, `.txt`, `.docx`, and standard `.pdf` files work out of the box)

### Install & Run

```bash
git clone https://github.com/zodicall/studia.git
cd studia
cargo build --release --locked
```

1. Copy the environment template and configure an AI provider:

   ```bash
   cp .env.example .env
   chmod 600 .env   # recommended on Unix/macOS
   ```

2. Create a course folder and add your study notes (or copy a template from [`examples/`](examples/)):

   ```bash
   mkdir -p kaynaklar/Calculus
   # Add .md, .txt, .docx, or .pdf files inside kaynaklar/Calculus/
   cp examples/general-course/course.json kaynaklar/Calculus/course.json
   ```

3. Run offline diagnostics, index sources, and launch the TUI:

   ```bash
   cargo run --release -- doctor
   cargo run --release -- index
   cargo run --release
   ```

## AI Backend Configuration

`EnvLookup` resolves configuration with precedence: **OS environment > `<root>/.env` > `<root>/data/.env`**.

| Variable | Description | Default |
| :--- | :--- | :--- |
| `STUDIA_AI_PROVIDER` | `auto`, `gemini`, `openai`, `agy`, `claude`, `codex`, `gemini-cli`, `custom` | `auto` |
| `STUDIA_AI_MODEL` | Optional model override | `gemini-2.5-flash` / `gpt-4o-mini` |
| `STUDIA_AI_BASE_URL` | Custom API endpoint (e.g., `http://localhost:11434/v1` for Ollama) | Provider default |
| `GEMINI_API_KEY` | Google Gemini API key (also supports `STUDIA_GEMINI_API_KEY`) | — |
| `OPENAI_API_KEY` | OpenAI / compatible API key (also supports `STUDIA_OPENAI_API_KEY`, `STUDIA_AI_API_KEY`) | — |
| `STUDIA_AI_CMD` | Command template for `custom` provider (`{prompt_file}`, `{schema_file}`) | — |
| `STUDIA_AI_TIMEOUT_SECS` | Timeout in seconds (`0` disables timeout) | `180` |
| `STUDIA_LANG` | Startup UI/prompt language (`en` \| `tr`) | Auto-detected |

When `STUDIA_AI_PROVIDER=auto`, the order checked is: `GEMINI_API_KEY` → `OPENAI_API_KEY` → `agy` → `claude` → `codex` → `gemini` in `PATH`.

## Telegram Bot Setup

1. Create a bot via `@BotFather` and retrieve your bot token and numeric chat ID.
2. Add credentials to `.env`:

   ```env
   STUDIA_TELEGRAM_TOKEN=123456:your-bot-token
   STUDIA_TELEGRAM_CHAT_ID=123456789
   STUDIA_MORNING=08:30
   ```

3. Start the listener standalone or toggle it inside the TUI with `F2`:

   ```bash
   studia listen
   ```

   Only messages/callbacks matching `STUDIA_TELEGRAM_CHAT_ID` are processed; others are ignored silently.

## Customizing Courses (`course.json`)

Place an optional `course.json` inside `kaynaklar/<course>/` to tailor:

- `title` & `persona`: Display name and AI tutor instructions.
- `exam`: Question count (`1..=200`), choices count (`4` or `5`), weighted categories, and exam rules.
- `plan`: Sequential day-by-day checklist (`day`, `title`, `tasks` with optional slash `command` + `label`).

Ready-to-use templates are included in:
- [`examples/general-course/course.json`](examples/general-course/course.json)
- [`examples/academic-english/course.json`](examples/academic-english/course.json)

## CLI, TUI & Bot Commands

### CLI Usage

```text
studia [--root <dir>] [--lang tr|en] [tui|listen|index|doctor]
studia --help | -h
studia --version | -V
```

### TUI Shortcuts

| Key | Action |
| :--- | :--- |
| `1`–`6` / `Tab` / `Shift+Tab` | Switch tabs (`Ask`, `Quiz`, `Review`, `Stats`, `Today`, `Exams`) |
| `↑` / `↓` | Switch active course (or move between Today tasks / Exam history items) |
| `F1` / `?` | Open / close Help overlay |
| `F2` | Start / stop background Telegram listener |
| `F3` | Toggle AI Coach (auto-picks and launches next study step) |
| `F4` | Switch UI & AI prompt language live (`EN` ↔ `TR`) |
| `Enter` | Submit command, start study task, or open prompt |
| `Esc` | Cancel input / end active quiz or background AI job |

### Bilingual Bot & TUI Slash Commands

All commands accept both English and Turkish aliases:

| English | Turkish | Purpose |
| :--- | :--- | :--- |
| `/help` | `/yardim` | Show command summary & menu |
| `/courses` | `/dersler` | List courses with stats & inline selector |
| `/course <name>` | `/ders <ad>` | Switch active course |
| `/explain <topic>` *(also `/lesson`)* | `/anlat <konu>` | Guided lesson + 5-question quick check |
| `/quiz [topic] [n]` | `/quiz [konu] [n]` | Generate or replay saved questions |
| `/practice <topic> [n]` | `/kazanim <konu> [n]` | Focused quick-check practice |
| `/review [n] [exact]` | `/tekrar [n] [orijinal]` | Review due mistakes (AI variations or exact) |
| `/cards <topic> [n]` | `/kelime <konu> [n]` | Generate concept/vocabulary Anki deck |
| `/exam [n]` | `/deneme [n]` | Start full mock exam |
| `/exams [index]` | `/denemeler [sıra]` | View mock exam history & AI coaching breakdown |
| `/mistakes` | `/yanlislar` | Due review counts across courses |
| `/today` | `/bugun` | Today's study plan checklist |
| `/plan [start\|day\|goto]` | `/plan [baslat\|gun\|gec]` | Inspect or schedule study plan |
| `/lang <tr\|en>` | `/lang <tr\|en>` | Switch language persistently |
| `/test` | `/test` | Run a local offline sample question |
| `/cancel` | `/iptal` | Cancel running AI job or active quiz |

## Troubleshooting (`doctor`)

Run `studia doctor` to diagnose setup issues offline:
- **`[FAIL] curl not found`**: Install `curl` and ensure it is in `PATH`.
- **`[INFO] pdftotext: not installed`**: Optional fallback only; the built-in PDF text extractor is active. Install Poppler (`pdftotext`) if you need fallback extraction for complex PDFs.
- **`[WARN] Env file permissions are too broad`**: Restrict `.env` permissions (`chmod 600 .env`) on Unix/macOS.
- **`[WARN] non-loopback http://`**: Prefer `https://` for remote `STUDIA_AI_BASE_URL` endpoints.

## Disclaimer, Third-Party Notices & Privacy

- **AI Accuracy & No Outcome Guarantee**: AI-generated explanations, quizzes, Leitner review variations, and coaching feedback may contain inaccuracies or hallucinations and are provided for study assistance only, with no guarantee of academic, grading, or exam outcomes.
- **Non-Affiliation & Trademarks**: `studia` is an independent open-source project. Telegram, Google Gemini, OpenAI, Anthropic Claude, Anki, Ollama, and other referenced names/services are trademarks of their respective owners; users are solely responsible for complying with the Terms of Service of any external provider they configure.
- **Self-Hosted Privacy (KVKK / GDPR)**: `studia` runs locally on your machine, sends indexed note excerpts and prompts only to the AI and Telegram APIs you configure, and stores study history and runtime logs locally in `data/studia.db` and `data/bot.log`. Operators must avoid uploading personal, confidential, or copyrighted third-party materials without proper authorization.

## Security & License

See [SECURITY.md](SECURITY.md) for the security model, local-first data guarantees, and vulnerability reporting, [LICENSE](LICENSE) for the MIT License, and [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt) for third-party crate license and copyright notices.
