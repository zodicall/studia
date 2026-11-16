use crate::{
    Result,
    ai::Question,
    fsrs_engine::{
        self, BACKLOG_PROTECTION_THRESHOLD, BinaryRating, CardKind, CardState, CardStatus,
        DAILY_NEW_CARD_LIMIT, Flashcard, ScheduleOutcome,
    },
    invalid,
};
use chrono::{DateTime, Days, NaiveTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::{path::Path, time::Duration};

pub struct Store {
    conn: Connection,
}

#[derive(Clone, Debug)]
pub struct StoredQuestion {
    pub id: i64,
    pub value: Question,
    pub review_target: Option<i64>,
}

pub type SavedLesson = (String, Vec<StoredQuestion>, Vec<(String, String)>);

pub struct SessionStats {
    pub id: i64,
    pub subject: String,
    pub topic: String,
    pub started: DateTime<Utc>,
    pub ended: DateTime<Utc>,
    pub correct: i64,
    pub total: i64,
    pub minutes: f64,
}

#[derive(Clone, Debug)]
pub struct NewFlashcard {
    pub subject: String,
    pub tag: String,
    pub status: CardStatus,
    pub card_type: CardKind,
    pub context_prefix: String,
    pub front: String,
    pub back: String,
    pub extra_context: String,
    pub mnemonic_hint: Option<String>,
    pub is_high_yield: bool,
    pub source_question_id: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct FlashcardQueue {
    pub cards: Vec<Flashcard>,
    pub due_learning: usize,
    pub due_reviews: usize,
    pub new_available: usize,
    pub new_today: usize,
    pub backlog_paused: bool,
}

#[derive(Clone, Debug, Default)]
pub struct FlashcardStats {
    pub total_active: usize,
    pub due_learning: usize,
    pub due_reviews: usize,
    pub new_total: usize,
    pub new_today: usize,
    pub new_remaining_today: usize,
    pub backlog_paused: bool,
    pub incubation_count: usize,
    pub leech_count: usize,
    pub strategy_notes_count: usize,
    pub avg_stability: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagSummary {
    pub tag: String,
    pub total: usize,
    pub due: usize,
    pub new_count: usize,
    pub leech_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExamTechniqueNote {
    pub id: i64,
    pub subject: String,
    pub question_id: Option<i64>,
    pub tag: String,
    pub note: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug)]
pub struct LeechRepair<'a> {
    pub card_type: CardKind,
    pub context_prefix: &'a str,
    pub front: &'a str,
    pub back: &'a str,
    pub extra_context: &'a str,
    pub mnemonic_hint: Option<&'a str>,
}

pub fn now() -> String {
    Utc::now().to_rfc3339()
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let is_memory =
            path == Path::new(":memory:") || path.to_string_lossy().contains("mode=memory");
        if !is_memory && let Some(parent) = path.parent() {
            crate::config::ensure_private_dir(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        if !is_memory {
            crate::config::set_private_file_permissions(path)?;
            for attempt in 0..10 {
                match conn.pragma_update(None, "journal_mode", "WAL") {
                    Ok(()) => break,
                    Err(rusqlite::Error::SqliteFailure(err, _))
                        if err.code == rusqlite::ErrorCode::DatabaseBusy && attempt < 9 =>
                    {
                        std::thread::sleep(Duration::from_millis(20 * (attempt + 1)));
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        }
        // Support up to 5 choices (A–E); rebuild tables created with the old 0–3 CHECK.
        conn.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE;")?;
        let rebuild = (|| -> Result<()> {
            let old: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name IN ('questions','answers') AND sql LIKE '%BETWEEN 0 AND 3%')",
                [],
                |row| row.get(0),
            )?;
            if old {
                conn.execute_batch(
                    "CREATE TABLE questions_new(id INTEGER PRIMARY KEY, subject TEXT NOT NULL, topic TEXT NOT NULL, question TEXT NOT NULL, choices_json TEXT NOT NULL, answer_index INTEGER NOT NULL CHECK(answer_index BETWEEN 0 AND 4), explanation TEXT NOT NULL, source TEXT NOT NULL, created_at TEXT NOT NULL);
                     INSERT INTO questions_new SELECT * FROM questions;
                     DROP TABLE questions;
                     ALTER TABLE questions_new RENAME TO questions;
                     CREATE TABLE answers_new(id INTEGER PRIMARY KEY, question_id INTEGER NOT NULL REFERENCES questions(id), choice INTEGER NOT NULL CHECK(choice BETWEEN 0 AND 4), correct INTEGER NOT NULL, at TEXT NOT NULL);
                     INSERT INTO answers_new SELECT * FROM answers;
                     DROP TABLE answers;
                     ALTER TABLE answers_new RENAME TO answers;",
                )?;
            }
            Ok(())
        })();
        match rebuild {
            Ok(()) => conn.execute_batch("COMMIT;")?,
            Err(err) => {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err);
            }
        }
        conn.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;")?;
        let migrate = (|| -> Result<()> {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS state(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE IF NOT EXISTS history(id INTEGER PRIMARY KEY, subject TEXT NOT NULL, role TEXT NOT NULL, text TEXT NOT NULL, at TEXT NOT NULL);
                 CREATE TABLE IF NOT EXISTS questions(id INTEGER PRIMARY KEY, subject TEXT NOT NULL, topic TEXT NOT NULL, question TEXT NOT NULL, choices_json TEXT NOT NULL, answer_index INTEGER NOT NULL CHECK(answer_index BETWEEN 0 AND 4), explanation TEXT NOT NULL, source TEXT NOT NULL, created_at TEXT NOT NULL, is_exam INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE IF NOT EXISTS answers(id INTEGER PRIMARY KEY, question_id INTEGER NOT NULL REFERENCES questions(id), choice INTEGER NOT NULL CHECK(choice BETWEEN 0 AND 4), correct INTEGER NOT NULL, at TEXT NOT NULL);
                 CREATE TABLE IF NOT EXISTS sessions(id INTEGER PRIMARY KEY, subject TEXT NOT NULL, topic TEXT NOT NULL, started_at TEXT NOT NULL, ended_at TEXT, correct INTEGER NOT NULL DEFAULT 0, total INTEGER NOT NULL DEFAULT 0, kind TEXT NOT NULL DEFAULT 'quiz');
                 CREATE TABLE IF NOT EXISTS reviews(question_id INTEGER PRIMARY KEY REFERENCES questions(id), box INTEGER NOT NULL CHECK(box BETWEEN 1 AND 5), due_at TEXT NOT NULL);
                 CREATE TABLE IF NOT EXISTS flashcards(
                    id INTEGER PRIMARY KEY,
                    subject TEXT NOT NULL,
                    tag TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active','incubation','suspended')),
                    card_type TEXT NOT NULL CHECK(card_type IN ('qa','cloze','contrastive')),
                    context_prefix TEXT NOT NULL,
                    front TEXT NOT NULL,
                    back TEXT NOT NULL,
                    extra_context TEXT NOT NULL DEFAULT '',
                    mnemonic_hint TEXT,
                    is_high_yield INTEGER NOT NULL DEFAULT 0,
                    is_leech INTEGER NOT NULL DEFAULT 0,
                    source_question_id INTEGER REFERENCES questions(id) ON DELETE SET NULL,
                    state INTEGER NOT NULL DEFAULT 0 CHECK(state BETWEEN 0 AND 3),
                    stability REAL NOT NULL DEFAULT 0.0,
                    stability_fast REAL NOT NULL DEFAULT 0.0,
                    difficulty REAL NOT NULL DEFAULT 0.0,
                    elapsed_days REAL NOT NULL DEFAULT 0.0,
                    scheduled_days INTEGER NOT NULL DEFAULT 0,
                    learning_step INTEGER NOT NULL DEFAULT 0,
                    reps INTEGER NOT NULL DEFAULT 0,
                    lapses INTEGER NOT NULL DEFAULT 0,
                    last_review TEXT,
                    due TEXT NOT NULL,
                    created_at TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_flashcards_queue ON flashcards(subject, status, state, due);
                 CREATE INDEX IF NOT EXISTS idx_flashcards_tag ON flashcards(subject, tag);
                 CREATE TABLE IF NOT EXISTS review_logs(
                    id INTEGER PRIMARY KEY,
                    card_id INTEGER NOT NULL REFERENCES flashcards(id) ON DELETE CASCADE,
                    rating INTEGER NOT NULL CHECK(rating IN (1,3)),
                    state INTEGER NOT NULL,
                    stability REAL NOT NULL,
                    difficulty REAL NOT NULL,
                    elapsed_days REAL NOT NULL,
                    scheduled_days INTEGER NOT NULL,
                    duration_ms INTEGER NOT NULL DEFAULT 0,
                    reviewed_at TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_review_logs_card ON review_logs(card_id, reviewed_at);
                 CREATE TABLE IF NOT EXISTS exam_technique_notes(
                    id INTEGER PRIMARY KEY,
                    subject TEXT NOT NULL,
                    question_id INTEGER REFERENCES questions(id) ON DELETE SET NULL,
                    tag TEXT NOT NULL,
                    note TEXT NOT NULL,
                    created_at TEXT NOT NULL
                 );",
            )?;
            if !has_column(&conn, "sessions", "kind")? {
                conn.execute_batch(
                    "ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'quiz';
                     UPDATE sessions SET kind = 'exam' WHERE topic LIKE '%Deneme%';",
                )?;
            }
            if !has_column(&conn, "questions", "is_exam")? {
                conn.execute_batch(
                    "ALTER TABLE questions ADD COLUMN is_exam INTEGER NOT NULL DEFAULT 0;
                     UPDATE questions SET is_exam = 1 WHERE topic LIKE '%Deneme%';",
                )?;
            }
            Ok(())
        })();
        match migrate {
            Ok(()) => conn.execute_batch("COMMIT;")?,
            Err(err) => {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(err);
            }
        }
        Ok(Self { conn })
    }

    /// SQLite veritabanı bütünlüğü için hızlı denetim (`PRAGMA quick_check`).
    pub fn quick_check(&self) -> Result<bool> {
        let mut stmt = self.conn.prepare("PRAGMA quick_check")?;
        let rows: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows.len() == 1 && rows[0].eq_ignore_ascii_case("ok"))
    }

    pub fn state(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM state WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub fn set_state(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT INTO state(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [key, value])?;
        Ok(())
    }

    pub fn clear_state(&self, key: &str) -> Result<()> {
        self.conn.execute("DELETE FROM state WHERE key=?1", [key])?;
        Ok(())
    }

    /// Prefix deletion via `instr` avoids `LIKE` wildcard ambiguity on subject names.
    pub fn clear_state_prefix(&self, prefix: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM state WHERE instr(key, ?1) = 1", [prefix])?;
        Ok(())
    }

    /// Returns the stored native language name, or the fallback for `ui_lang` if unset.
    pub fn native_lang(&self, ui_lang: crate::i18n::Lang) -> Result<String> {
        let saved = self
            .state("native_lang")?
            .map(|raw| crate::i18n::normalize_native_lang(&raw))
            .filter(|norm| !norm.is_empty());
        Ok(saved.unwrap_or_else(|| ui_lang.default_native_name().to_string()))
    }

    /// Persists the user's native language name in `state("native_lang")`.
    pub fn set_native_lang(&self, raw: &str) -> Result<String> {
        let normalized = crate::i18n::normalize_native_lang(raw);
        if normalized.is_empty() {
            return Err(crate::invalid("Native language cannot be empty."));
        }
        self.set_state("native_lang", &normalized)?;
        Ok(normalized)
    }

    fn translation_cache_key(target_lang: &str, source_text: &str) -> String {
        let lang_norm = crate::i18n::normalize_native_lang(target_lang).to_ascii_lowercase();
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &byte in source_text.as_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        format!("tr_cache:{lang_norm}:{}:{hash:016x}", source_text.len())
    }

    /// Returns a cached translation of `source_text` into `target_lang`, if previously saved.
    pub fn cached_translation(
        &self,
        target_lang: &str,
        source_text: &str,
    ) -> Result<Option<String>> {
        self.state(&Self::translation_cache_key(target_lang, source_text))
    }

    /// Persists a translated text snippet in SQLite for instant re-use across sessions.
    pub fn save_translation(
        &self,
        target_lang: &str,
        source_text: &str,
        translated: &str,
    ) -> Result<()> {
        let key = Self::translation_cache_key(target_lang, source_text);
        self.set_state(&key, translated)
    }

    pub fn offset(&self) -> Result<i64> {
        match self.state("offset")? {
            Some(value) => Ok(value.parse()?),
            None => Ok(0),
        }
    }

    pub fn history(&self, subject: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT role,text FROM (SELECT id,role,text FROM history WHERE subject=?1 ORDER BY id DESC LIMIT 12) ORDER BY id")?;
        Ok(stmt
            .query_map([subject], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Newest-first question texts already stored for `subject`.
    pub fn recent_questions(&self, subject: &str, limit: usize) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT question FROM questions WHERE subject=?1 ORDER BY id DESC LIMIT ?2")?;
        Ok(stmt
            .query_map(params![subject, i64::try_from(limit)?], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn add_turn(&mut self, subject: &str, question: &str, answer: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        let at = now();
        for (role, text) in [("user", question), ("assistant", answer)] {
            tx.execute(
                "INSERT INTO history(subject,role,text,at) VALUES(?1,?2,?3,?4)",
                params![subject, role, text, at],
            )?;
        }
        tx.execute("DELETE FROM history WHERE subject=?1 AND id NOT IN (SELECT id FROM history WHERE subject=?1 ORDER BY id DESC LIMIT 12)", [subject])?;
        tx.commit()?;
        Ok(())
    }

    pub fn question_count(&self, subject: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM questions WHERE subject=?1",
            [subject],
            |row| row.get(0),
        )?)
    }

    pub fn add_questions(
        &mut self,
        subject: &str,
        topic: &str,
        questions: Vec<Question>,
    ) -> Result<Vec<StoredQuestion>> {
        self.add_questions_with_flag(subject, topic, false, questions)
    }

    pub fn add_exam_questions(
        &mut self,
        subject: &str,
        topic: &str,
        questions: Vec<Question>,
    ) -> Result<Vec<StoredQuestion>> {
        self.add_questions_with_flag(subject, topic, true, questions)
    }

    fn add_questions_with_flag(
        &mut self,
        subject: &str,
        topic: &str,
        is_exam: bool,
        questions: Vec<Question>,
    ) -> Result<Vec<StoredQuestion>> {
        let tx = self.conn.transaction()?;
        let mut saved = vec![];
        let at = now();
        let exam_flag: i64 = if is_exam { 1 } else { 0 };
        for mut value in questions {
            let q_topic = value.topic.clone().unwrap_or_else(|| topic.to_string());
            value.topic = Some(q_topic.clone());
            tx.execute(
                "INSERT INTO questions(subject,topic,question,choices_json,answer_index,explanation,source,created_at,is_exam) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    subject,
                    q_topic,
                    value.question,
                    serde_json::to_string(&value.choices)?,
                    value.answer_index as i64,
                    value.explanation,
                    value.source,
                    at,
                    exam_flag
                ],
            )?;
            saved.push(StoredQuestion {
                id: tx.last_insert_rowid(),
                value,
                review_target: None,
            });
        }
        tx.commit()?;
        Ok(saved)
    }

    pub fn unanswered(&self, subject: &str) -> Result<Vec<StoredQuestion>> {
        self.questions("SELECT id,question,choices_json,answer_index,explanation,source FROM questions q WHERE subject=?1 AND NOT EXISTS(SELECT 1 FROM answers a WHERE a.question_id=q.id) ORDER BY id", [subject])
    }

    pub fn due_reviews(
        &self,
        subject: &str,
        count: usize,
        at: DateTime<Utc>,
    ) -> Result<Vec<StoredQuestion>> {
        let count = i64::try_from(count)?;
        self.questions(
            "SELECT q.id,q.question,q.choices_json,q.answer_index,q.explanation,q.source FROM reviews r JOIN questions q ON q.id=r.question_id WHERE q.subject=?1 AND r.due_at<=?2 ORDER BY r.due_at,q.id LIMIT ?3",
            params![subject, at.date_naive().to_string(), count],
        )
    }

    pub fn review_counts(&self, subject: &str, at: DateTime<Utc>) -> Result<(i64, i64)> {
        Ok(self.conn.query_row(
            "SELECT COUNT(CASE WHEN r.due_at<=?2 THEN 1 END),COUNT(*) FROM reviews r JOIN questions q ON q.id=r.question_id WHERE q.subject=?1",
            params![subject, at.date_naive().to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    pub fn next_review(&self, subject: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row(
            "SELECT MIN(r.due_at) FROM reviews r JOIN questions q ON q.id=r.question_id WHERE q.subject=?1",
            [subject],
            |row| row.get(0),
        )?)
    }

    /// Exam questions (`is_exam = 1`) are excluded from original review replay; Leitner scheduling is unchanged.
    pub fn due_original_reviews(
        &self,
        subject: &str,
        count: usize,
        at: DateTime<Utc>,
    ) -> Result<Vec<StoredQuestion>> {
        let count = i64::try_from(count)?;
        self.questions(
            "SELECT q.id,q.question,q.choices_json,q.answer_index,q.explanation,q.source FROM reviews r JOIN questions q ON q.id=r.question_id WHERE q.subject=?1 AND q.is_exam=0 AND r.due_at<=?2 ORDER BY r.due_at,q.id LIMIT ?3",
            params![subject, at.date_naive().to_string(), count],
        )
    }

    /// Earliest due review date excluding exam questions; matches `due_original_reviews`.
    pub fn next_original_review(&self, subject: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row(
            "SELECT MIN(r.due_at) FROM reviews r JOIN questions q ON q.id=r.question_id WHERE q.subject=?1 AND q.is_exam=0",
            [subject],
            |row| row.get(0),
        )?)
    }

    fn questions(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<StoredQuestion>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? as usize,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut output = vec![];
        for row in rows {
            let (id, question, choices, answer_index, explanation, source) = row?;
            let value = Question {
                question,
                choices: serde_json::from_str(&choices)?,
                answer_index,
                explanation,
                source,
                topic: None,
            };
            if value.answer_index >= value.choices.len() {
                return Err(invalid(
                    "Kayıtlı soru geçersiz / Stored question is invalid.",
                ));
            }
            output.push(StoredQuestion {
                id,
                value,
                review_target: None,
            });
        }
        Ok(output)
    }

    #[cfg(test)]
    pub fn start_session(&self, subject: &str, topic: &str, at: &str) -> Result<i64> {
        self.start_session_with_kind(subject, topic, "quiz", at)
    }

    pub fn start_session_with_kind(
        &self,
        subject: &str,
        topic: &str,
        kind: &str,
        at: &str,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO sessions(subject,topic,kind,started_at) VALUES(?1,?2,?3,?4)",
            params![subject, topic, kind, at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    #[allow(dead_code)]
    pub fn record_answer(&mut self, session: i64, question: i64, choice: usize) -> Result<bool> {
        self.record_answer_with_review_at(session, question, choice, question, Utc::now())
    }

    #[allow(dead_code)]
    pub fn record_answer_at(
        &mut self,
        session: i64,
        question: i64,
        choice: usize,
        at: DateTime<Utc>,
    ) -> Result<bool> {
        self.record_answer_with_review_at(session, question, choice, question, at)
    }

    pub fn record_answer_with_review(
        &mut self,
        session: i64,
        question: i64,
        choice: usize,
        review_target: i64,
    ) -> Result<bool> {
        self.record_answer_with_review_at(session, question, choice, review_target, Utc::now())
    }

    pub fn record_answer_with_review_at(
        &mut self,
        session: i64,
        question: i64,
        choice: usize,
        review_target: i64,
        at: DateTime<Utc>,
    ) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let (answer, choices_json): (i64, String) = tx.query_row(
            "SELECT answer_index, choices_json FROM questions WHERE id=?1",
            [question],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let choices: Vec<String> = serde_json::from_str(&choices_json)?;
        if choice >= choices.len() {
            return Err(invalid("Seçenek geçersiz / Invalid choice."));
        }
        let correct = choice as i64 == answer;
        if tx.execute(
            "UPDATE sessions SET correct=correct+?1,total=total+1 WHERE id=?2 AND ended_at IS NULL",
            params![correct, session],
        )? != 1
        {
            return Err(invalid("Quiz oturumu kapalı / Quiz session is closed."));
        }
        tx.execute(
            "INSERT INTO answers(question_id,choice,correct,at) VALUES(?1,?2,?3,?4)",
            params![question, choice as i64, correct, at.to_rfc3339()],
        )?;
        let current_box: Option<u32> = tx
            .query_row(
                "SELECT box FROM reviews WHERE question_id=?1",
                [review_target],
                |row| row.get(0),
            )
            .optional()?;
        if correct && current_box == Some(5) {
            tx.execute("DELETE FROM reviews WHERE question_id=?1", [review_target])?;
        } else if !correct || current_box.is_some() {
            let next_box = match current_box {
                Some(b) if correct => b + 1,
                _ => 1,
            };
            // Reviews use UTC calendar dates: every question due today is available all day.
            let due_at = at
                .date_naive()
                .checked_add_days(Days::new(1 << (next_box - 1)))
                .ok_or_else(|| {
                    invalid("Tekrar tarihi hesaplanamadı / Failed to compute review due date.")
                })?;
            tx.execute(
                "INSERT INTO reviews(question_id,box,due_at) VALUES(?1,?2,?3) ON CONFLICT(question_id) DO UPDATE SET box=excluded.box,due_at=excluded.due_at",
                params![review_target, next_box, due_at.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(correct)
    }

    /// Deneme geçmişinde gösterilen toplam: bitişte kaydedilen planlanan soru sayısı.
    /// `total` ise yalnızca cevaplananları sayar; istatistikler onu kullanmaya devam eder.
    pub fn set_deneme_total(&self, session: i64, planned: usize) -> Result<()> {
        self.set_state(&format!("deneme_total:{session}"), &planned.to_string())
    }

    pub fn end_session(&self, session: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE sessions SET ended_at=?1 WHERE id=?2 AND ended_at IS NULL",
            params![now(), session],
        )? == 1)
    }

    pub fn close_interrupted_sessions(&self) -> Result<()> {
        // Only the listener's previous session belongs to this process on restart.
        self.conn.execute(
            "UPDATE sessions SET ended_at=?1 WHERE ended_at IS NULL AND id=(SELECT value FROM state WHERE key='listener_session')",
            [now()],
        )?;
        Ok(())
    }

    pub fn recent_sessions(&self) -> Result<Vec<SessionStats>> {
        self.query_session_stats(
            "SELECT subject,topic,started_at,ended_at,correct,total,id FROM sessions ORDER BY id DESC LIMIT 10",
            [],
        )
    }

    /// Ders bazında toplam (doğru, toplam); yalnızca en az bir cevabı olan oturumlar.
    pub fn subject_accuracy(&self) -> Result<Vec<(String, i64, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT subject, SUM(correct), SUM(total) FROM sessions WHERE total > 0 GROUP BY subject ORDER BY subject",
        )?;
        Ok(stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Bir dersin en zayıf konuları (doğruluk artan). Deneme oturumları hariç; en az üç cevap gerekir.
    pub fn weakest_topics(&self, subject: &str, limit: usize) -> Result<Vec<(String, i64, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT topic, SUM(correct), SUM(total) FROM sessions \
             WHERE subject = ?1 AND total > 0 AND kind != 'exam' \
             GROUP BY topic HAVING SUM(total) >= 3 \
             ORDER BY CAST(SUM(correct) AS REAL) / SUM(total) ASC, SUM(total) DESC LIMIT ?2",
        )?;
        Ok(stmt
            .query_map(params![subject, i64::try_from(limit)?], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Dersin tüm konuları için toplam (konu, doğru, toplam); deneme oturumları hariç.
    pub fn topic_totals(&self, subject: &str) -> Result<Vec<(String, i64, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT topic, SUM(correct), SUM(total) FROM sessions \
             WHERE subject = ?1 AND total > 0 AND kind != 'exam' GROUP BY topic",
        )?;
        Ok(stmt
            .query_map([subject], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// `since` (UTC, YYYY-MM-DD) ve sonrasındaki günlük (gün, doğru, toplam).
    pub fn daily_accuracy(&self, since: &str) -> Result<Vec<(String, i64, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT substr(started_at, 1, 10), SUM(correct), SUM(total) FROM sessions \
             WHERE total > 0 AND substr(started_at, 1, 10) >= ?1 GROUP BY 1 ORDER BY 1",
        )?;
        Ok(stmt
            .query_map([since], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn deneme_sessions(&self, subject: &str, limit: usize) -> Result<Vec<SessionStats>> {
        self.query_session_stats(
            "SELECT subject,topic,started_at,ended_at,correct,COALESCE((SELECT CAST(value AS INTEGER) FROM state WHERE key='deneme_total:'||sessions.id),total),sessions.id FROM sessions WHERE subject=?1 AND kind='exam' AND ended_at IS NOT NULL AND (total > 0 OR EXISTS(SELECT 1 FROM state WHERE key='deneme_total:'||sessions.id)) ORDER BY id DESC LIMIT ?2",
            params![subject, i64::try_from(limit)?],
        )
    }

    fn query_session_stats(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<SessionStats>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        let mut output = vec![];
        for row in rows {
            let (subject, topic, started, ended, correct, total, id) = row?;
            let started = DateTime::parse_from_rfc3339(&started)?.with_timezone(&Utc);
            let ended = ended
                .map(|value| DateTime::parse_from_rfc3339(&value))
                .transpose()?
                .map(|value| value.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);
            output.push(SessionStats {
                id,
                subject,
                topic,
                started,
                correct,
                total,
                ended,
                minutes: (ended - started).num_seconds().max(0) as f64 / 60.0,
            });
        }
        Ok(output)
    }

    /// Bir denemenin cevapları: konu adı eşleşen ve `from`..=`to` aralığındaki cevaplar, cevap
    /// sırasıyla. Cevap tablosunda oturum kolonu yok; aynı konu başka oturumlarda da geçebildiği
    /// için zaman aralığı oturumu ayırır.
    pub fn deneme_answers(
        &self,
        session: i64,
        topic: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<(StoredQuestion, usize, bool)>> {
        type Raw = (
            i64,
            String,
            String,
            i64,
            String,
            String,
            String,
            i64,
            i64,
            String,
        );
        let read = |row: &rusqlite::Row| -> rusqlite::Result<Raw> {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ))
        };
        let rows: Vec<Raw> = match self.deneme_question_ids(session)? {
            // Oturumun soru kimlikleri biliniyorsa konu adına bakılmaz: AI denemelerinin soruları
            // "Deneme Sınavı", oturumu ise "Deneme Sınavı" konusuyla açılır.
            Some(ids) => {
                let mut stmt = self.conn.prepare(
                    "SELECT q.id,q.question,q.choices_json,q.answer_index,q.explanation,q.source,q.topic,a.choice,a.correct,a.at FROM answers a JOIN questions q ON q.id=a.question_id WHERE q.id=?1 ORDER BY a.id",
                )?;
                let mut rows = vec![];
                for id in ids {
                    rows.extend(
                        stmt.query_map([id], read)?
                            .collect::<rusqlite::Result<Vec<Raw>>>()?,
                    );
                }
                rows
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT q.id,q.question,q.choices_json,q.answer_index,q.explanation,q.source,q.topic,a.choice,a.correct,a.at FROM answers a JOIN questions q ON q.id=a.question_id WHERE q.topic=?1 ORDER BY a.id",
                )?;
                stmt.query_map([topic], read)?
                    .collect::<rusqlite::Result<_>>()?
            }
        };
        let mut output = vec![];
        for (
            id,
            question,
            choices,
            answer_index,
            explanation,
            source,
            stored_topic,
            choice,
            correct,
            at,
        ) in rows
        {
            let at = DateTime::parse_from_rfc3339(&at)?.with_timezone(&Utc);
            // Aynı soru satırı başka bir oturumda da cevaplanmış olabilir; yalnızca bu oturumun aralığı.
            if at < from || at > to {
                continue;
            }
            let choices_vec: Vec<String> = serde_json::from_str(&choices)?;
            let answer_idx = usize::try_from(answer_index)
                .map_err(|_| invalid("Kayıtlı soru geçersiz / Stored question is invalid."))?;
            let choice_idx = usize::try_from(choice)
                .map_err(|_| invalid("Kayıtlı cevap geçersiz / Stored answer is invalid."))?;
            if answer_idx >= choices_vec.len() || choice_idx >= choices_vec.len() {
                return Err(invalid(
                    "Kayıtlı soru veya cevap geçersiz / Stored question or answer is invalid.",
                ));
            }
            let value = Question {
                question,
                choices: choices_vec,
                answer_index: answer_idx,
                explanation,
                source,
                topic: Some(stored_topic),
            };
            output.push((
                StoredQuestion {
                    id,
                    value,
                    review_target: None,
                },
                choice_idx,
                correct != 0,
            ));
        }
        Ok(output)
    }

    /// Deneme oturumunun soru kimlikleri, sırayla; oturum başında kaydedilir.
    pub fn set_deneme_questions(&self, session: i64, ids: &[i64]) -> Result<()> {
        self.set_state(
            &format!("deneme_questions:{session}"),
            &serde_json::to_string(ids)?,
        )
    }

    /// Oturumun soru kimlikleri; eski (kimliksiz) denemelerde `None`.
    pub fn deneme_question_ids(&self, session: i64) -> Result<Option<Vec<i64>>> {
        match self.state(&format!("deneme_questions:{session}"))? {
            Some(value) => Ok(Some(serde_json::from_str(&value)?)),
            None => Ok(None),
        }
    }

    /// Konusu tam olarak `topic` olan soruların kimlikleri.
    pub fn question_ids_by_topic(&self, topic: &str) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM questions WHERE topic=?1 ORDER BY id")?;
        let ids = stmt
            .query_map([topic], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// Ders anlatımını, kazanım sorularının kimliklerini ve ders içi sohbet turlarını kaydeder.
    pub fn save_lesson(
        &self,
        subject: &str,
        lang: crate::i18n::Lang,
        topic: &str,
        lesson: &str,
        questions: &[StoredQuestion],
        turns: &[(String, String)],
    ) -> Result<()> {
        let ids: Vec<i64> = questions.iter().map(|q| q.id).collect();
        let value = serde_json::json!({
            "lesson": lesson,
            "questions": ids,
            "turns": turns,
        });
        self.set_state(
            &format!("lesson:{subject}:{}:{topic}", lang.code()),
            &value.to_string(),
        )
    }

    /// Kayıtlı ders anlatımını, sorularını ve sohbet geçmişini yükler; yoksa `None`.
    pub fn load_lesson(
        &self,
        subject: &str,
        lang: crate::i18n::Lang,
        topic: &str,
    ) -> Result<Option<SavedLesson>> {
        let Some(raw) = self.state(&format!("lesson:{subject}:{}:{topic}", lang.code()))? else {
            return Ok(None);
        };
        let parsed: serde_json::Value = serde_json::from_str(&raw)?;
        let Some(lesson) = parsed["lesson"].as_str().map(String::from) else {
            return Ok(None);
        };
        let mut questions = Vec::new();
        if let Some(ids) = parsed["questions"].as_array() {
            for id in ids.iter().filter_map(|v| v.as_i64()) {
                questions.extend(self.questions(
                    "SELECT id,question,choices_json,answer_index,explanation,source FROM questions WHERE id=?1",
                    [id],
                )?);
            }
        }
        if questions.is_empty() {
            return Ok(None);
        }
        let turns: Vec<(String, String)> = parsed
            .get("turns")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        Ok(Some((lesson, questions, turns)))
    }

    /// Denemeyi siler: oturum, durum anahtarları ve `from`..=`to` aralığındaki cevaplar. Cevabı
    /// kalmayan sorular ve Leitner kayıtları da silinir; aralık dışında cevaplanmış soru kalır.
    pub fn delete_deneme(
        &mut self,
        session: i64,
        question_ids: &[i64],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        for &question in question_ids {
            let answers: Vec<(i64, String)> = tx
                .prepare("SELECT id,at FROM answers WHERE question_id=?1")?
                .query_map([question], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (answer, at) in answers {
                let at = DateTime::parse_from_rfc3339(&at)?.with_timezone(&Utc);
                if at >= from && at <= to {
                    tx.execute("DELETE FROM answers WHERE id=?1", [answer])?;
                }
            }
            let left: i64 = tx.query_row(
                "SELECT COUNT(*) FROM answers WHERE question_id=?1",
                [question],
                |row| row.get(0),
            )?;
            if left == 0 {
                tx.execute("DELETE FROM reviews WHERE question_id=?1", [question])?;
                tx.execute("DELETE FROM questions WHERE id=?1", [question])?;
            }
        }
        tx.execute("DELETE FROM sessions WHERE id=?1", [session])?;
        tx.execute(
            "DELETE FROM state WHERE key=?1 OR key=?2",
            params![
                format!("deneme_total:{session}"),
                format!("deneme_questions:{session}")
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn question_by_id(&self, id: i64) -> Result<Option<(String, StoredQuestion)>> {
        let row: Option<(String, String, String, String, i64, String, String)> = self
            .conn
            .query_row(
                "SELECT subject,topic,question,choices_json,answer_index,explanation,source FROM questions WHERE id=?1",
                [id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((subject, topic, question, choices_json, answer_index, explanation, source)) = row
        else {
            return Ok(None);
        };
        let choices: Vec<String> = serde_json::from_str(&choices_json)?;
        let answer_index = usize::try_from(answer_index)
            .map_err(|_| invalid("Kayıtlı soru geçersiz / Stored question is invalid."))?;
        if answer_index >= choices.len() {
            return Err(invalid(
                "Kayıtlı soru geçersiz / Stored question is invalid.",
            ));
        }
        Ok(Some((
            subject,
            StoredQuestion {
                id,
                value: Question {
                    question,
                    choices,
                    answer_index,
                    explanation,
                    source,
                    topic: Some(topic),
                },
                review_target: None,
            },
        )))
    }

    pub fn add_flashcards(
        &mut self,
        cards: Vec<NewFlashcard>,
        now: DateTime<Utc>,
    ) -> Result<Vec<Flashcard>> {
        let tx = self.conn.transaction()?;
        let now_str = now.to_rfc3339();
        let mut saved = Vec::with_capacity(cards.len());
        for card in cards {
            let tag = normalize_tag(&card.tag);
            tx.execute(
                "INSERT INTO flashcards(subject,tag,status,card_type,context_prefix,front,back,extra_context,mnemonic_hint,is_high_yield,is_leech,source_question_id,state,stability,stability_fast,difficulty,elapsed_days,scheduled_days,learning_step,reps,lapses,last_review,due,created_at) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,0,?11,0,0.0,0.0,0.0,0.0,0,0,0,0,NULL,?12,?12)",
                params![
                    card.subject,
                    tag,
                    card.status.as_str(),
                    card.card_type.as_str(),
                    card.context_prefix.trim(),
                    card.front.trim(),
                    card.back.trim(),
                    card.extra_context.trim(),
                    card.mnemonic_hint.as_deref().map(str::trim).filter(|s| !s.is_empty()),
                    i64::from(card.is_high_yield),
                    card.source_question_id,
                    now_str,
                ],
            )?;
            let id = tx.last_insert_rowid();
            saved.push(Flashcard {
                id,
                subject: card.subject,
                tag,
                status: card.status,
                card_type: card.card_type,
                context_prefix: card.context_prefix.trim().to_string(),
                front: card.front.trim().to_string(),
                back: card.back.trim().to_string(),
                extra_context: card.extra_context.trim().to_string(),
                mnemonic_hint: card
                    .mnemonic_hint
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                is_high_yield: card.is_high_yield,
                is_leech: false,
                source_question_id: card.source_question_id,
                state: CardState::New,
                stability: 0.0,
                stability_fast: 0.0,
                difficulty: 0.0,
                elapsed_days: 0.0,
                scheduled_days: 0,
                learning_step: 0,
                reps: 0,
                lapses: 0,
                last_review: None,
                due: now,
                created_at: now,
            });
        }
        tx.commit()?;
        Ok(saved)
    }

    pub fn flashcard(&self, id: i64) -> Result<Option<Flashcard>> {
        let mut stmt = self.conn.prepare(FLASHCARD_SELECT_BY_ID)?;
        let mut rows = stmt.query([id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_flashcard(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn active_flashcards(
        &self,
        subject: &str,
        tag_prefix: Option<&str>,
    ) -> Result<Vec<Flashcard>> {
        let mut cards = self.load_flashcards_by_status(subject, CardStatus::Active)?;
        if let Some(prefix) = tag_prefix.map(normalize_tag).filter(|t| !t.is_empty()) {
            cards.retain(|c| tag_matches_prefix(&c.tag, &prefix));
        }
        Ok(cards)
    }

    pub fn incubation_cards(&self, subject: &str) -> Result<Vec<Flashcard>> {
        self.load_flashcards_by_status(subject, CardStatus::Incubation)
    }

    pub fn approve_incubation_card(&self, card_id: i64, now: DateTime<Utc>) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE flashcards SET status='active', due=?1 WHERE id=?2 AND status='incubation'",
            params![now.to_rfc3339(), card_id],
        )? == 1)
    }

    pub fn reject_incubation_card(&self, card_id: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM flashcards WHERE id=?1 AND status='incubation'",
            [card_id],
        )? == 1)
    }

    pub fn apply_leech_repair(&self, card_id: i64, repair: &LeechRepair<'_>) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE flashcards SET card_type=?1, context_prefix=?2, front=?3, back=?4, extra_context=?5, mnemonic_hint=?6, is_leech=0 WHERE id=?7",
            params![
                repair.card_type.as_str(),
                repair.context_prefix.trim(),
                repair.front.trim(),
                repair.back.trim(),
                repair.extra_context.trim(),
                repair
                    .mnemonic_hint
                    .map(str::trim)
                    .filter(|s| !s.is_empty()),
                card_id,
            ],
        )? == 1)
    }

    /// Builds today's FSRS study queue:
    /// 1) Due intraday Learning/Relearning cards (earliest due first)
    /// 2) Due Review cards sorted by Descending Retrievability (highest R first)
    /// 3) New cards capped by daily quota (20), automatically paused if subject due reviews >= 150.
    pub fn due_flashcards_queue(
        &self,
        subject: &str,
        tag_prefix: Option<&str>,
        high_yield_only: bool,
        now: DateTime<Utc>,
    ) -> Result<FlashcardQueue> {
        let all_active = self.load_flashcards_by_status(subject, CardStatus::Active)?;
        let total_due_reviews = all_active
            .iter()
            .filter(|c| c.state == CardState::Review && c.due <= now)
            .count();
        let backlog_paused = total_due_reviews >= BACKLOG_PROTECTION_THRESHOLD;
        let new_today = self.new_flashcards_studied_today(subject, now)?;
        let new_quota_left = if backlog_paused {
            0
        } else {
            DAILY_NEW_CARD_LIMIT.saturating_sub(new_today)
        };

        let prefix = tag_prefix.map(normalize_tag).filter(|t| !t.is_empty());
        let mut learning = Vec::new();
        let mut reviews = Vec::new();
        let mut new_cards = Vec::new();

        for card in all_active {
            if let Some(ref p) = prefix
                && !tag_matches_prefix(&card.tag, p)
            {
                continue;
            }
            if high_yield_only && !card.is_high_yield {
                continue;
            }
            match card.state {
                CardState::Learning | CardState::Relearning if card.due <= now => {
                    learning.push(card);
                }
                CardState::Review if card.due <= now => {
                    let r = fsrs_engine::retrievability(&card, now, None);
                    reviews.push((r, card));
                }
                CardState::New => new_cards.push(card),
                _ => {}
            }
        }

        learning.sort_by_key(|c| (c.due, c.id));
        // Descending Retrievability: preserve cards closest to threshold before deep backlog.
        reviews.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| b.1.is_high_yield.cmp(&a.1.is_high_yield))
                .then_with(|| a.1.due.cmp(&b.1.due))
                .then_with(|| a.1.id.cmp(&b.1.id))
        });
        new_cards.sort_by(|a, b| {
            b.is_high_yield
                .cmp(&a.is_high_yield)
                .then_with(|| a.id.cmp(&b.id))
        });
        new_cards.truncate(new_quota_left);

        let due_learning = learning.len();
        let due_reviews = reviews.len();
        let new_available = new_cards.len();

        let mut cards = Vec::with_capacity(due_learning + due_reviews + new_available);
        cards.extend(learning);
        cards.extend(reviews.into_iter().map(|(_, c)| c));
        cards.extend(new_cards);

        Ok(FlashcardQueue {
            cards,
            due_learning,
            due_reviews,
            new_available,
            new_today,
            backlog_paused,
        })
    }

    pub fn record_flashcard_review(
        &mut self,
        card_id: i64,
        rating: BinaryRating,
        duration_ms: i64,
        now: DateTime<Utc>,
    ) -> Result<ScheduleOutcome> {
        let tx = self.conn.transaction()?;
        let card = {
            let mut stmt = tx.prepare(FLASHCARD_SELECT_BY_ID)?;
            let mut rows = stmt.query([card_id])?;
            let Some(row) = rows.next()? else {
                return Err(invalid("Kart bulunamadı / Flashcard not found."));
            };
            row_to_flashcard(row)?
        };

        let outcome = fsrs_engine::schedule_card(&card, rating, now, None)?;
        let u = &outcome.updated;

        tx.execute(
            "INSERT INTO review_logs(card_id,rating,state,stability,difficulty,elapsed_days,scheduled_days,duration_ms,reviewed_at) \
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                card.id,
                rating.as_i64(),
                card.state.as_i64(),
                u.stability,
                u.difficulty,
                u.elapsed_days,
                u.scheduled_days,
                duration_ms.max(0),
                now.to_rfc3339(),
            ],
        )?;

        tx.execute(
            "UPDATE flashcards SET state=?1, stability=?2, stability_fast=?3, difficulty=?4, elapsed_days=?5, scheduled_days=?6, learning_step=?7, reps=?8, lapses=?9, is_leech=?10, last_review=?11, due=?12 WHERE id=?13",
            params![
                u.state.as_i64(),
                u.stability,
                u.stability_fast,
                u.difficulty,
                u.elapsed_days,
                u.scheduled_days,
                i64::from(u.learning_step),
                u.reps,
                u.lapses,
                i64::from(u.is_leech),
                u.last_review.map(|d| d.to_rfc3339()),
                u.due.to_rfc3339(),
                u.id,
            ],
        )?;
        tx.commit()?;
        Ok(outcome)
    }

    pub fn flashcard_stats(&self, subject: &str, now: DateTime<Utc>) -> Result<FlashcardStats> {
        let active = self.load_flashcards_by_status(subject, CardStatus::Active)?;
        let total_active = active.len();
        let due_learning = active
            .iter()
            .filter(|c| {
                matches!(c.state, CardState::Learning | CardState::Relearning) && c.due <= now
            })
            .count();
        let due_reviews = active
            .iter()
            .filter(|c| c.state == CardState::Review && c.due <= now)
            .count();
        let new_total = active.iter().filter(|c| c.state == CardState::New).count();
        let leech_count = active.iter().filter(|c| c.is_leech).count();
        let stability_sum: f64 = active
            .iter()
            .filter(|c| c.stability > 0.0)
            .map(|c| c.stability)
            .sum();
        let stability_n = active.iter().filter(|c| c.stability > 0.0).count();
        let avg_stability = if stability_n > 0 {
            stability_sum / stability_n as f64
        } else {
            0.0
        };
        let new_today = self.new_flashcards_studied_today(subject, now)?;
        let backlog_paused = due_reviews >= BACKLOG_PROTECTION_THRESHOLD;
        let new_remaining_today = if backlog_paused {
            0
        } else {
            DAILY_NEW_CARD_LIMIT
                .saturating_sub(new_today)
                .min(new_total)
        };
        let incubation_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM flashcards WHERE subject=?1 AND status='incubation'",
            [subject],
            |r| r.get(0),
        )?;
        let strategy_notes_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM exam_technique_notes WHERE subject=?1",
            [subject],
            |r| r.get(0),
        )?;

        Ok(FlashcardStats {
            total_active,
            due_learning,
            due_reviews,
            new_total,
            new_today,
            new_remaining_today,
            backlog_paused,
            incubation_count: usize::try_from(incubation_count).unwrap_or(0),
            leech_count,
            strategy_notes_count: usize::try_from(strategy_notes_count).unwrap_or(0),
            avg_stability,
        })
    }

    pub fn flashcard_tag_tree(&self, subject: &str, now: DateTime<Utc>) -> Result<Vec<TagSummary>> {
        use std::collections::BTreeMap;
        let active = self.load_flashcards_by_status(subject, CardStatus::Active)?;
        let mut by_tag: BTreeMap<String, TagSummary> = BTreeMap::new();
        for card in &active {
            let mut prefix = String::new();
            for part in card.tag.split("::").filter(|p| !p.is_empty()) {
                if prefix.is_empty() {
                    prefix.push_str(part);
                } else {
                    prefix.push_str("::");
                    prefix.push_str(part);
                }
                let entry = by_tag.entry(prefix.clone()).or_insert_with(|| TagSummary {
                    tag: prefix.clone(),
                    total: 0,
                    due: 0,
                    new_count: 0,
                    leech_count: 0,
                });
                entry.total += 1;
                if card.state == CardState::New {
                    entry.new_count += 1;
                } else if card.due <= now {
                    entry.due += 1;
                }
                if card.is_leech {
                    entry.leech_count += 1;
                }
            }
        }
        Ok(by_tag.into_values().collect())
    }

    pub fn add_exam_technique_note(
        &self,
        subject: &str,
        question_id: Option<i64>,
        tag: &str,
        note: &str,
        now: DateTime<Utc>,
    ) -> Result<i64> {
        let tag = normalize_tag(tag);
        self.conn.execute(
            "INSERT INTO exam_technique_notes(subject,question_id,tag,note,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![subject, question_id, tag, note.trim(), now.to_rfc3339()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn exam_technique_notes(
        &self,
        subject: &str,
        limit: usize,
    ) -> Result<Vec<ExamTechniqueNote>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,subject,question_id,tag,note,created_at FROM exam_technique_notes WHERE subject=?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![subject, i64::try_from(limit)?], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, subject, question_id, tag, note, created_at) = row?;
            out.push(ExamTechniqueNote {
                id,
                subject,
                question_id,
                tag,
                note,
                created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
            });
        }
        Ok(out)
    }

    fn new_flashcards_studied_today(&self, subject: &str, now: DateTime<Utc>) -> Result<usize> {
        let today_start = now
            .date_naive()
            .and_time(NaiveTime::MIN)
            .and_utc()
            .to_rfc3339();
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT l.card_id) FROM review_logs l JOIN flashcards f ON f.id = l.card_id \
             WHERE f.subject=?1 AND l.state=0 AND l.reviewed_at>=?2",
            params![subject, today_start],
            |r| r.get(0),
        )?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    fn load_flashcards_by_status(
        &self,
        subject: &str,
        status: CardStatus,
    ) -> Result<Vec<Flashcard>> {
        let mut stmt = self.conn.prepare(FLASHCARD_SELECT_BY_SUBJECT_STATUS)?;
        let mut rows = stmt.query(params![subject, status.as_str()])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_flashcard(row)?);
        }
        Ok(out)
    }
}

const FLASHCARD_COLS: &str = "id,subject,tag,status,card_type,context_prefix,front,back,extra_context,mnemonic_hint,is_high_yield,is_leech,source_question_id,state,stability,stability_fast,difficulty,elapsed_days,scheduled_days,learning_step,reps,lapses,last_review,due,created_at";
const FLASHCARD_SELECT_BY_ID: &str = "SELECT id,subject,tag,status,card_type,context_prefix,front,back,extra_context,mnemonic_hint,is_high_yield,is_leech,source_question_id,state,stability,stability_fast,difficulty,elapsed_days,scheduled_days,learning_step,reps,lapses,last_review,due,created_at FROM flashcards WHERE id=?1";
const FLASHCARD_SELECT_BY_SUBJECT_STATUS: &str = "SELECT id,subject,tag,status,card_type,context_prefix,front,back,extra_context,mnemonic_hint,is_high_yield,is_leech,source_question_id,state,stability,stability_fast,difficulty,elapsed_days,scheduled_days,learning_step,reps,lapses,last_review,due,created_at FROM flashcards WHERE subject=?1 AND status=?2 ORDER BY id";

fn row_to_flashcard(row: &rusqlite::Row<'_>) -> Result<Flashcard> {
    let _ = FLASHCARD_COLS;
    let last_review: Option<String> = row.get(22)?;
    let due: String = row.get(23)?;
    let created_at: String = row.get(24)?;
    Ok(Flashcard {
        id: row.get(0)?,
        subject: row.get(1)?,
        tag: row.get(2)?,
        status: CardStatus::parse(&row.get::<_, String>(3)?)?,
        card_type: CardKind::parse(&row.get::<_, String>(4)?)?,
        context_prefix: row.get(5)?,
        front: row.get(6)?,
        back: row.get(7)?,
        extra_context: row.get(8)?,
        mnemonic_hint: row.get(9)?,
        is_high_yield: row.get::<_, i64>(10)? != 0,
        is_leech: row.get::<_, i64>(11)? != 0,
        source_question_id: row.get(12)?,
        state: CardState::from_i64(row.get(13)?)?,
        stability: row.get(14)?,
        stability_fast: row.get(15)?,
        difficulty: row.get(16)?,
        elapsed_days: row.get(17)?,
        scheduled_days: row.get(18)?,
        learning_step: u8::try_from(row.get::<_, i64>(19)?).unwrap_or(0),
        reps: row.get(20)?,
        lapses: row.get(21)?,
        last_review: last_review
            .as_deref()
            .map(DateTime::parse_from_rfc3339)
            .transpose()?
            .map(|d| d.with_timezone(&Utc)),
        due: DateTime::parse_from_rfc3339(&due)?.with_timezone(&Utc),
        created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
    })
}

pub fn normalize_tag(raw: &str) -> String {
    let cleaned = raw.trim();
    if cleaned.is_empty() {
        "#Genel".into()
    } else if cleaned.starts_with('#') {
        cleaned.to_string()
    } else {
        format!("#{cleaned}")
    }
}

fn tag_matches_prefix(tag: &str, prefix: &str) -> bool {
    tag == prefix
        || tag
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with("::"))
}

fn has_column(conn: &Connection, table: &str, col: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let found = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == col);
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn review(store: &Store, question: i64) -> Option<(u32, String)> {
        store
            .conn
            .query_row(
                "SELECT box,due_at FROM reviews WHERE question_id=?1",
                [question],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap()
    }

    fn question() -> Question {
        Question {
            question: "Soru".into(),
            choices: ["a", "b", "c", "d"].map(String::from).into(),
            answer_index: 1,
            explanation: "Açıklama".into(),
            source: "ders.md".into(),
            topic: None,
        }
    }

    #[test]
    fn four_choice_tables_migrate_to_five_and_keep_rows() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("old.db");
        Connection::open(&path).unwrap().execute_batch(
            "CREATE TABLE questions(id INTEGER PRIMARY KEY, subject TEXT NOT NULL, topic TEXT NOT NULL, question TEXT NOT NULL, choices_json TEXT NOT NULL, answer_index INTEGER NOT NULL CHECK(answer_index BETWEEN 0 AND 3), explanation TEXT NOT NULL, source TEXT NOT NULL, created_at TEXT NOT NULL);
             CREATE TABLE answers(id INTEGER PRIMARY KEY, question_id INTEGER NOT NULL REFERENCES questions(id), choice INTEGER NOT NULL CHECK(choice BETWEEN 0 AND 3), correct INTEGER NOT NULL, at TEXT NOT NULL);
             INSERT INTO questions VALUES(1,'D','t','Q?','[\"a\",\"b\",\"c\",\"d\"]',1,'e','s','x');
             INSERT INTO answers VALUES(1,1,1,1,'x');",
        ).unwrap();
        let mut store = Store::open(&path).unwrap();
        assert!(store.unanswered("D").unwrap().is_empty());
        let five = Question {
            question: "Q5?".into(),
            choices: ["a", "b", "c", "d", "e"].map(String::from).into(),
            answer_index: 4,
            explanation: "e".into(),
            source: "s".into(),
            topic: None,
        };
        let saved = store.add_questions("D", "t", vec![five]).unwrap();
        let session = store.start_session("D", "t", &now()).unwrap();
        assert!(store.record_answer(session, saved[0].id, 4).unwrap());
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM answers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn offset_roundtrip_history_retention_and_subject_isolation() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        assert_eq!(store.offset().unwrap(), 0);
        store.set_state("offset", "987").unwrap();
        assert_eq!(store.offset().unwrap(), 987);
        store.add_turn("Diğer", "other", "answer").unwrap();
        for n in 0..8 {
            store
                .add_turn("Ders", &format!("q{n}"), &format!("a{n}"))
                .unwrap();
        }
        let history = store.history("Ders").unwrap();
        assert_eq!(history.len(), 12);
        assert_eq!(history[0], ("user".into(), "q2".into()));
        assert_eq!(history[11], ("assistant".into(), "a7".into()));
        assert_eq!(store.history("Diğer").unwrap().len(), 2);
    }

    #[test]
    fn recent_questions_are_newest_first_limited_and_subject_scoped() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        for text in ["S1", "S2", "S3"] {
            let mut q = question();
            q.question = text.into();
            store.add_questions("Ders", "Konu", vec![q]).unwrap();
        }
        let mut other = question();
        other.question = "O1".into();
        store.add_questions("Diğer", "Konu", vec![other]).unwrap();
        assert_eq!(store.recent_questions("Ders", 2).unwrap(), ["S3", "S2"]);
        assert!(store.recent_questions("Yok", 5).unwrap().is_empty());
    }

    #[test]
    fn listener_restart_closes_only_its_own_session() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let listener = store.start_session("Ders", "Telegram", &now()).unwrap();
        let tui = store.start_session("Ders", "TUI", &now()).unwrap();
        store
            .set_state("listener_session", &listener.to_string())
            .unwrap();
        store.close_interrupted_sessions().unwrap();
        let ended = |id| {
            store
                .conn
                .query_row("SELECT ended_at FROM sessions WHERE id=?1", [id], |row| {
                    row.get::<_, Option<String>>(0)
                })
                .unwrap()
        };
        assert!(ended(listener).is_some());
        assert!(ended(tui).is_none());
    }

    #[test]
    fn delete_deneme_removes_only_its_window_answers_and_orphaned_questions() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let questions: Vec<Question> = ["Deneme 1", "Deneme 2", "Deneme 3", "Başka"]
            .into_iter()
            .map(|text| Question {
                question: text.into(),
                ..question()
            })
            .collect();
        let ids: Vec<i64> = store
            .add_exam_questions("Ders", "Deneme", questions)
            .unwrap()
            .iter()
            .map(|q| q.id)
            .collect();
        let session = store
            .start_session_with_kind("Ders", "Deneme Sınavı", "exam", "2026-01-01T10:00:00+00:00")
            .unwrap();
        let other = store
            .start_session("Ders", "Quiz", "2026-01-01T10:00:00+00:00")
            .unwrap();
        store
            .set_state(&format!("deneme_total:{session}"), "3")
            .unwrap();
        store.set_deneme_questions(session, &ids[..3]).unwrap();
        // Pencere içi cevaplar silinir; pencere dışı, başka oturum ve başka soru kalır.
        store
            .record_answer_at(session, ids[0], 1, at("2026-01-01T10:30:00Z"))
            .unwrap();
        store
            .record_answer_at(session, ids[1], 1, at("2026-01-01T09:00:00Z"))
            .unwrap();
        store
            .record_answer_at(session, ids[1], 1, at("2026-01-01T10:30:00Z"))
            .unwrap();
        store
            .record_answer_at(session, ids[2], 1, at("2026-01-01T12:00:00Z"))
            .unwrap();
        store
            .record_answer_at(other, ids[3], 1, at("2026-01-01T10:30:00Z"))
            .unwrap();
        for id in [ids[0], ids[1]] {
            store
                .conn
                .execute(
                    "INSERT OR REPLACE INTO reviews(question_id,box,due_at) VALUES(?1,1,'2026-01-02T00:00:00+00:00')",
                    [id],
                )
                .unwrap();
        }

        store
            .delete_deneme(
                session,
                &ids[..3],
                at("2026-01-01T10:00:00Z"),
                at("2026-01-01T11:00:00Z"),
            )
            .unwrap();

        let count = |sql: &str| -> i64 { store.conn.query_row(sql, [], |row| row.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM questions"), 3);
        assert_eq!(count("SELECT COUNT(*) FROM answers"), 3);
        assert_eq!(count("SELECT COUNT(*) FROM sessions"), 1);
        assert!(review(&store, ids[0]).is_none());
        assert!(review(&store, ids[1]).is_some());
        assert_eq!(
            store.state(&format!("deneme_total:{session}")).unwrap(),
            None
        );
        assert_eq!(store.deneme_question_ids(session).unwrap(), None);
    }

    #[test]
    fn answers_are_per_attempt_and_session_scores_are_atomic() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let questions = store
            .add_questions("Ders", "Konu", vec![question(), question()])
            .unwrap();
        assert_eq!(store.question_count("Ders").unwrap(), 2);
        assert_eq!(store.unanswered("Ders").unwrap().len(), 2);
        let session = store.start_session("Ders", "Konu", &now()).unwrap();
        assert!(store.record_answer(session, questions[0].id, 1).unwrap());
        assert!(!store.record_answer(session, questions[0].id, 0).unwrap());
        assert!(store.record_answer(session, questions[1].id, 5).is_err());
        assert_eq!(store.unanswered("Ders").unwrap().len(), 1);
        store.end_session(session).unwrap();
        assert!(store.record_answer(session, questions[1].id, 1).is_err());
        let score: (i64, i64, String) = store
            .conn
            .query_row(
                "SELECT correct,total,ended_at FROM sessions WHERE id=?1",
                [session],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((score.0, score.1), (1, 2));
        let attempts: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM answers", [], |row| row.get(0))
            .unwrap();
        assert_eq!(attempts, 2);
    }

    #[test]
    fn review_boxes_follow_intervals_and_graduate_after_box_five() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let id = store
            .add_questions("Ders", "Konu", vec![question()])
            .unwrap()[0]
            .id;
        let session = store.start_session("Ders", "Konu", "2028-01-31").unwrap();
        let initial = at("2028-01-31T23:59:59Z");
        assert!(store.record_answer_at(session, id, 1, initial).unwrap());
        assert_eq!(review(&store, id), None);
        assert!(!store.record_answer_at(session, id, 0, initial).unwrap());
        assert_eq!(review(&store, id), Some((1, "2028-02-01".into())));
        for (box_number, answered_at, expected_due) in [
            (2, "2028-02-01T12:00:00Z", "2028-02-03"),
            (3, "2028-02-03T12:00:00Z", "2028-02-07"),
            (4, "2028-02-07T12:00:00Z", "2028-02-15"),
            (5, "2028-02-15T12:00:00Z", "2028-03-02"),
        ] {
            assert!(
                store
                    .record_answer_at(session, id, 1, at(answered_at))
                    .unwrap()
            );
            assert_eq!(review(&store, id), Some((box_number, expected_due.into())));
        }
        let final_at = at("2028-03-02T00:00:00Z");
        assert!(store.record_answer_at(session, id, 1, final_at).unwrap());
        assert_eq!(review(&store, id), None);
        assert!(store.record_answer_at(session, id, 1, final_at).unwrap());
        assert_eq!(review(&store, id), None);
    }

    #[test]
    fn wrong_answers_reset_every_box_and_failed_answers_leave_reviews_unchanged() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let id = store
            .add_questions("Ders", "Konu", vec![question()])
            .unwrap()[0]
            .id;
        let session = store.start_session("Ders", "Konu", "2027-12-31").unwrap();
        let initial = at("2027-12-31T23:59:59Z");
        for target_box in 1..=5 {
            assert!(!store.record_answer_at(session, id, 0, initial).unwrap());
            for _ in 1..target_box {
                assert!(store.record_answer_at(session, id, 1, initial).unwrap());
            }
            assert_eq!(review(&store, id).unwrap().0, target_box);
            assert!(!store.record_answer_at(session, id, 0, initial).unwrap());
            assert_eq!(review(&store, id), Some((1, "2028-01-01".into())));
        }
        let score = || {
            store
                .conn
                .query_row(
                    "SELECT correct,total FROM sessions WHERE id=?1",
                    [session],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap()
        };
        let before = score();
        let attempts: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM answers", [], |row| row.get(0))
            .unwrap();
        assert!(
            store
                .record_answer_at(session, id, 0, DateTime::<Utc>::MAX_UTC)
                .is_err()
        );
        let after: (i64, i64) = store
            .conn
            .query_row(
                "SELECT correct,total FROM sessions WHERE id=?1",
                [session],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(
            store
                .conn
                .query_row("SELECT COUNT(*) FROM answers", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            attempts
        );
        assert!(store.record_answer_at(session, id, 5, initial).is_err());
        store.end_session(session).unwrap();
        assert!(store.record_answer_at(session, id, 1, initial).is_err());
        assert_eq!(review(&store, id), Some((1, "2028-01-01".into())));
    }

    #[test]
    fn due_reviews_order_limit_counts_and_next_date_are_subject_specific() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let questions = store
            .add_questions("Ders", "Konu", vec![question(); 4])
            .unwrap();
        let other = store
            .add_questions("Diğer", "Konu", vec![question()])
            .unwrap()[0]
            .id;
        let session = store.start_session("Ders", "Konu", "2028-01-01").unwrap();
        for (index, answered_at) in [
            (0, "2028-02-28T23:59:59Z"),
            (1, "2028-01-31T00:00:00Z"),
            (2, "2028-01-31T01:00:00Z"),
            (3, "2028-02-29T00:00:00Z"),
        ] {
            store
                .record_answer_at(session, questions[index].id, 0, at(answered_at))
                .unwrap();
        }
        let other_session = store.start_session("Diğer", "Konu", "2028-01-01").unwrap();
        store
            .record_answer_at(other_session, other, 0, at("2028-01-01T00:00:00Z"))
            .unwrap();
        let today = at("2028-02-29T00:00:00Z");
        let due = store.due_reviews("Ders", 10, today).unwrap();
        assert_eq!(
            due.iter().map(|q| q.id).collect::<Vec<_>>(),
            vec![questions[1].id, questions[2].id, questions[0].id]
        );
        assert_eq!(
            store.due_reviews("Ders", 1, today).unwrap()[0].id,
            questions[1].id
        );
        assert!(store.due_reviews("Ders", 0, today).unwrap().is_empty());
        assert!(
            store
                .due_reviews("Ders", 10, at("2028-01-31T23:59:59Z"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.review_counts("Ders", today).unwrap(), (3, 4));
        assert_eq!(store.review_counts("Diğer", today).unwrap(), (1, 1));
        assert_eq!(store.review_counts("Boş", today).unwrap(), (0, 0));
        assert_eq!(
            store.next_review("Ders").unwrap(),
            Some("2028-02-01".into())
        );
        assert_eq!(store.next_review("Boş").unwrap(), None);
    }

    #[test]
    fn record_answer_with_review_advances_or_resets_target_box() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let orig = store
            .add_questions("Ders", "Konu", vec![question()])
            .unwrap()[0]
            .id;
        let var1 = store
            .add_questions("Ders", "Konu", vec![question()])
            .unwrap()[0]
            .id;
        let session = store.start_session("Ders", "Konu", "2028-01-01").unwrap();
        // Initially fail original question so it enters Box 1 (due 1 day later)
        store
            .record_answer_at(session, orig, 0, at("2028-01-01T00:00:00Z"))
            .unwrap();
        assert_eq!(review(&store, orig), Some((1, "2028-01-02".into())));

        // Answering variation correctly (answer_index is 1, so choice 1 is correct)
        // should advance orig to Box 2 (2 days later)
        let session2 = store.start_session("Ders", "Konu", "2028-01-02").unwrap();
        let correct = store
            .record_answer_with_review_at(session2, var1, 1, orig, at("2028-01-02T00:00:00Z"))
            .unwrap();
        assert!(correct);
        assert_eq!(review(&store, orig), Some((2, "2028-01-04".into())));

        // Answering another variation incorrectly should reset orig back to Box 1 (1 day later)
        let var2 = store
            .add_questions("Ders", "Konu", vec![question()])
            .unwrap()[0]
            .id;
        let session3 = store.start_session("Ders", "Konu", "2028-01-04").unwrap();
        let correct = store
            .record_answer_with_review_at(session3, var2, 0, orig, at("2028-01-04T00:00:00Z"))
            .unwrap();
        assert!(!correct);
        assert_eq!(review(&store, orig), Some((1, "2028-01-05".into())));
    }

    #[test]
    fn deneme_sessions_queries_only_finished_denemes() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let q = store
            .add_exam_questions("İngilizce", "Deneme Sınavı", vec![question()])
            .unwrap()[0]
            .id;

        // Regular quiz session
        let s_regular = store
            .start_session("İngilizce", "Gramer", "2028-01-01T10:00:00Z")
            .unwrap();
        store
            .record_answer_at(s_regular, q, 1, at("2028-01-01T10:01:00Z"))
            .unwrap();
        store.end_session(s_regular).unwrap();

        // Deneme session 1 (finished)
        let s_deneme1 = store
            .start_session_with_kind("İngilizce", "Deneme Sınavı", "exam", "2028-01-02T10:00:00Z")
            .unwrap();
        store
            .record_answer_at(s_deneme1, q, 1, at("2028-01-02T10:01:00Z"))
            .unwrap();
        store.end_session(s_deneme1).unwrap();

        // Deneme session 2 (not ended / active)
        let s_deneme2 = store
            .start_session_with_kind("İngilizce", "Deneme Sınavı", "exam", "2028-01-03T10:00:00Z")
            .unwrap();
        store
            .record_answer_at(s_deneme2, q, 1, at("2028-01-03T10:01:00Z"))
            .unwrap();

        let denemes = store.deneme_sessions("İngilizce", 10).unwrap();
        assert_eq!(denemes.len(), 1);
        assert_eq!(denemes[0].topic, "Deneme Sınavı");
        assert_eq!(denemes[0].correct, 1);
        assert_eq!(denemes[0].total, 1);
    }

    #[test]
    fn spec_store_1_wal_mode_and_idempotent_legacy_migration() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("legacy.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE state(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE history(id INTEGER PRIMARY KEY, subject TEXT NOT NULL, role TEXT NOT NULL, text TEXT NOT NULL, at TEXT NOT NULL);
                 CREATE TABLE questions(id INTEGER PRIMARY KEY, subject TEXT NOT NULL, topic TEXT NOT NULL, question TEXT NOT NULL, choices_json TEXT NOT NULL, answer_index INTEGER NOT NULL CHECK(answer_index BETWEEN 0 AND 4), explanation TEXT NOT NULL, source TEXT NOT NULL, created_at TEXT NOT NULL);
                 CREATE TABLE answers(id INTEGER PRIMARY KEY, question_id INTEGER NOT NULL REFERENCES questions(id), choice INTEGER NOT NULL CHECK(choice BETWEEN 0 AND 4), correct INTEGER NOT NULL, at TEXT NOT NULL);
                 CREATE TABLE sessions(id INTEGER PRIMARY KEY, subject TEXT NOT NULL, topic TEXT NOT NULL, started_at TEXT NOT NULL, ended_at TEXT, correct INTEGER NOT NULL DEFAULT 0, total INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE reviews(question_id INTEGER PRIMARY KEY REFERENCES questions(id), box INTEGER NOT NULL CHECK(box BETWEEN 1 AND 5), due_at TEXT NOT NULL);
                 INSERT INTO questions VALUES(1,'english','Sample Deneme 1','Q1','[\"a\",\"b\",\"c\",\"d\"]',1,'e','s','2026-01-01T00:00:00Z');
                 INSERT INTO questions VALUES(2,'english','Gramer','Q2','[\"a\",\"b\",\"c\",\"d\"]',1,'e','s','2026-01-01T00:00:00Z');
                 INSERT INTO sessions VALUES(1,'english','Sample Deneme 1','2026-01-01T00:00:00Z','2026-01-01T00:10:00Z',1,1);
                 INSERT INTO sessions VALUES(2,'english','Gramer','2026-01-01T00:00:00Z','2026-01-01T00:05:00Z',1,1);",
            )
            .unwrap();
        }
        let store = Store::open(&db).unwrap();
        assert!(store.quick_check().unwrap());
        let mode: String = store
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        let kind_exam: String = store
            .conn
            .query_row("SELECT kind FROM sessions WHERE id=1", [], |r| r.get(0))
            .unwrap();
        let kind_quiz: String = store
            .conn
            .query_row("SELECT kind FROM sessions WHERE id=2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kind_exam, "exam");
        assert_eq!(kind_quiz, "quiz");
        let q1_exam: i64 = store
            .conn
            .query_row("SELECT is_exam FROM questions WHERE id=1", [], |r| r.get(0))
            .unwrap();
        let q2_exam: i64 = store
            .conn
            .query_row("SELECT is_exam FROM questions WHERE id=2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(q1_exam, 1);
        assert_eq!(q2_exam, 0);
        drop(store);
        let reopened = Store::open(&db).unwrap();
        assert!(reopened.quick_check().unwrap());
    }

    #[test]
    fn spec_store_2_english_mock_exam_and_subject_scoping() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let eq = store
            .add_exam_questions("math", "Mock Exam 1", vec![question()])
            .unwrap()[0]
            .id;
        let math_exam = store
            .start_session_with_kind("math", "Mock Exam 1", "exam", "2026-05-01T10:00:00Z")
            .unwrap();
        store
            .record_answer_at(math_exam, eq, 0, at("2026-05-01T10:01:00Z"))
            .unwrap();
        store.end_session(math_exam).unwrap();

        let yq = store
            .add_exam_questions("english", "English Mock", vec![question()])
            .unwrap()[0]
            .id;
        let eng_exam = store
            .start_session_with_kind("english", "English Mock", "exam", "2026-05-01T11:00:00Z")
            .unwrap();
        store
            .record_answer_at(eng_exam, yq, 0, at("2026-05-01T11:01:00Z"))
            .unwrap();
        store.end_session(eng_exam).unwrap();

        assert!(
            store
                .due_original_reviews("math", 10, at("2026-05-10T00:00:00Z"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.next_original_review("math").unwrap(), None);

        let math_denemes = store.deneme_sessions("math", 10).unwrap();
        assert_eq!(math_denemes.len(), 1);
        assert_eq!(math_denemes[0].topic, "Mock Exam 1");
        assert!(store.topic_totals("math").unwrap().is_empty());
        assert!(store.weakest_topics("math", 10).unwrap().is_empty());
    }

    #[test]
    fn spec_store_3_lang_scoped_lesson_cache_and_4_choice_validation() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let qs = store
            .add_questions("math", "integrals", vec![question()])
            .unwrap();
        let qid = qs[0].id;
        store
            .save_lesson(
                "math",
                crate::i18n::Lang::Tr,
                "integrals",
                "Türkçe ders",
                &qs,
                &[],
            )
            .unwrap();
        assert!(
            store
                .load_lesson("math", crate::i18n::Lang::Tr, "integrals")
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .load_lesson("math", crate::i18n::Lang::En, "integrals")
                .unwrap()
                .is_none()
        );

        let sess = store
            .start_session("math", "integrals", "2026-05-01T10:00:00Z")
            .unwrap();
        assert!(
            store
                .record_answer_at(sess, qid, 4, at("2026-05-01T10:01:00Z"))
                .is_err()
        );
        assert!(
            store
                .record_answer_at(sess, qid, 1, at("2026-05-01T10:01:00Z"))
                .unwrap()
        );
    }

    #[test]
    fn spec_sec_store_permissions_concurrent_open_and_deneme_answers_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let db_dir = dir.path().join("priv_db");
        let db_path = db_dir.join("legacy_concurrent.db");
        std::fs::create_dir_all(&db_dir).unwrap();
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE state(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE history(id INTEGER PRIMARY KEY, subject TEXT NOT NULL, role TEXT NOT NULL, text TEXT NOT NULL, at TEXT NOT NULL);
                 CREATE TABLE questions(id INTEGER PRIMARY KEY, subject TEXT NOT NULL, topic TEXT NOT NULL, question TEXT NOT NULL, choices_json TEXT NOT NULL, answer_index INTEGER NOT NULL CHECK(answer_index BETWEEN 0 AND 4), explanation TEXT NOT NULL, source TEXT NOT NULL, created_at TEXT NOT NULL);
                 CREATE TABLE answers(id INTEGER PRIMARY KEY, question_id INTEGER NOT NULL REFERENCES questions(id), choice INTEGER NOT NULL CHECK(choice BETWEEN 0 AND 4), correct INTEGER NOT NULL, at TEXT NOT NULL);
                 CREATE TABLE sessions(id INTEGER PRIMARY KEY, subject TEXT NOT NULL, topic TEXT NOT NULL, started_at TEXT NOT NULL, ended_at TEXT, correct INTEGER NOT NULL DEFAULT 0, total INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE reviews(question_id INTEGER PRIMARY KEY REFERENCES questions(id), box INTEGER NOT NULL CHECK(box BETWEEN 1 AND 5), due_at TEXT NOT NULL);",
            )
            .unwrap();
        }

        let mut handles = Vec::new();
        for _ in 0..4 {
            let p = db_path.clone();
            handles.push(std::thread::spawn(move || Store::open(&p).map(|_| ())));
        }
        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&db_dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        let store = Store::open(Path::new(":memory:")).unwrap();
        store
            .conn
            .execute_batch(
                "INSERT INTO questions(id,subject,topic,question,choices_json,answer_index,explanation,source,created_at,is_exam) VALUES(1,'D','Deneme','Q?','[\"a\",\"b\",\"c\",\"d\"]',4,'e','s','2026-01-01T00:00:00Z',1);
                 INSERT INTO answers(id,question_id,choice,correct,at) VALUES(1,1,1,0,'2026-01-01T00:01:00Z');",
            )
            .unwrap();
        assert!(
            store
                .deneme_answers(
                    1,
                    "Deneme",
                    at("2026-01-01T00:00:00Z"),
                    at("2026-01-01T01:00:00Z")
                )
                .is_err()
        );
    }

    fn new_flashcard(tag: &str, front: &str, high_yield: bool) -> NewFlashcard {
        NewFlashcard {
            subject: "yds".into(),
            tag: tag.into(),
            status: CardStatus::Active,
            card_type: CardKind::Qa,
            context_prefix: "[YDS - Grammar]".into(),
            front: front.into(),
            back: "**anahtar** cevap".into(),
            extra_context: "Açıklama".into(),
            mnemonic_hint: None,
            is_high_yield: high_yield,
            source_question_id: None,
        }
    }

    #[test]
    fn flashcard_queue_respects_quota_backlog_threshold_and_descending_retrievability() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let t0 = at("2026-05-10T10:00:00Z");

        // 25 new cards -> daily new quota caps at 20, prioritizing high-yield
        let mut batch = Vec::new();
        for i in 0..25 {
            batch.push(new_flashcard(
                if i % 2 == 0 {
                    "#YDS::Grammar::Passive"
                } else {
                    "#YDS::Vocabulary::Verbs"
                },
                &format!("Card {i}?"),
                i == 24,
            ));
        }
        let saved = store.add_flashcards(batch, t0).unwrap();
        let q = store.due_flashcards_queue("yds", None, false, t0).unwrap();
        assert_eq!(q.new_available, 20);
        assert!(!q.backlog_paused);
        assert_eq!(q.cards[0].id, saved[24].id); // high-yield first

        // Tag hierarchy prefix filtering
        let grammar_q = store
            .due_flashcards_queue("yds", Some("#YDS::Grammar"), false, t0)
            .unwrap();
        assert!(
            grammar_q
                .cards
                .iter()
                .all(|c| c.tag.starts_with("#YDS::Grammar"))
        );

        // Review 1 new card -> remaining daily quota becomes 19
        store
            .record_flashcard_review(saved[0].id, BinaryRating::Good, 3200, t0)
            .unwrap();
        let q_after = store.due_flashcards_queue("yds", None, false, t0).unwrap();
        assert_eq!(q_after.new_today, 1);
        assert_eq!(q_after.new_available, 19);

        // Create 2 due Review cards with different elapsed times -> higher R comes first
        let r_cards = store
            .add_flashcards(
                vec![
                    new_flashcard("#YDS::Test", "Staler?", false),
                    new_flashcard("#YDS::Test", "Fresher?", false),
                ],
                t0,
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE flashcards SET state=2, stability=10.0, stability_fast=10.0, difficulty=5.0, last_review=?1, due=?2 WHERE id=?3",
                params![
                    (t0 - chrono::Duration::days(15)).to_rfc3339(),
                    (t0 - chrono::Duration::days(1)).to_rfc3339(),
                    r_cards[0].id
                ],
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE flashcards SET state=2, stability=10.0, stability_fast=10.0, difficulty=5.0, last_review=?1, due=?2 WHERE id=?3",
                params![
                    (t0 - chrono::Duration::days(3)).to_rfc3339(),
                    (t0 - chrono::Duration::hours(2)).to_rfc3339(),
                    r_cards[1].id
                ],
            )
            .unwrap();
        let ordered = store.due_flashcards_queue("yds", None, false, t0).unwrap();
        assert_eq!(ordered.due_reviews, 2);
        assert_eq!(ordered.cards[0].id, r_cards[1].id);
        assert_eq!(ordered.cards[1].id, r_cards[0].id);

        // Backlog protection: >= 150 due reviews pauses new cards
        let extra: Vec<NewFlashcard> = (0..148)
            .map(|i| new_flashcard("#YDS::Backlog", &format!("B{i}"), false))
            .collect();
        let extra_ids = store.add_flashcards(extra, t0).unwrap();
        for c in extra_ids {
            store
                .conn
                .execute(
                    "UPDATE flashcards SET state=2, stability=5.0, stability_fast=5.0, difficulty=5.0, last_review=?1, due=?2 WHERE id=?3",
                    params![
                        (t0 - chrono::Duration::days(6)).to_rfc3339(),
                        (t0 - chrono::Duration::days(1)).to_rfc3339(),
                        c.id
                    ],
                )
                .unwrap();
        }
        let backlog_q = store.due_flashcards_queue("yds", None, false, t0).unwrap();
        assert!(backlog_q.backlog_paused);
        assert_eq!(backlog_q.due_reviews, 150);
        assert_eq!(backlog_q.new_available, 0);
    }

    #[test]
    fn incubation_strategy_notes_and_leech_repair_roundtrip() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let t0 = at("2026-05-10T10:00:00Z");

        let mut inc = new_flashcard("#YDS::Clauses", "Yetim zamirsiz soru?", true);
        inc.status = CardStatus::Incubation;
        let saved = store.add_flashcards(vec![inc], t0).unwrap();
        assert_eq!(store.incubation_cards("yds").unwrap().len(), 1);
        assert!(
            store
                .due_flashcards_queue("yds", None, false, t0)
                .unwrap()
                .cards
                .is_empty()
        );

        assert!(store.approve_incubation_card(saved[0].id, t0).unwrap());
        assert!(store.incubation_cards("yds").unwrap().is_empty());
        assert_eq!(
            store
                .due_flashcards_queue("yds", None, false, t0)
                .unwrap()
                .cards
                .len(),
            1
        );

        // Strategy note stays out of flashcards queue
        store
            .add_exam_technique_note("yds", None, "#YDS::Strategy", "Öncülleri sondan oku.", t0)
            .unwrap();
        assert_eq!(store.exam_technique_notes("yds", 10).unwrap().len(), 1);

        // Leech flag & repair
        store
            .conn
            .execute(
                "UPDATE flashcards SET is_leech=1, lapses=3 WHERE id=?1",
                [saved[0].id],
            )
            .unwrap();
        assert!(
            store
                .apply_leech_repair(
                    saved[0].id,
                    &LeechRepair {
                        card_type: CardKind::Contrastive,
                        context_prefix: "[YDS - Contrast]",
                        front: "Although vs Despite farkı nedir?",
                        back: "**Although + cümle**, **Despite + isim** alır.",
                        extra_context: "Bağlaç vs edat ayrımı.",
                        mnemonic_hint: Some("Although = All-clause, Despite = D-isim"),
                    },
                )
                .unwrap()
        );
        let repaired = store.flashcard(saved[0].id).unwrap().unwrap();
        assert!(!repaired.is_leech);
        assert_eq!(repaired.card_type, CardKind::Contrastive);
        assert_eq!(repaired.lapses, 3);
    }

    #[test]
    fn spec_store_native_lang_and_translation_cache() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("studia.db")).unwrap();

        assert_eq!(store.native_lang(crate::i18n::Lang::Tr).unwrap(), "Türkçe");
        assert_eq!(store.native_lang(crate::i18n::Lang::En).unwrap(), "English");

        assert!(store.set_native_lang("   ").is_err());
        assert_eq!(store.set_native_lang("  deutsch ").unwrap(), "deutsch");
        assert_eq!(store.native_lang(crate::i18n::Lang::Tr).unwrap(), "deutsch");

        let src = "Correct answer: 'however'. It introduces contrast.";
        assert_eq!(store.cached_translation("Türkçe", src).unwrap(), None);
        store
            .save_translation("Türkçe", src, "Doğru cevap: 'however'. Zıtlık bildirir.")
            .unwrap();
        assert_eq!(
            store.cached_translation("tr", src).unwrap().as_deref(),
            Some("Doğru cevap: 'however'. Zıtlık bildirir.")
        );
        assert_eq!(store.cached_translation("Deutsch", src).unwrap(), None);
    }
}
