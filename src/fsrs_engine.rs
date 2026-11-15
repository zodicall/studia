use crate::{Result, i18n::Lang, invalid};
use chrono::{DateTime, Duration, Utc};
use fsrs::{FSRS, MemoryState};

/// Target retention (0.88–0.90 band reduces review workload by ~30–40% vs 0.95).
pub const DEFAULT_RETENTION: f32 = 0.88;
/// Intra-day learning steps in minutes: 10m -> 30m.
pub const LEARNING_STEPS_MINS: [i64; 2] = [10, 30];
/// Intra-day relearning step in minutes: 10m.
pub const RELEARNING_STEPS_MINS: [i64; 1] = [10];
/// Default daily new card quota (1 new card ~= 10 daily reviews long term).
pub const DAILY_NEW_CARD_LIMIT: usize = 20;
/// Auto-pause new cards when due reviews reach or exceed this threshold.
pub const BACKLOG_PROTECTION_THRESHOLD: usize = 150;
/// Leech threshold: lapses hitting 3, 6, 9, ...
pub const LEECH_LAPSE_INTERVAL: i64 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum CardState {
    New = 0,
    Learning = 1,
    Review = 2,
    Relearning = 3,
}

impl CardState {
    pub fn from_i64(value: i64) -> Result<Self> {
        match value {
            0 => Ok(Self::New),
            1 => Ok(Self::Learning),
            2 => Ok(Self::Review),
            3 => Ok(Self::Relearning),
            _ => Err(invalid(format!("Invalid FSRS card state: {value}"))),
        }
    }

    pub fn as_i64(self) -> i64 {
        self as i64
    }

    pub fn label(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::New, Lang::Tr) => "Yeni",
            (Self::New, Lang::En) => "New",
            (Self::Learning, Lang::Tr) => "Öğrenme",
            (Self::Learning, Lang::En) => "Learning",
            (Self::Review, Lang::Tr) => "Tekrar",
            (Self::Review, Lang::En) => "Review",
            (Self::Relearning, Lang::Tr) => "Yeniden Öğrenme",
            (Self::Relearning, Lang::En) => "Relearning",
        }
    }
}

/// 2-button Pass/Fail discipline to eliminate decision fatigue and FSRS RMSE drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum BinaryRating {
    Again = 1,
    Good = 3,
}

impl BinaryRating {
    pub fn from_i64(value: i64) -> Result<Self> {
        match value {
            1 => Ok(Self::Again),
            3 => Ok(Self::Good),
            _ => Err(invalid(format!("Invalid 2-button rating: {value}"))),
        }
    }

    pub fn as_i64(self) -> i64 {
        self as i64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    Qa,
    Cloze,
    Contrastive,
}

impl CardKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Qa => "qa",
            Self::Cloze => "cloze",
            Self::Contrastive => "contrastive",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "qa" => Ok(Self::Qa),
            "cloze" => Ok(Self::Cloze),
            "contrastive" => Ok(Self::Contrastive),
            other => Err(invalid(format!("Invalid flashcard type: {other}"))),
        }
    }

    /// Speed Focus threshold in seconds: 6s for cloze, 15s for conceptual Q&A / contrastive.
    pub fn speed_focus_limit_secs(self) -> u64 {
        match self {
            Self::Cloze => 6,
            Self::Qa | Self::Contrastive => 15,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardStatus {
    Active,
    Incubation,
    Suspended,
}

impl CardStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Incubation => "incubation",
            Self::Suspended => "suspended",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "active" => Ok(Self::Active),
            "incubation" => Ok(Self::Incubation),
            "suspended" => Ok(Self::Suspended),
            other => Err(invalid(format!("Invalid flashcard status: {other}"))),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Flashcard {
    pub id: i64,
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
    pub is_leech: bool,
    pub source_question_id: Option<i64>,
    pub state: CardState,
    pub stability: f64,
    pub stability_fast: f64,
    pub difficulty: f64,
    pub elapsed_days: f64,
    pub scheduled_days: i64,
    pub learning_step: u8,
    pub reps: i64,
    pub lapses: i64,
    pub last_review: Option<DateTime<Utc>>,
    pub due: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScheduleOutcome {
    pub updated: Flashcard,
    pub became_leech: bool,
    pub interval_label_tr: String,
    pub interval_label_en: String,
}

fn build_fsrs(weights: Option<&[f32]>) -> Result<FSRS> {
    match weights {
        Some(w) if !w.is_empty() => FSRS::new(w)
            .map_err(|err| invalid(format!("FSRS model error: {err}"))),
        _ => Ok(FSRS::default()),
    }
}

fn memory_state(card: &Flashcard) -> Option<MemoryState> {
    if card.state == CardState::New || card.stability <= 0.0 || card.difficulty <= 0.0 {
        None
    } else {
        Some(MemoryState {
            stability: card.stability as f32,
            difficulty: card.difficulty as f32,
            stability_fast: if card.stability_fast > 0.0 {
                card.stability_fast as f32
            } else {
                card.stability as f32
            },
        })
    }
}

pub fn elapsed_days_since(card: &Flashcard, now: DateTime<Utc>) -> f64 {
    match card.last_review {
        Some(last) => ((now - last).num_seconds().max(0) as f64) / 86_400.0,
        None => 0.0,
    }
}

/// Instantaneous probability of recall R(t) in [0.0, 1.0].
pub fn retrievability(card: &Flashcard, now: DateTime<Utc>, weights: Option<&[f32]>) -> f64 {
    match card.state {
        CardState::New => 0.0,
        CardState::Learning | CardState::Relearning => 1.0,
        CardState::Review => {
            let Some(mem) = memory_state(card) else {
                return 0.0;
            };
            let Ok(fsrs) = build_fsrs(weights) else {
                return 0.0;
            };
            let elapsed = elapsed_days_since(card, now) as f32;
            f64::from(fsrs.current_retrievability(mem, elapsed)).clamp(0.0, 1.0)
        }
    }
}

/// Computes the next FSRS + intraday learning state for a 2-button review.
pub fn schedule_card(
    card: &Flashcard,
    rating: BinaryRating,
    now: DateTime<Utc>,
    weights: Option<&[f32]>,
) -> Result<ScheduleOutcome> {
    let fsrs = build_fsrs(weights)?;
    let elapsed = elapsed_days_since(card, now);
    let current_mem = memory_state(card);
    let next_states = fsrs
        .next_states_with_elapsed_days(current_mem, DEFAULT_RETENTION, elapsed as f32)
        .map_err(|err| invalid(format!("FSRS scheduling failed: {err}")))?;

    let mut updated = card.clone();
    updated.elapsed_days = elapsed;
    updated.reps += 1;
    updated.last_review = Some(now);

    let mut became_leech = false;
    let (label_tr, label_en): (String, String);

    match (card.state, rating) {
        (CardState::New, BinaryRating::Again) => {
            updated.state = CardState::Learning;
            updated.learning_step = 0;
            updated.scheduled_days = 0;
            updated.due = now + Duration::minutes(LEARNING_STEPS_MINS[0]);
            apply_memory(&mut updated, next_states.again.memory);
            label_tr = format!("{} dk", LEARNING_STEPS_MINS[0]);
            label_en = format!("{}m", LEARNING_STEPS_MINS[0]);
        }
        (CardState::New, BinaryRating::Good) => {
            updated.state = CardState::Learning;
            let step = usize::from(LEARNING_STEPS_MINS.len() > 1);
            updated.learning_step = step as u8;
            updated.scheduled_days = 0;
            let mins = LEARNING_STEPS_MINS[step];
            updated.due = now + Duration::minutes(mins);
            apply_memory(&mut updated, next_states.good.memory);
            label_tr = format!("{mins} dk");
            label_en = format!("{mins}m");
        }
        (CardState::Learning, BinaryRating::Again) => {
            updated.state = CardState::Learning;
            updated.learning_step = 0;
            updated.scheduled_days = 0;
            updated.due = now + Duration::minutes(LEARNING_STEPS_MINS[0]);
            apply_memory(&mut updated, next_states.again.memory);
            label_tr = format!("{} dk", LEARNING_STEPS_MINS[0]);
            label_en = format!("{}m", LEARNING_STEPS_MINS[0]);
        }
        (CardState::Learning, BinaryRating::Good) => {
            let next_step = usize::from(card.learning_step) + 1;
            if next_step < LEARNING_STEPS_MINS.len() {
                let mins = LEARNING_STEPS_MINS[next_step];
                updated.state = CardState::Learning;
                updated.learning_step = next_step as u8;
                updated.scheduled_days = 0;
                updated.due = now + Duration::minutes(mins);
                apply_memory(&mut updated, next_states.good.memory);
                label_tr = format!("{mins} dk");
                label_en = format!("{mins}m");
            } else {
                let days = (next_states.good.interval.round() as i64).max(1);
                updated.state = CardState::Review;
                updated.learning_step = 0;
                updated.scheduled_days = days;
                updated.due = now + Duration::days(days);
                apply_memory(&mut updated, next_states.good.memory);
                label_tr = format!("{days} gün");
                label_en = format!("{days}d");
            }
        }
        (CardState::Review, BinaryRating::Again) => {
            updated.lapses += 1;
            if updated.lapses >= LEECH_LAPSE_INTERVAL
                && updated.lapses % LEECH_LAPSE_INTERVAL == 0
            {
                updated.is_leech = true;
                became_leech = true;
            }
            updated.state = CardState::Relearning;
            updated.learning_step = 0;
            updated.scheduled_days = 0;
            updated.due = now + Duration::minutes(RELEARNING_STEPS_MINS[0]);
            apply_memory(&mut updated, next_states.again.memory);
            label_tr = format!("{} dk", RELEARNING_STEPS_MINS[0]);
            label_en = format!("{}m", RELEARNING_STEPS_MINS[0]);
        }
        (CardState::Review, BinaryRating::Good) => {
            let days = (next_states.good.interval.round() as i64).max(1);
            updated.state = CardState::Review;
            updated.learning_step = 0;
            updated.scheduled_days = days;
            updated.due = now + Duration::days(days);
            apply_memory(&mut updated, next_states.good.memory);
            label_tr = format!("{days} gün");
            label_en = format!("{days}d");
        }
        (CardState::Relearning, BinaryRating::Again) => {
            updated.state = CardState::Relearning;
            updated.learning_step = 0;
            updated.scheduled_days = 0;
            updated.due = now + Duration::minutes(RELEARNING_STEPS_MINS[0]);
            apply_memory(&mut updated, next_states.again.memory);
            label_tr = format!("{} dk", RELEARNING_STEPS_MINS[0]);
            label_en = format!("{}m", RELEARNING_STEPS_MINS[0]);
        }
        (CardState::Relearning, BinaryRating::Good) => {
            let next_step = usize::from(card.learning_step) + 1;
            if next_step < RELEARNING_STEPS_MINS.len() {
                let mins = RELEARNING_STEPS_MINS[next_step];
                updated.state = CardState::Relearning;
                updated.learning_step = next_step as u8;
                updated.scheduled_days = 0;
                updated.due = now + Duration::minutes(mins);
                apply_memory(&mut updated, next_states.good.memory);
                label_tr = format!("{mins} dk");
                label_en = format!("{mins}m");
            } else {
                let days = (next_states.good.interval.round() as i64).max(1);
                updated.state = CardState::Review;
                updated.learning_step = 0;
                updated.scheduled_days = days;
                updated.due = now + Duration::days(days);
                apply_memory(&mut updated, next_states.good.memory);
                label_tr = format!("{days} gün");
                label_en = format!("{days}d");
            }
        }
    }

    Ok(ScheduleOutcome {
        updated,
        became_leech,
        interval_label_tr: label_tr,
        interval_label_en: label_en,
    })
}

fn apply_memory(card: &mut Flashcard, mem: MemoryState) {
    card.stability = f64::from(mem.stability);
    card.stability_fast = f64::from(mem.stability_fast);
    card.difficulty = f64::from(mem.difficulty).clamp(1.0, 10.0);
}

/// Returns `(again_label, good_label)` for the 2-button UI in the requested language.
pub fn preview_ratings(
    card: &Flashcard,
    now: DateTime<Utc>,
    lang: Lang,
) -> (String, String) {
    let again = schedule_card(card, BinaryRating::Again, now, None)
        .map(|o| match lang {
            Lang::Tr => o.interval_label_tr,
            Lang::En => o.interval_label_en,
        })
        .unwrap_or_else(|_| "10m".into());
    let good = schedule_card(card, BinaryRating::Good, now, None)
        .map(|o| match lang {
            Lang::Tr => o.interval_label_tr,
            Lang::En => o.interval_label_en,
        })
        .unwrap_or_else(|_| "1d".into());
    (again, good)
}

/// Masks `{{c1::answer}}` spans on the front face for `cloze` cards, or reveals them cleanly.
pub fn format_cloze_front(text: &str) -> String {
    render_cloze(text, false)
}

pub fn format_cloze_back(text: &str) -> String {
    render_cloze(text, true)
}

fn render_cloze(text: &str, reveal: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        let Some(end) = after_open.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &after_open[..end];
        let content = inner
            .split_once("::")
            .map(|(_, body)| body)
            .unwrap_or(inner);
        let (answer, hint) = content
            .split_once("::")
            .map(|(a, h)| (a.trim(), Some(h.trim())))
            .unwrap_or((content.trim(), None));
        if reveal {
            out.push_str(&format!("**{answer}**"));
        } else if let Some(h) = hint.filter(|h| !h.is_empty()) {
            out.push_str(&format!("[{h}…]"));
        } else {
            out.push_str("[ … ]");
        }
        rest = &after_open[end + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_card(state: CardState) -> Flashcard {
        let now = Utc::now();
        Flashcard {
            id: 1,
            subject: "yds".into(),
            tag: "#YDS::Vocabulary::Verbs".into(),
            status: CardStatus::Active,
            card_type: CardKind::Cloze,
            context_prefix: "[YDS - Academic Verbs]".into(),
            front: "The results {{c1::corroborate}} the initial hypothesis.".into(),
            back: "**corroborate** = kanıtla **desteklemek**".into(),
            extra_context: "Teoriyi yeni bulgularla sağlamlaştırmak anlamında kullanılır.".into(),
            mnemonic_hint: Some("co- (birlikte) + roborare (güçlendirmek)".into()),
            is_high_yield: true,
            is_leech: false,
            source_question_id: None,
            state,
            stability: if state == CardState::Review { 5.0 } else { 0.0 },
            stability_fast: if state == CardState::Review { 5.0 } else { 0.0 },
            difficulty: if state == CardState::Review { 5.0 } else { 0.0 },
            elapsed_days: 0.0,
            scheduled_days: if state == CardState::Review { 5 } else { 0 },
            learning_step: 0,
            reps: 0,
            lapses: 0,
            last_review: if state == CardState::Review {
                Some(now - Duration::days(5))
            } else {
                None
            },
            due: now,
            created_at: now,
        }
    }

    #[test]
    fn new_card_follows_intraday_learning_steps_then_graduates_to_fsrs_review() {
        let t0 = Utc::now();
        let card = sample_card(CardState::New);

        // First Good -> Learning step 1 (30m)
        let s1 = schedule_card(&card, BinaryRating::Good, t0, None).unwrap();
        assert_eq!(s1.updated.state, CardState::Learning);
        assert_eq!(s1.updated.learning_step, 1);
        assert_eq!((s1.updated.due - t0).num_minutes(), 30);
        assert!(s1.updated.stability > 0.0);
        assert!((1.0..=10.0).contains(&s1.updated.difficulty));

        // Again resets to step 0 (10m)
        let t1 = t0 + Duration::minutes(30);
        let s_again = schedule_card(&s1.updated, BinaryRating::Again, t1, None).unwrap();
        assert_eq!(s_again.updated.state, CardState::Learning);
        assert_eq!(s_again.updated.learning_step, 0);
        assert_eq!((s_again.updated.due - t1).num_minutes(), 10);

        // Good from step 0 -> step 1 (30m) -> Good from step 1 -> Graduates to Review
        let t2 = t1 + Duration::minutes(10);
        let s2 = schedule_card(&s_again.updated, BinaryRating::Good, t2, None).unwrap();
        assert_eq!(s2.updated.state, CardState::Learning);
        assert_eq!(s2.updated.learning_step, 1);

        let t3 = t2 + Duration::minutes(30);
        let s3 = schedule_card(&s2.updated, BinaryRating::Good, t3, None).unwrap();
        assert_eq!(s3.updated.state, CardState::Review);
        assert!(s3.updated.scheduled_days >= 1);
        assert_eq!((s3.updated.due - t3).num_days(), s3.updated.scheduled_days);
    }

    #[test]
    fn review_again_increments_lapses_and_flags_leech_at_multiples_of_three() {
        let now = Utc::now();
        let mut card = sample_card(CardState::Review);
        for expected_lapses in 1..=6 {
            card.state = CardState::Review;
            card.last_review = Some(now - Duration::days(3));
            let outcome = schedule_card(&card, BinaryRating::Again, now, None).unwrap();
            assert_eq!(outcome.updated.lapses, expected_lapses);
            assert_eq!(outcome.updated.state, CardState::Relearning);
            assert_eq!(outcome.became_leech, expected_lapses % 3 == 0);
            card = outcome.updated;
            card.is_leech = false; // simulate repair before next cycle
        }
    }

    #[test]
    fn retrievability_decreases_over_elapsed_time_for_descending_queue() {
        let now = Utc::now();
        let mut fresher = sample_card(CardState::Review);
        fresher.stability = 10.0;
        fresher.last_review = Some(now - Duration::days(2));

        let mut staler = sample_card(CardState::Review);
        staler.stability = 10.0;
        staler.last_review = Some(now - Duration::days(15));

        let r_fresh = retrievability(&fresher, now, None);
        let r_stale = retrievability(&staler, now, None);
        assert!(r_fresh > r_stale);
        assert!(r_fresh > 0.8 && r_stale < 0.9);
    }

    #[test]
    fn cloze_masks_front_and_bolds_back() {
        let raw = "The {{c1::mitochondria::organelle}} produces {{c2::ATP}}.";
        assert_eq!(
            format_cloze_front(raw),
            "The [organelle…] produces [ … ]."
        );
        assert_eq!(
            format_cloze_back(raw),
            "The **mitochondria** produces **ATP**."
        );
    }
}
