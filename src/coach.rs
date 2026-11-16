//! Yapay zeka koçu: ders yapılandırmasını (`course.json`) ve çalışma planını izler.
//! Plan varsa bugünün bekleyen görevleri arasından seçer; plan yoksa istatistiklere göre öneri verir.

use crate::{
    Result,
    course::{CourseConfig, DayConfig},
    i18n::Lang,
    plan, sources,
    store::Store,
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Bekleyen tekrar sayısı bu eşiğe ulaşınca tekrar görevi öne alınır.
pub const REVIEW_THRESHOLD: i64 = 5;
/// Bir konunun doğruluğu bu yüzdenin altındaysa o konunun görevi öne alınır.
pub const WEAK_PERCENT: f64 = 70.0;
/// Bir konunun zayıf sayılması için gereken en az cevap sayısı.
pub const MIN_ANSWERS: i64 = 5;

/// TUI'de çalıştırılabilen plan komutları (Türkçe ve İngilizce takma adlarıyla).
const RUNNABLE: [&str; 11] = [
    "/tekrar",
    "/review",
    "/anlat",
    "/lesson",
    "/kazanim",
    "/practice",
    "/quiz",
    "/kelime",
    "/cards",
    "/deneme",
    "/exam",
];

/// AI yanıtının beklenen biçimi.
pub const SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"properties":{"index":{"type":"integer","minimum":0},"reason":{"type":"string","minLength":1,"maxLength":300}},"required":["index","reason"]}"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanTask {
    pub index: usize,
    pub label: String,
    pub command: String,
    pub done: bool,
}

impl PlanTask {
    pub fn runnable(&self) -> bool {
        let name = self.command.split_whitespace().next().unwrap_or("");
        RUNNABLE.contains(&name)
    }

    /// Koçun seçebileceği görev: çalıştırılabilir ve henüz bitmemiş.
    pub fn coachable(&self) -> bool {
        !self.done && self.runnable()
    }
}

/// Plandaki yer: plansız, başlamamış, beklemede, tamamlanmış ya da aktif gün.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Program {
    NoPlan,
    NotStarted,
    Upcoming(String),
    Finished,
    Day {
        day: usize,
        total_days: usize,
        tasks: Vec<PlanTask>,
    },
}

/// Koçun seçtiği bir plan görevi.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub day: usize,
    pub task: usize,
    pub command: String,
    pub label: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Topic {
    pub name: String,
    pub taught: bool,
    pub correct: i64,
    pub total: i64,
}

impl Topic {
    pub fn percent(&self) -> Option<f64> {
        (self.total > 0).then(|| self.correct as f64 * 100.0 / self.total as f64)
    }
}

/// Koçun karar verirken gördüğü durum.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub subject: String,
    pub course_title: String,
    pub persona: Option<String>,
    pub program: Program,
    pub due: i64,
    pub topics: Vec<Topic>,
    pub recent: Vec<String>,
}

impl Snapshot {
    pub fn day(&self) -> Option<usize> {
        match self.program {
            Program::Day { day, .. } => Some(day),
            _ => None,
        }
    }

    /// Bugün koçun seçebileceği görevler, plan sırasıyla.
    pub fn coachable_tasks(&self) -> Vec<&PlanTask> {
        match &self.program {
            Program::Day { tasks, .. } => tasks.iter().filter(|task| task.coachable()).collect(),
            _ => vec![],
        }
    }

    pub fn has_coachable(&self) -> bool {
        !self.coachable_tasks().is_empty()
    }
}

/// Dersin planındaki `/anlat` veya `/lesson` konuları (plan sırasıyla, tekilleştirilmiş).
pub fn lesson_topics(course: &CourseConfig) -> Vec<String> {
    let mut topics: Vec<String> = Vec::new();
    for task in course.plan.iter().flat_map(|day| day.tasks.iter()) {
        let raw = task
            .command
            .strip_prefix("/anlat ")
            .or_else(|| task.command.strip_prefix("/lesson "));
        if let Some(topic) = raw {
            let topic = topic.trim().to_string();
            if !topic.is_empty() && !topics.contains(&topic) {
                topics.push(topic);
            }
        }
    }
    topics
}

/// Bir konunun anlatıldığını işaretleyen durum anahtarı.
pub fn taught_key(subject: &str, topic: &str) -> String {
    format!("coach:taught:{subject}:{topic}")
}

fn collect_topics_and_recent(
    store: &Store,
    subject: &str,
    course: &CourseConfig,
) -> Result<(Vec<Topic>, Vec<String>)> {
    let totals = store.topic_totals(subject)?;
    let mut names = lesson_topics(course);
    for (topic, _, _) in &totals {
        if !names.iter().any(|existing| topic.contains(existing)) && !names.contains(topic) {
            names.push(topic.clone());
        }
    }
    let mut topics = Vec::with_capacity(names.len());
    for name in names {
        let (correct, total) = totals
            .iter()
            .filter(|(topic, _, _)| topic.contains(&name))
            .fold((0, 0), |(c, t), (_, dc, dt)| (c + dc, t + dt));
        let taught = store.state(&taught_key(subject, &name))?.is_some();
        topics.push(Topic {
            name,
            taught,
            correct,
            total,
        });
    }
    let recent = store
        .recent_sessions()?
        .into_iter()
        .filter(|session| session.subject == subject)
        .take(5)
        .map(|session| format!("{}: {}/{}", session.topic, session.correct, session.total))
        .collect();
    Ok((topics, recent))
}

/// Bir dersin koç için durum özeti. `today` planın hangi gününde olunduğunu belirler.
pub fn snapshot(
    store: &Store,
    subject: &str,
    course: &CourseConfig,
    due: i64,
    today: NaiveDate,
) -> Result<Snapshot> {
    let total_days = course.plan.len();
    let program = if course.plan.is_empty() {
        Program::NoPlan
    } else {
        match plan::status(store, subject, &course.plan, today)? {
            plan::Status::NotStarted => Program::NotStarted,
            plan::Status::Upcoming { start } => {
                Program::Upcoming(start.format("%d.%m.%Y").to_string())
            }
            plan::Status::Finished { .. } => Program::Finished,
            plan::Status::Active { day, done, .. } => {
                let tasks = plan::day(&course.plan, day)
                    .map(|info| {
                        info.tasks
                            .iter()
                            .enumerate()
                            .map(|(index, task)| PlanTask {
                                index,
                                label: task.label.clone(),
                                command: task.command.clone(),
                                done: done.get(index).copied().unwrap_or(false),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Program::Day {
                    day,
                    total_days,
                    tasks,
                }
            }
        }
    };

    let (topics, recent) = collect_topics_and_recent(store, subject, course)?;
    Ok(Snapshot {
        subject: subject.to_string(),
        course_title: course.display_title(subject).to_string(),
        persona: course.persona.clone(),
        program,
        due,
        topics,
        recent,
    })
}

/// Plan güncelleme kontrolünün neden tetiklendiği.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdaptTrigger {
    /// Bugünün çalıştırılabilir görevleri tamamlandı (gün sonu kontrolü).
    EndOfDay,
    /// Yeni gün başladı ve bir önceki günde eksik görevler var.
    Carryover {
        missed_day: usize,
        missed_labels: Vec<String>,
    },
}

impl AdaptTrigger {
    pub fn kind_key(&self) -> &'static str {
        match self {
            Self::EndOfDay => "eod",
            Self::Carryover { .. } => "carryover",
        }
    }
}

/// Plan düzenleme kontrolü için AI'ye gönderilen durum özeti.
#[derive(Clone, Debug, PartialEq)]
pub struct AdaptContext {
    pub subject: String,
    pub course_title: String,
    pub persona: Option<String>,
    pub start: NaiveDate,
    pub current_day: usize,
    pub total_days: usize,
    pub editable_from_day: usize,
    pub trigger: AdaptTrigger,
    pub done_today_labels: Vec<String>,
    pub remaining_plan: Vec<DayConfig>,
    pub due: i64,
    pub topics: Vec<Topic>,
    pub recent: Vec<String>,
    pub catalog: Vec<String>,
}

/// Kullanıcının onayına sunulan plan değişikliği önerisi.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlanProposal {
    pub subject: String,
    #[serde(with = "date_serde")]
    pub start: NaiveDate,
    pub editable_from_day: usize,
    pub summary: String,
    pub adapted_days: Vec<DayConfig>,
    pub trigger_kind: String,
}

mod date_serde {
    use chrono::NaiveDate;
    use serde::{Deserialize, Deserializer, Serializer, de};

    pub fn serialize<S: Serializer>(date: &NaiveDate, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&date.format("%Y-%m-%d").to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<NaiveDate, D::Error> {
        let raw = String::deserialize(d)?;
        NaiveDate::parse_from_str(&raw, "%Y-%m-%d").map_err(de::Error::custom)
    }
}

/// Aynı gün aynı kontrolün yinelenmesini önleyen durum anahtarı.
pub fn adapt_checked_key(subject: &str, start: NaiveDate, day: usize, kind: &str) -> String {
    format!(
        "plan_adapt_checked:{subject}:{}:{day}:{kind}",
        start.format("%Y-%m-%d")
    )
}

pub fn mark_adapt_checked(store: &Store, ctx: &AdaptContext) -> Result<()> {
    let key = adapt_checked_key(
        &ctx.subject,
        ctx.start,
        ctx.current_day,
        ctx.trigger.kind_key(),
    );
    store.set_state(&key, "1")
}

/// Plan elle düzenlendiğinde veya yeniden oluşturulduğunda eski kontrol/öneri durumunu temizler.
pub fn clear_adapt_state(store: &Store, subject: &str) -> Result<()> {
    store.clear_state(&format!("plan_proposal:{subject}"))?;
    store.clear_state_prefix(&format!("plan_adapt_checked:{subject}:"))
}

fn is_runnable_cmd(command: &str) -> bool {
    let name = command.split_whitespace().next().unwrap_or("");
    RUNNABLE.contains(&name)
}

/// Gün sonu (bugünün çalıştırılabilir görevleri bitti) veya ertesi gün eksik görevi kontrol eder.
pub fn detect_adapt_trigger(
    store: &Store,
    root: &Path,
    subject: &str,
    course: &CourseConfig,
    due: i64,
    today: NaiveDate,
) -> Result<Option<AdaptContext>> {
    if course.plan.is_empty() {
        return Ok(None);
    }
    let plan::Status::Active { start, day, done } =
        plan::status(store, subject, &course.plan, today)?
    else {
        return Ok(None);
    };
    let Some(today_cfg) = plan::day(&course.plan, day) else {
        return Ok(None);
    };
    let total_days = course.plan.len();
    let any_done_today = done.iter().any(|d| *d);

    if day > 1
        && store
            .state(&adapt_checked_key(subject, start, day, "carryover"))?
            .is_none()
        && let Some(prev_cfg) = plan::day(&course.plan, day - 1)
    {
        let prev_done = plan::day_done(store, subject, &course.plan, start, day - 1)?;
        let missed: Vec<String> = prev_cfg
            .tasks
            .iter()
            .enumerate()
            .filter(|(idx, _)| !prev_done.get(*idx).copied().unwrap_or(false))
            .map(|(_, task)| task.label.clone())
            .collect();
        let editable_from = if any_done_today { day + 1 } else { day };
        if !missed.is_empty() && editable_from <= plan::MAX_PLAN_DAYS {
            return build_adapt_context(
                store,
                root,
                subject,
                course,
                due,
                start,
                day,
                total_days,
                editable_from,
                AdaptTrigger::Carryover {
                    missed_day: day - 1,
                    missed_labels: missed,
                },
                today_cfg,
                &done,
            )
            .map(Some);
        }
    }

    check_eod_trigger(
        store,
        root,
        subject,
        course,
        due,
        start,
        day,
        total_days,
        today_cfg,
        &done,
    )
}

#[allow(clippy::too_many_arguments)]
fn check_eod_trigger(
    store: &Store,
    root: &Path,
    subject: &str,
    course: &CourseConfig,
    due: i64,
    start: NaiveDate,
    day: usize,
    total_days: usize,
    today_cfg: &DayConfig,
    done: &[bool],
) -> Result<Option<AdaptContext>> {
    if day >= plan::MAX_PLAN_DAYS {
        return Ok(None);
    }
    if store
        .state(&adapt_checked_key(subject, start, day, "eod"))?
        .is_some()
    {
        return Ok(None);
    }
    let any_done_today = done.iter().any(|d| *d);
    if !any_done_today {
        return Ok(None);
    }
    let pending_runnable = today_cfg
        .tasks
        .iter()
        .enumerate()
        .any(|(idx, t)| !done.get(idx).copied().unwrap_or(false) && is_runnable_cmd(&t.command));
    if pending_runnable {
        return Ok(None);
    }
    build_adapt_context(
        store,
        root,
        subject,
        course,
        due,
        start,
        day,
        total_days,
        day + 1,
        AdaptTrigger::EndOfDay,
        today_cfg,
        done,
    )
    .map(Some)
}

#[allow(clippy::too_many_arguments)]
fn build_adapt_context(
    store: &Store,
    root: &Path,
    subject: &str,
    course: &CourseConfig,
    due: i64,
    start: NaiveDate,
    current_day: usize,
    total_days: usize,
    editable_from_day: usize,
    trigger: AdaptTrigger,
    today_cfg: &DayConfig,
    done: &[bool],
) -> Result<AdaptContext> {
    let (topics, recent) = collect_topics_and_recent(store, subject, course)?;
    let catalog = sources::catalog(root, subject, 40).unwrap_or_default();
    let done_today_labels = today_cfg
        .tasks
        .iter()
        .enumerate()
        .filter(|(idx, _)| done.get(*idx).copied().unwrap_or(false))
        .map(|(_, t)| t.label.clone())
        .collect();
    let remaining_plan = course
        .plan
        .get(editable_from_day.saturating_sub(1)..)
        .unwrap_or(&[])
        .to_vec();
    Ok(AdaptContext {
        subject: subject.to_string(),
        course_title: course.display_title(subject).to_string(),
        persona: course.persona.clone(),
        start,
        current_day,
        total_days,
        editable_from_day,
        trigger,
        done_today_labels,
        remaining_plan,
        due,
        topics,
        recent,
        catalog,
    })
}

/// Önerilen plan değişikliğinin kullanıcıya gösterilecek kısa önizleme satırları.
pub fn proposal_preview(
    course: &CourseConfig,
    proposal: &PlanProposal,
    lang: Lang,
) -> Vec<String> {
    proposal
        .adapted_days
        .iter()
        .enumerate()
        .take(5)
        .map(|(idx, new_day)| {
            let day_num = proposal.editable_from_day + idx;
            let tag = match day_num.checked_sub(1).and_then(|i| course.plan.get(i)) {
                Some(old) if old.title == new_day.title && old.tasks == new_day.tasks => {
                    lang.pick("korundu", "unchanged")
                }
                Some(_) => lang.pick("güncellendi", "updated"),
                None => lang.pick("eklendi", "added"),
            };
            let tasks: Vec<&str> = new_day.tasks.iter().map(|t| t.label.as_str()).collect();
            format!(
                "{} {day_num} ({tag}): {} — {}",
                lang.pick("Gün", "Day"),
                new_day.title,
                tasks.join(", ")
            )
        })
        .collect()
}

/// AI'ye giden karar istemi: bugünün görevleri, durum ve kurallar.
pub fn prompt(snapshot: &Snapshot, lang: Lang) -> String {
    let mut text = match lang {
        Lang::Tr => {
            let role = snapshot
                .persona
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    format!(
                        "'{}' dersi için yardımcı bir çalışma koçusun",
                        snapshot.course_title
                    )
                });
            format!(
                "Sen {role}. Öğrenci kendi çalışma planını izliyor. \
                 Plan bağlayıcıdır: yalnızca aşağıdaki bugünkü görevler arasından seç, plan dışı görev uydurma.\n\n"
            )
        }
        Lang::En => {
            let role = snapshot
                .persona
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    format!("a helpful study coach for '{}'", snapshot.course_title)
                });
            format!(
                "You are {role}. The student is following their study plan. \
                 Follow the plan strictly: pick only from today's tasks listed below and do not invent tasks.\n\n"
            )
        }
    };

    if let Program::Day {
        day,
        total_days,
        tasks,
    } = &snapshot.program
    {
        match lang {
            Lang::Tr => text.push_str(&format!("Bugün plan günü {day}/{total_days}. Görevler:\n")),
            Lang::En => text.push_str(&format!("Today is plan day {day}/{total_days}. Tasks:\n")),
        }
        for task in tasks {
            let status = match lang {
                Lang::Tr => {
                    if task.done {
                        "tamam"
                    } else if task.runnable() {
                        "bekliyor"
                    } else {
                        "elle yapılır"
                    }
                }
                Lang::En => {
                    if task.done {
                        "done"
                    } else if task.runnable() {
                        "pending"
                    } else {
                        "manual"
                    }
                }
            };
            text.push_str(&format!(
                "[{}] {status}: {} ({})\n",
                task.index, task.label, task.command
            ));
        }
    }

    match lang {
        Lang::Tr => text.push_str(&format!(
            "\nBekleyen tekrar sorusu: {}\nKonu durumu:\n",
            snapshot.due
        )),
        Lang::En => text.push_str(&format!(
            "\nDue review questions: {}\nTopic stats:\n",
            snapshot.due
        )),
    }
    for topic in &snapshot.topics {
        match lang {
            Lang::Tr => {
                let state = if topic.taught {
                    "anlatıldı"
                } else {
                    "anlatılmadı"
                };
                let score = match topic.percent() {
                    Some(percent) => {
                        format!("kazanım {}/{} (%{percent:.0})", topic.correct, topic.total)
                    }
                    None => "henüz test yok".into(),
                };
                text.push_str(&format!("- {}: {state}, {score}\n", topic.name));
            }
            Lang::En => {
                let state = if topic.taught {
                    "covered"
                } else {
                    "not covered"
                };
                let score = match topic.percent() {
                    Some(percent) => {
                        format!("score {}/{} ({percent:.0}%)", topic.correct, topic.total)
                    }
                    None => "no quiz yet".into(),
                };
                text.push_str(&format!("- {}: {state}, {score}\n", topic.name));
            }
        }
    }
    if !snapshot.recent.is_empty() {
        match lang {
            Lang::Tr => text.push_str(&format!("Son oturumlar: {}\n", snapshot.recent.join("; "))),
            Lang::En => text.push_str(&format!(
                "Recent sessions: {}\n",
                snapshot.recent.join("; ")
            )),
        }
    }
    match lang {
        Lang::Tr => text.push_str(&format!(
            "\nKURALLAR:\n\
             1. index yalnızca [bekliyor] olan bir görevin numarası olsun.\n\
             2. Varsayılan: listedeki ilk [bekliyor] görev.\n\
             3. Bekleyen tekrar {REVIEW_THRESHOLD} veya daha fazlaysa ve bir tekrar görevi [bekliyor] ise onu seç.\n\
             4. Doğruluğu %{WEAK_PERCENT:.0} altında olan (en az {MIN_ANSWERS} cevap) bir konunun görevi [bekliyor] ise onu öne al.\n\
             5. reason: seçimi Türkçe, en fazla 200 karakterle açıkla; öğrenciye doğrudan hitap et.\n\
             Yalnızca şu biçimde JSON döndür: {{\"index\": <sayı>, \"reason\": \"<metin>\"}}"
        )),
        Lang::En => text.push_str(&format!(
            "\nRULES:\n\
             1. index MUST be the number of a [pending] task.\n\
             2. Default: the first [pending] task in the list.\n\
             3. If due reviews >= {REVIEW_THRESHOLD} and a review task is [pending], choose it.\n\
             4. If a topic below {WEAK_PERCENT:.0}% accuracy (with at least {MIN_ANSWERS} answers) has a [pending] task, prioritize it.\n\
             5. reason: explain in English in <=200 chars; address the student directly.\n\
             Return JSON only: {{\"index\": <number>, \"reason\": \"<text>\"}}"
        )),
    }
    text
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    index: usize,
    reason: String,
}

fn step_for(day: usize, task: &PlanTask, reason: String) -> Step {
    Step {
        day,
        task: task.index,
        command: task.command.clone(),
        label: task.label.clone(),
        reason,
    }
}

/// AI yanıtını doğrular: seçilen görev bugünün bekleyen ve çalıştırılabilir bir görevi olmalı.
pub fn parse(raw: &str, snapshot: &Snapshot) -> Option<Step> {
    let decision: Decision = serde_json::from_str(raw).ok()?;
    let reason = decision.reason.trim();
    if reason.is_empty() {
        return None;
    }
    let day = snapshot.day()?;
    let task = snapshot
        .coachable_tasks()
        .into_iter()
        .find(|task| task.index == decision.index)?;
    Some(step_for(day, task, reason.into()))
}

fn is_review_cmd(cmd: &str) -> bool {
    matches!(cmd.split_whitespace().next(), Some("/tekrar" | "/review"))
}

/// AI yanıtı geçersizse veya yoksa yerel kurallarla karar. Plan veya çalıştırılacak görev yoksa `None`.
pub fn rule_step(snapshot: &Snapshot, lang: Lang) -> Option<Step> {
    let day = snapshot.day()?;
    let tasks = snapshot.coachable_tasks();
    if snapshot.due >= REVIEW_THRESHOLD
        && let Some(review) = tasks.iter().find(|task| is_review_cmd(&task.command))
    {
        let reason = match lang {
            Lang::Tr => format!("{} soru tekrar zamanı geldi.", snapshot.due),
            Lang::En => format!("{} questions are due for review.", snapshot.due),
        };
        return Some(step_for(day, review, reason));
    }
    let first = tasks.first()?;
    let reason = match lang {
        Lang::Tr => "Plan sırasındaki görev.".into(),
        Lang::En => "Next task in plan order.".into(),
    };
    Some(step_for(day, first, reason))
}

/// Karar verilemediğinde gösterilecek açıklama.
pub fn idle_message(snapshot: &Snapshot, lang: Lang) -> String {
    match &snapshot.program {
        Program::NoPlan => {
            let weak = snapshot
                .topics
                .iter()
                .filter(|t| t.total >= MIN_ANSWERS && t.percent().is_some_and(|p| p < WEAK_PERCENT))
                .map(|t| t.name.as_str())
                .next();
            match lang {
                Lang::Tr => {
                    if snapshot.due >= REVIEW_THRESHOLD {
                        format!(
                            "Plan tanımlı değil; {} tekrar sorusu bekliyor (/review kullanın).",
                            snapshot.due
                        )
                    } else if let Some(topic) = weak {
                        format!(
                            "Plan tanımlı değil; zayıf konu '{topic}' için /quiz çalıştırabilirsiniz."
                        )
                    } else {
                        "Bu ders için plan tanımlı değil (isteğe bağlı: course.json). /lesson, /quiz veya /review ile çalışabilirsiniz."
                            .into()
                    }
                }
                Lang::En => {
                    if snapshot.due >= REVIEW_THRESHOLD {
                        format!(
                            "No study plan configured; {} review questions due (run /review).",
                            snapshot.due
                        )
                    } else if let Some(topic) = weak {
                        format!("No study plan configured; try /quiz on weak topic '{topic}'.")
                    } else {
                        "No study plan configured (optional course.json). Study anytime with /lesson, /quiz, or /review."
                            .into()
                    }
                }
            }
        }
        Program::NotStarted => match lang {
            Lang::Tr => "Plan başlatılmadı.".into(),
            Lang::En => "Study plan not started.".into(),
        },
        Program::Upcoming(date) => match lang {
            Lang::Tr => format!("Plan {date} tarihinde başlayacak."),
            Lang::En => format!("Study plan starts on {date}."),
        },
        Program::Finished => match lang {
            Lang::Tr => "Plan tamamlandı.".into(),
            Lang::En => "Study plan finished.".into(),
        },
        Program::Day { tasks, .. } => {
            let manual: Vec<&str> = tasks
                .iter()
                .filter(|task| !task.done && !task.runnable())
                .map(|task| task.label.as_str())
                .collect();
            match lang {
                Lang::Tr => {
                    if manual.is_empty() {
                        "Bugünün tüm görevleri tamam.".into()
                    } else {
                        format!(
                            "Koçun görevleri tamam. Elle kalan: {}. Bugün sekmesinde Enter ile işaretleyin.",
                            manual.join("; ")
                        )
                    }
                }
                Lang::En => {
                    if manual.is_empty() {
                        "All tasks for today are complete.".into()
                    } else {
                        format!(
                            "Coach tasks complete. Manual remaining: {}. Press Enter in Today tab to mark.",
                            manual.join("; ")
                        )
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::course::{DayConfig, TaskConfig};
    use std::path::Path;

    fn task(index: usize, command: &str, done: bool) -> PlanTask {
        PlanTask {
            index,
            label: format!("task {index}"),
            command: command.into(),
            done,
        }
    }

    fn day_snapshot(day: usize, tasks: Vec<PlanTask>) -> Snapshot {
        Snapshot {
            subject: "Demo".into(),
            course_title: "Demo Course".into(),
            persona: None,
            program: Program::Day {
                day,
                total_days: 3,
                tasks,
            },
            due: 0,
            topics: vec![],
            recent: vec![],
        }
    }

    fn sample_course() -> CourseConfig {
        CourseConfig {
            title: Some("Rust 101".into()),
            persona: None,
            exam: None,
            plan: vec![
                DayConfig {
                    day: 1,
                    title: "Ownership".into(),
                    tasks: vec![
                        TaskConfig {
                            command: "/review".into(),
                            label: "Review".into(),
                        },
                        TaskConfig {
                            command: "/lesson Ownership".into(),
                            label: "Ownership".into(),
                        },
                    ],
                },
                DayConfig {
                    day: 2,
                    title: "Lifetimes".into(),
                    tasks: vec![TaskConfig {
                        command: "/anlat Lifetimes".into(),
                        label: "Lifetimes".into(),
                    }],
                },
            ],
        }
    }

    #[test]
    fn plan_lesson_topics_are_unique_and_follow_course_plan_order() {
        let mut course = sample_course();
        course.plan[1].tasks.push(TaskConfig {
            command: "/lesson Ownership".into(),
            label: "Duplicate".into(),
        });
        assert_eq!(lesson_topics(&course), vec!["Ownership", "Lifetimes"]);
    }

    #[test]
    fn spec_coach_1_2_no_plan_bilingual_prompts_and_cards_runnable() {
        assert!(task(0, "/kelime", false).runnable());
        assert!(task(1, "/cards", false).runnable());

        let store = Store::open(Path::new(":memory:")).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        let empty = CourseConfig::default();
        let snap = snapshot(&store, "Rust101", &empty, 6, today).unwrap();
        assert_eq!(snap.program, Program::NoPlan);
        assert!(rule_step(&snap, Lang::En).is_none());
        assert!(idle_message(&snap, Lang::En).contains("/review"));
        assert!(idle_message(&snap, Lang::Tr).contains("/review"));

        let day_snap = day_snapshot(1, vec![task(0, "/cards vocab 10", false)]);
        let en_prompt = prompt(&day_snap, Lang::En);
        assert!(!en_prompt.contains("hardcoded_exam"));
        assert!(en_prompt.contains("Demo Course"));
    }

    #[test]
    fn coach_only_picks_pending_runnable_tasks_of_today() {
        let snap = day_snapshot(
            2,
            vec![
                task(0, "/tekrar", true),
                task(1, "/anlat Ownership", false),
                task(2, "/cards vocab 30", false),
                task(3, "", false),
                task(4, "/quiz basics 5", false),
            ],
        );
        let pending: Vec<usize> = snap.coachable_tasks().iter().map(|t| t.index).collect();
        assert_eq!(pending, vec![1, 2, 4]);
        assert_eq!(rule_step(&snap, Lang::Tr).unwrap().task, 1);
    }

    #[test]
    fn rule_puts_review_first_only_when_enough_work_is_due() {
        let mut snap = day_snapshot(
            2,
            vec![
                task(0, "/anlat Ownership", false),
                task(1, "/review", false),
            ],
        );
        assert_eq!(rule_step(&snap, Lang::Tr).unwrap().task, 0);
        snap.due = REVIEW_THRESHOLD;
        assert_eq!(rule_step(&snap, Lang::Tr).unwrap().task, 1);
    }

    #[test]
    fn parse_accepts_only_pending_runnable_tasks_of_today() {
        let snap = day_snapshot(
            2,
            vec![
                task(0, "/tekrar", true),
                task(1, "/anlat Ownership", false),
                task(2, "", false),
            ],
        );
        let ok = parse(r#"{"index":1,"reason":"Next topic."}"#, &snap).unwrap();
        assert_eq!((ok.day, ok.task), (2, 1));
        assert!(parse(r#"{"index":0,"reason":"x"}"#, &snap).is_none());
        assert!(parse(r#"{"index":2,"reason":"x"}"#, &snap).is_none());
        assert!(parse(r#"{"index":9,"reason":"x"}"#, &snap).is_none());
        assert!(parse(r#"{"index":1,"reason":"  "}"#, &snap).is_none());
        assert!(parse("not json", &snap).is_none());
        assert!(parse(r#"{"index":1,"reason":"x","extra":1}"#, &snap).is_none());
    }

    #[test]
    fn snapshot_follows_the_plan_day_and_its_done_flags() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        let course = sample_course();
        assert_eq!(
            snapshot(&store, "rust", &course, 0, today).unwrap().program,
            Program::NotStarted
        );
        plan::set_start(&store, "rust", today).unwrap();
        plan::mark_done(&store, "rust", today, 1, 0).unwrap();
        let snap = snapshot(&store, "rust", &course, 0, today).unwrap();
        let Program::Day {
            day,
            total_days,
            tasks,
        } = &snap.program
        else {
            panic!("day 1 should be active");
        };
        assert_eq!((*day, *total_days), (1, 2));
        assert!(tasks[0].done);
        assert!(!tasks[1].done);
    }

    #[test]
    fn idle_message_names_manual_tasks_left_for_the_user() {
        let snap = day_snapshot(1, vec![task(0, "/tekrar", true), task(1, "", false)]);
        assert!(idle_message(&snap, Lang::Tr).contains("Elle kalan: task 1"));
        assert!(idle_message(&snap, Lang::En).contains("Manual remaining: task 1"));
        let done = day_snapshot(1, vec![task(0, "/tekrar", true)]);
        assert!(idle_message(&done, Lang::Tr).contains("tüm görevleri tamam"));
        assert!(idle_message(&done, Lang::En).contains("complete"));
    }

    #[test]
    fn detect_adapt_trigger_fires_end_of_day_once_when_runnable_tasks_done() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("studia.db")).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        let course = sample_course();
        plan::set_start(&store, "rust", today).unwrap();

        // No tasks done yet -> should not trigger EOD.
        assert!(
            detect_adapt_trigger(&store, dir.path(), "rust", &course, 0, today)
                .unwrap()
                .is_none()
        );

        // One runnable task still pending -> should not trigger EOD.
        plan::mark_done(&store, "rust", today, 1, 0).unwrap();
        assert!(
            detect_adapt_trigger(&store, dir.path(), "rust", &course, 0, today)
                .unwrap()
                .is_none()
        );

        // All runnable tasks of day 1 done -> fires EOD once.
        plan::mark_done(&store, "rust", today, 1, 1).unwrap();
        let ctx = detect_adapt_trigger(&store, dir.path(), "rust", &course, 3, today)
            .unwrap()
            .expect("EOD trigger should fire");
        assert_eq!(ctx.trigger, AdaptTrigger::EndOfDay);
        assert_eq!(ctx.current_day, 1);
        assert_eq!(ctx.editable_from_day, 2);
        assert_eq!(ctx.remaining_plan.len(), 1);

        mark_adapt_checked(&store, &ctx).unwrap();
        assert!(
            detect_adapt_trigger(&store, dir.path(), "rust", &course, 3, today)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn detect_adapt_trigger_fires_carryover_when_yesterday_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("studia.db")).unwrap();
        let start = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        let day2 = NaiveDate::from_ymd_opt(2026, 11, 6).unwrap();
        let course = sample_course();
        plan::set_start(&store, "rust", start).unwrap();
        // Only task 0 of day 1 was finished; task 1 ("Ownership") was missed.
        plan::mark_done(&store, "rust", start, 1, 0).unwrap();

        let ctx = detect_adapt_trigger(&store, dir.path(), "rust", &course, 0, day2)
            .unwrap()
            .expect("Carryover should fire on day 2");
        assert_eq!(
            ctx.trigger,
            AdaptTrigger::Carryover {
                missed_day: 1,
                missed_labels: vec!["Ownership".into()],
            }
        );
        assert_eq!(ctx.editable_from_day, 2);
        mark_adapt_checked(&store, &ctx).unwrap();
        assert!(
            detect_adapt_trigger(&store, dir.path(), "rust", &course, 0, day2)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn proposal_preview_marks_updated_and_added_days() {
        let course = sample_course();
        let proposal = PlanProposal {
            subject: "rust".into(),
            start: NaiveDate::from_ymd_opt(2026, 11, 5).unwrap(),
            editable_from_day: 2,
            summary: "Extra practice".into(),
            adapted_days: vec![
                DayConfig {
                    day: 2,
                    title: "Ownership Review".into(),
                    tasks: vec![TaskConfig {
                        command: "/quiz Ownership 5".into(),
                        label: "Ownership Quiz".into(),
                    }],
                },
                DayConfig {
                    day: 3,
                    title: "Lifetimes".into(),
                    tasks: vec![TaskConfig {
                        command: "/anlat Lifetimes".into(),
                        label: "Lifetimes".into(),
                    }],
                },
            ],
            trigger_kind: "eod".into(),
        };
        let preview = proposal_preview(&course, &proposal, Lang::Tr);
        assert_eq!(preview.len(), 2);
        assert!(preview[0].contains("Gün 2 (güncellendi)"));
        assert!(preview[1].contains("Gün 3 (eklendi)"));
    }

    #[test]
    fn detect_adapt_trigger_skips_when_at_max_plan_days() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("studia.db")).unwrap();
        let start = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
        let day_90 = start + chrono::Days::new((plan::MAX_PLAN_DAYS - 1) as u64);
        let days = (1..=plan::MAX_PLAN_DAYS)
            .map(|d| DayConfig {
                day: d,
                title: format!("Day {d}"),
                tasks: vec![TaskConfig {
                    command: "/review".into(),
                    label: "Review".into(),
                }],
            })
            .collect();
        let course = CourseConfig {
            title: Some("Long Course".into()),
            persona: None,
            exam: None,
            plan: days,
        };
        plan::set_start(&store, "rust", start).unwrap();
        for d in 1..=plan::MAX_PLAN_DAYS {
            plan::mark_done(&store, "rust", start, d, 0).unwrap();
        }
        assert!(
            detect_adapt_trigger(&store, dir.path(), "rust", &course, 0, day_90)
                .unwrap()
                .is_none()
        );
    }
}
