# SPEC-05: Bilingual Telegram Bot & Subject-Aware TUI

- **Scope:** `src/bot.rs`, `src/tui.rs`, `src/tui/app.rs`, `src/tui/ui.rs`
- **Status:** Draft -> Implementation Contract

---

## 1. Bilingual Telegram Bot Contracts (`BOT-*`)

| ID | Requirement |
| :--- | :--- |
| `BOT-1` | Command dispatcher accepts both English and Turkish aliases regardless of active language: `/help\|/yardim`, `/today\|/bugun`, `/plan`, `/lesson\|/explain\|/anlat`, `/quiz`, `/practice\|/kazanim`, `/review\|/tekrar`, `/cards\|/kelime`, `/exam\|/deneme`, `/exams\|/denemeler`, `/mistakes\|/yanlislar`, `/courses\|/dersler`, `/course\|/ders`, `/cancel\|/iptal`, `/lang`, `/test`. Reply keyboard `MENU` and BotFather `setMyCommands` reflect active `Lang`. |
| `BOT-2` | `/lang [tr\|en]` switches & persists `state("lang")` cleanly, responds in the newly chosen language, and refreshes `setMyCommands` + reply keyboard. Subject inline buttons use compact indexed callbacks `d:<index>` (guaranteeing Telegram's **64-byte `callback_data` ceiling** regardless of UTF-8 subject length). |
| `BOT-3` | `Session` explicitly tracks `is_exam: bool` (never inferred from `"Deneme"` substring). Mock exam report header uses the session/course title instead of hardcoded exam labels. `/test` serves a clean bilingual sample question (`4` or `5` choices matching config/default). `edit_with_markup` tolerates `"message is not modified"` before attempting plain-text fallback when HTML parse errors occur. |
| `BOT-4` | Polling & security invariants:<br>- `telegram::accepted(update, &chat_id)` enforced on every incoming `message` and `callback_query`.<br>- AI scratch workdirs live in `std::env::temp_dir()` (`AI-3`).<br>- Listener transient error loop uses capped exponential backoff (`2s -> 4s -> 8s -> 16s -> 32s -> 60s`, reset to `2s` on success) and rotates `data/bot.log` at `2 MiB` (to `data/bot.log.1`).<br>- Zero production `.unwrap()` calls inside message/callback/session/pending handlers. |

### Acceptance Criteria (`BOT-*`)
- **Given** Telegram bot in English mode receives `/yardim` or `/help` -> **Then** both invoke the help handler and reply in English.
- **Given** bot receives `/lang tr` -> **Then** subsequent replies, `MENU`, and `setMyCommands` switch to Turkish and `state("lang") == "tr"`.
- **Given** a subject named `"Çok_Uzun_Türkçe_Ders_Klasörü_2026_İleri_Seviye_Mühendislik_Notları"` -> **Then** inline keyboard `callback_data` stays `<= 64` bytes.
- **Given** consecutive poll failures in listener -> **Then** sleep backoff progresses cleanly up to `60s` and resets immediately upon a successful `getUpdates`.

---

## 2. Subject-Aware Bilingual TUI Contracts (`TUI-*`)

| ID | Requirement |
| :--- | :--- |
| `TUI-1` | All tabs (`Today`, `Plan`, `Stats`, `Exams`) load and use the active subject's `CourseConfig`, calling subject-scoped APIs (`plan::status_for_subject`, `plan::day`, `bot::plan_text_for_subject`, `bot::deneme_count_for_subject`, `coach::snapshot`, `exam::next_unused_for_subject`, `store.deneme_sessions(&subject)`). `ui::draw_plan` renders dynamically for `course.plan.len()` days (clean `NoPlan` empty state if `plan` is empty). |
| `TUI-2` | Switching subjects (`↑` / `↓` or `/course`) persists `active_subject` AND **immediately calls `self.refresh()`**, resetting `plan_view_day` and reloading `course`, `plan`, `weakest`, `deneme`, and `coach_step` with zero 1-second stale-view lag. Default initial subject selection is sorted alphabetically with no hardcoded subject bias. |
| `TUI-3` | Today tab executes `/cards` (`/kelime`) directly inside TUI (generating & displaying flashcards cleanly without blocking it as `"Telegram'da üretilir"`). `F4` toggles `Lang::Tr <-> Lang::En` live, saves `state("lang")`, clears language-sensitive transient status, and redraws all chrome/tabs/help/onboarding/keycaps cleanly. |
| `TUI-4` | Zero `eprintln!` calls while TUI alternate screen is active; `TerminalGuard` + `ratatui::try_init()` panic restore remain intact; zero `.unwrap()` crash vectors in `app.rs` (`pending`, `session`, `key`). |

### Acceptance Criteria (`TUI-*`)
- **Given** TUI running on an empty `--root` -> **Then** clean bilingual onboarding displays without errors and `F4` immediately flips UI between Turkish and English.
- **Given** two subjects `A` (with 5-day plan) and `B` (no plan), pressing `↓` switches immediately from `A`'s plan view to `B`'s clean `NoPlan` view within the same key event.
- **Given** `run_command` / slash input `/cards` in TUI Today tab -> **Then** vocabulary generation starts directly in `App` background worker without returning a Telegram-only error.
