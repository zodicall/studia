# SPEC-02: Course Config, Dynamic Study Plans, Coach, and Source Extraction

- **Scope:** `src/course.rs`, `src/plan.rs`, `src/coach.rs`, `src/sources.rs`, `examples/`
- **Status:** Draft -> Implementation Contract

---

## 1. Generic Course Abstraction (`COURSE-*`)

| ID | Requirement |
| :--- | :--- |
| `COURSE-1` | Zero hardcoded domain-specific course names, exam brands, publishers, websites, or teacher personas in defaults or heuristics. |
| `COURSE-2` | Zero-config onboarding: any valid directory `<root>/resources/<subject>/` loads cleanly with `CourseConfig::default()` (`title = None`, `persona = None`, `exam = None`, `plan = []`). |
| `COURSE-3` | Optional `<root>/resources/<subject>/course.json` schema (`#[serde(deny_unknown_fields)]` optional or clean typed fields): `title: Option<String>`, `persona: Option<String>`, `exam: Option<ExamConfig>`, `plan: Vec<DayConfig>`. `ExamConfig` validates `question_count` (`1..=200`, default `20`), `choices_count` (`4..=5`, default `5`), `categories: Vec<CategoryWeight { name, count }>`, and `rules: Option<String>`. `DayConfig` holds `day: u16`, `title: String`, `tasks: Vec<TaskConfig { command, label }>`. Invalid JSON surfaces a clear error identifying the file path rather than silently ignoring user typos. |
| `COURSE-4` | Safe identifier containment: subject directory names and slug identifiers reject `""`, `"."`, `".."`, `/`, `\`, `\0`, and leading `-`. Canonicalized resource targets stay strictly inside their base directory. |

### Acceptance Criteria (`COURSE-*`)
- **Given** an empty directory `resources/Rust101/notes.md` with no `course.json` -> **Then** `CourseConfig::load_or_default` returns default config cleanly and indexing discovers `"Rust101"`.
- **Given** `examples/general-course/course.json` and `examples/academic-english/course.json` -> **Then** both deserialize cleanly and pass `CourseConfig::validate()`.
- **Given** `choices_count: 7` or `day: 0` or subject `"../secret"` -> **Then** validation rejects cleanly with `Err`.

---

## 2. Subject-Scoped Study Plans (`PLAN-*`)

| ID | Requirement |
| :--- | :--- |
| `PLAN-1` | `plan.rs` contains no static built-in study schedules; all functions operate dynamically on `days: &[DayConfig]`. Empty `days` cleanly represents a flexible course without a fixed day schedule (`total_days() == 0`). |
| `PLAN-2` | All SQLite state keys are strictly namespaced by subject: `plan_start:{subject}` and `plan_done:{subject}:{date}:{day}:{task}`. No subject falls back to bare `"plan_start"` or `"plan_done:*"`. |
| `PLAN-3` | Telegram plan callback parsing (`p:<day>:<task>` and `pd:<day>`) accepts valid 1-based numbers and dispatchers validate `1 <= day <= days.len()` and `1 <= task <= tasks.len()`. |

### Acceptance Criteria (`PLAN-*`)
- **Given** two subjects `"math"` and `"history"` both starting a plan -> **Then** marking day 1 task 1 done on `"math"` leaves `"history"`'s status untouched.
- **Given** a 3-day custom `course.json` plan -> **Then** day progression tracks `1..=3` without assuming 7 or 30 days.

---

## 3. Subject-Agnostic Autonomous Coach (`COACH-*`)

| ID | Requirement |
| :--- | :--- |
| `COACH-1` | `coach.rs` accepts `(store, subject, &course, &snapshot, lang)` without hardcoding exam-specific prompts or fixed schedules. When `course.plan.is_empty()`, rule selection returns `Program::NoPlan` / `None` cleanly or suggests weak-topic quiz/review based on stats & due reviews without fabricating non-existent day tasks. |
| `COACH-2` | `coach::RUNNABLE` aligns with TUI Today execution (`TUI-3`): includes `"/cards"` (`"/kelime"`), `"/lesson"`, `"/quiz"`, `"/review"`, `"/exam"` when supported in TUI. |

---

## 4. Source Indexing & Extraction Bounds (`SRC-*`)

| ID | Requirement |
| :--- | :--- |
| `SRC-1` | Supports `.txt`, `.md`, `.pdf`, `.docx` inside `resources/<subject>/`. Skips `course.json`, `exams/`, `_duplicates`, `99_ARCHIVE`, and hidden (`.`-prefixed) directories. Tolerates broken symlinks (`read_dir`/metadata errors skip offending entry instead of aborting the subject). Prevents circular symlink infinite recursion via `max_depth = 16` and visited canonical directory set. |
| `SRC-2` | Resource bounds: checks `metadata.len() > MAX_BYTES` (`25 MiB`) **before** copying a PDF to a `tempfile::Builder` file or calling `pdftotext`. DOCX zip extraction enforces `MAX_BYTES` on entry size and decompressed XML stream. PDF/DOCX extraction loops track accumulated character counts incrementally ($O(N)$, no per-iteration `output.chars().count()`). `pdftotext` fallback path (`C:\Program Files\Git\mingw64\bin\pdftotext.exe`) is guarded with `#[cfg(windows)]` and runs with bounded stdout + subprocess timeout. |
| `SRC-3` | Zero `eprintln!` calls inside `sources::index` (prevents corrupting Ratatui's alternate screen when indexed from TUI background threads); non-fatal extraction warnings append via `telegram::log_file(root, ...)`. Zero hardcoded exam marker strings, keyword expansion maps, or watermark cleanup lists. |

### Acceptance Criteria (`SRC-*`)
- **Given** a 26 MiB file named `türkçe.pdf` -> **Then** `MAX_BYTES` check rejects it *before* creating a temp PDF copy or invoking `pdftotext`.
- **Given** a circular directory symlink `a/b -> ../` inside `resources/Demo/` -> **Then** `sources::index` completes safely without stack overflow.
- **Given** a broken symlink inside `resources/` -> **Then** `sources::subjects()` and `sources::index()` continue discovering valid subjects/files.
