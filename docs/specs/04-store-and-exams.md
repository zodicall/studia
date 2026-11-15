# SPEC-04: SQLite Storage Migrations & Generic Mock Exam Banks

- **Scope:** `src/store.rs`, `src/exam.rs`
- **Status:** Draft -> Implementation Contract

---

## 1. Storage Schema Migrations & Concurrency (`STORE-*`)

| ID | Requirement |
| :--- | :--- |
| `STORE-1` | `Store::open` configures `busy_timeout(Duration::from_secs(5))`, `foreign_keys = ON`, and for file-backed DBs calls `conn.pragma_update(None, "journal_mode", "WAL")` (not `conn.execute`, which fails on returned rows). Idempotently checks `PRAGMA table_info(...)` and migrates:<br>- `sessions`: adds `kind TEXT NOT NULL DEFAULT 'quiz'`, backfilling `'exam'` where `topic LIKE '%Deneme%'`.<br>- `questions`: adds `is_exam INTEGER NOT NULL DEFAULT 0`, backfilling `1` where `topic LIKE '%Deneme%'`. |
| `STORE-2` | Queries never depend on localized `"Deneme"` strings for new data:<br>- `due_original_reviews` & `next_original_review` filter `questions.is_exam = 0`.<br>- `deneme_sessions(&self, subject: &str)` scopes strictly by `WHERE subject = ?1 AND kind = 'exam'`.<br>- `weakest_topics` & `topic_totals` filter `sessions.kind != 'exam'`. |
| `STORE-3` | - Cache state keys include language: `lesson:{subject}:{lang}:{topic}` and `answer_cache:{subject}:{lang}:{question}` so toggling `Lang` never serves cached prose in the wrong language.<br>- `record_answer_with_review_at` validates `choice` against the stored question's actual `choices` length (`Vec<String>`) instead of hardcoded `choice > 4`, and computes Leitner `next_box` cleanly via `match current_box { Some(b) if correct => b + 1, _ => 1 }` (zero `.unwrap()`).<br>- Add `Store::quick_check(&self) -> Result<bool>` executing `PRAGMA quick_check` for `studia doctor`. |

### Acceptance Criteria (`STORE-*`)
- **Given** a legacy SQLite DB missing `sessions.kind` and `questions.is_exam` with `"Sample Deneme 1"` rows -> **Then** `Store::open` upgrades schema idempotently and preserves `"Sample Deneme 1"` as `'exam'` / `is_exam = 1`.
- **Given** an English `"Mock Exam 1"` session saved with `kind = "exam"` and questions saved with `is_exam = 1` -> **Then** `deneme_sessions(subject)` returns it AND `due_original_reviews` excludes its questions.
- **Given** two subjects `"math"` and `"english"` each with exam sessions -> **Then** `deneme_sessions("math")` returns only `"math"` exams.
- **Given** lesson saved under `Lang::Tr`, then language toggled to `Lang::En` -> **Then** `load_lesson(..., Lang::En, ...)` returns `None`.

---

## 2. Dynamic Mock Exam Discovery & Chunking (`EXAM-*`)

| ID | Requirement |
| :--- | :--- |
| `EXAM-1` | `discover_banks(root, subject)` dynamically discovers valid `Bank` JSON files under `<root>/data/denemeler/<subject>/*.json` and `<root>/kaynaklar/<subject>/exams/*.json`, sorted deterministically by slug/filename without hardcoded exam lists. |
| `EXAM-2` | Validated slug (`COURSE-4`) prevents traversal (`../`); completion state tracks per-subject/bank (`deneme:used:{subject}:{slug}`). |
| `EXAM-3` | `exam::import_with` supports arbitrary question counts (`1..=answers.len()`) by chunking dynamically in batches of `40` (`(1..=answers.len())` windows of `40`, fixing `< 40`-question slice panic), accepts configured `choices_count` (`4..=5`), and verifies `answer_index < choices.len()` before indexing `choices[answer_index]`. Exam breakdown groups by `CourseConfig.exam.categories` if present or dynamically by question `topic`, formatting timestamps via `chrono::Local`. |

### Acceptance Criteria (`EXAM-*`)
- **Given** `<root>/kaynaklar/Biology/exams/midterm.json` -> **Then** `discover_banks` finds `"midterm"` for `"Biology"` without touching any Rust source list.
- **Given** `exam::import_with` called with a 15-question answer key -> **Then** it processes `1..=15` in a single chunk without panicking.
- **Given** `parse_items` receives a 4-choice question with answer index `4` (`E`) -> **Then** it returns an actionable validation `Err` instead of panicking on out-of-bounds access.
