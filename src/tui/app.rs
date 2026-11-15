use crate::{
    Result, ai,
    bot::{self, JobKind, JobResult, Session},
    coach, exam,
    i18n::Lang,
    invalid, plan, sources,
    store::{SessionStats, Store, StoredQuestion},
    telegram,
};
use chrono::{Days, Local, NaiveDate, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

pub const TABS: [&str; 6] = ["Sor", "Quiz", "Tekrar", "İstatistik", "Bugün", "Denemeler"];
pub const TABS_EN: [&str; 6] = ["Ask", "Quiz", "Review", "Stats", "Today", "Exams"];

pub fn tabs(lang: Lang) -> [&'static str; 6] {
    match lang {
        Lang::Tr => TABS,
        Lang::En => TABS_EN,
    }
}
/// Denemeler sekmesi: geçmiş denemeler ve seçilen denemenin analizi.
pub const DENEME_TAB: usize = 5;
/// `✓` ile başlayan başarı bildirimleri bu kadar sonra kaybolur; hatalar bir sonraki tuşa kadar kalır.
const NOTICE_TTL: Duration = Duration::from_secs(4);
/// Bu süreyi aşan AI işlerinde kullanıcıya uyarı gösterilir.
const LONG_WAIT: Duration = Duration::from_secs(300);

pub struct Subject {
    pub name: String,
    pub questions: i64,
    pub due: i64,
    pub reviews: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Answer,
    Topic,
    Count,
    /// Denemeler koç sohbeti: metin kutusundaki soru koça gider.
    DenemeChat,
    /// Ders anlatımı ekranındaki takip sohbeti: boş Enter quize geçer, dolu Enter soru sorar.
    LessonChat,
}

/// Koçun bir turdaki yanıtı. `Waiting`: AI henüz dönmedi.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoachReply {
    Waiting,
    Answer(String),
    Failed(&'static str),
}

/// Koç sohbetinde bir tur: öğrencinin sorusu (boşsa ilk değerlendirme) ve koçun yanıtı.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DenemeTurn {
    pub question: String,
    pub reply: CoachReply,
}

/// Denemeler sekmesindeki koç sohbeti. Yalnızca bellekte tutulur; sohbetten çıkınca kaybolur.
pub struct DenemeChat {
    pub session: i64,
    /// Panel başlığı, ör. "Deneme 2: 45/80 doğru".
    pub label: String,
    /// Sayısal döküm (düz metin); panelin en altında, kaydırarak ulaşılır.
    pub breakdown: String,
    pub turns: Vec<DenemeTurn>,
    /// Denemenin yanlış soruları, sınav sırasıyla. Tab ile bunlar arasında gezilir.
    pub wrongs: Vec<bot::WrongAnswer>,
    /// Şu an üzerinde konuşulan yanlışın dizini (`wrongs` içinde); yoksa genel sohbet.
    pub focus: Option<usize>,
    /// Takip açıkken görünüm her zaman en alttadır; yeni tur ve yanıtlar otomatik görünür.
    pub follow: bool,
    /// Takip kapalıyken baştan satır ofseti.
    pub scroll: u16,
    /// Son çizimde hesaplanan en büyük ofset (toplam satır − panel yüksekliği). Çizim yazar.
    pub max_scroll: Cell<u16>,
}

enum Work {
    Ai {
        subject: String,
        kind: JobKind,
    },
    Index,
    /// Koçun adım seçimi; sonucu `coach::parse` ile doğrulanır.
    Coach {
        snapshot: coach::Snapshot,
    },
}

/// Koçun durumu. `On`: kendi kendine çalışır. `Paused`: kullanıcı bir oturumu kesti; F3 ile sürer. `Off`: kapalı.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoachMode {
    On,
    Paused,
    Off,
}

impl CoachMode {
    fn from_state(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("paused") => Self::Paused,
            Some("off") => Self::Off,
            _ => Self::On,
        }
    }

    fn as_state(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Paused => "paused",
            Self::Off => "off",
        }
    }
}

struct Pending {
    work: Work,
    receiver: Receiver<Result<String>>,
    started: Instant,
}

/// Kullanıcıya gösterilen iş adı; bekleme göstergesinde kullanılır.
fn job_label(kind: &JobKind, lang: Lang) -> &'static str {
    match kind {
        JobKind::Answer { .. } | JobKind::LessonChat { .. } => {
            lang.pick("Yanıt hazırlanıyor", "Preparing answer")
        }
        JobKind::Quiz { .. } => lang.pick("Quiz hazırlanıyor", "Generating quiz"),
        JobKind::ReviewVariations { .. } => lang.pick(
            "Tekrar varyasyonları hazırlanıyor",
            "Generating review variations",
        ),
        JobKind::Lesson { .. } => lang.pick("Ders anlatımı hazırlanıyor", "Preparing lesson"),
        JobKind::Deneme { .. } => lang.pick(
            "Deneme sınavı hazırlanıyor (birkaç dakika sürebilir)",
            "Generating mock exam (may take a few minutes)",
        ),
        JobKind::Vocab { .. } => lang.pick("Kavram kartları hazırlanıyor", "Preparing flashcards"),
        JobKind::DenemeCoach { .. } => lang.pick("Koç yanıtlıyor", "Coach is answering"),
    }
}

/// Ctrl kısayolu olmayan bir harf yutulduğunda durum satırında gösterilen tanı metni.
fn ignored_key(key: &KeyEvent, lang: Lang) -> String {
    let mut text = String::from(lang.pick("Yok sayılan tuş: ", "Ignored key: "));
    for (flag, name) in [
        (KeyModifiers::CONTROL, "Ctrl+"),
        (KeyModifiers::ALT, "Alt+"),
        (KeyModifiers::SHIFT, "Shift+"),
    ] {
        if key.modifiers.contains(flag) {
            text.push_str(name);
        }
    }
    match key.code {
        KeyCode::Char(ch) => text.push(ch),
        other => text.push_str(&format!("{other:?}")),
    }
    text
}

/// Gerçek yapay zekâ çağrısı; iş parçacığında çalışır. Testler yerine sahte bir yanıt verebilir.
fn ai_generate(root: &Path) -> impl FnOnce(String, String) -> Result<String> + Send + 'static {
    let root = root.to_path_buf();
    move |prompt, schema| ai::generate(&root, &prompt, &schema)
}

/// Koça giden geçmiş: yanıtı tamamlanmış son 6 tur, sırasıyla (öğrenci, koç). Hatalı turlar girmez.
fn coach_history(chat: &DenemeChat, lang: Lang) -> Vec<(String, String)> {
    let done: Vec<(&str, &str)> = chat
        .turns
        .iter()
        .filter_map(|turn| match &turn.reply {
            CoachReply::Answer(answer) => Some((turn.question.as_str(), answer.as_str())),
            _ => None,
        })
        .collect();
    done[done.len().saturating_sub(6)..]
        .iter()
        .flat_map(|(question, answer)| {
            let asked = if question.is_empty() {
                lang.pick("Denemeyi değerlendir.", "Evaluate this mock exam.")
            } else {
                question
            };
            [
                ("user".to_string(), asked.to_string()),
                ("assistant".to_string(), answer.to_string()),
            ]
        })
        .collect()
}

/// Ders sohbetine giden geçmiş: yanıtı tamamlanmış son 6 tur.
fn lesson_history(turns: &[DenemeTurn]) -> Vec<(String, String)> {
    let done: Vec<(&str, &str)> = turns
        .iter()
        .filter_map(|turn| match &turn.reply {
            CoachReply::Answer(answer) => Some((turn.question.as_str(), answer.as_str())),
            _ => None,
        })
        .collect();
    done[done.len().saturating_sub(6)..]
        .iter()
        .flat_map(|(question, answer)| {
            [
                ("user".to_string(), (*question).to_string()),
                ("assistant".to_string(), (*answer).to_string()),
            ]
        })
        .collect()
}

/// Süreyi `dd:ss` biçiminde yazar.
fn clock(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

pub struct App {
    root: PathBuf,
    pub store: Store,
    pub subjects: Vec<Subject>,
    pub selected: usize,
    pub tab: usize,
    pub input: String,
    pub editing: Option<Input>,
    topic: String,
    pub answer: String,
    pub scroll: u16,
    pub session: Option<Session>,
    pub session_tab: usize,
    pub session_subject: String,
    pub feedback: Option<String>,
    pub summary: String,
    summary_session: Option<i64>,
    pending: Option<Pending>,
    /// AI ile hazırlanan plan görevi (gün, görev); içerik kaydedilince işaretlenir.
    plan_task: Option<(NaiveDate, usize, usize)>,
    /// `/anlat` ya da `/kelime` konu bekliyor (Telegram'daki `awaiting` durumu).
    awaiting: Option<&'static str>,
    pub stats: Vec<SessionStats>,
    pub status: String,
    pub listener: bool,
    pub quit: bool,
    pub course: crate::course::CourseConfig,
    pub plan: plan::Status,
    /// Bugün/Plan sekmesinde elle seçilen gün (None ise bugünün aktif günü ya da 1).
    pub plan_view_day: Option<usize>,
    pub task_selected: usize,
    /// Denemeler sekmesi: son bitmiş denemeler, seçili sıra, açık koç sohbeti ve silme onayı
    /// bekleyen deneme kimliği.
    pub deneme_items: Vec<SessionStats>,
    pub deneme_selected: usize,
    pub deneme_chat: Option<DenemeChat>,
    pub deneme_delete: Option<i64>,
    pub lesson: Option<String>,
    pub lesson_topic: String,
    pub lesson_turns: Vec<DenemeTurn>,
    pub lesson_follow: bool,
    pub lesson_max_scroll: Cell<u16>,
    /// F1 yardım penceresi açık mı.
    pub help: bool,
    /// Son Sor yanıtının sorusu; Ctrl+Q ile bu konudan quiz üretilir.
    answer_question: Option<String>,
    /// Son quizde yanlış cevaplanan sorular; "w" ile tekrar edilir.
    last_wrong: Vec<StoredQuestion>,
    /// TUI'den başlatılan Telegram dinleyicisi; TUI kapanınca durdurulur.
    listener_child: Option<Child>,
    status_seen: String,
    status_since: Instant,
    /// Ders bazında toplam (doğru, toplam).
    pub accuracy: Vec<(String, i64, i64)>,
    /// Seçili dersin en zayıf konuları (doğru, toplam).
    pub weakest: Vec<(String, i64, i64)>,
    /// Son yedi günün günlük (gün, doğru, toplam) değerleri.
    pub daily: Vec<(String, i64, i64)>,
    pub coach_mode: CoachMode,
    /// Bir sonraki adımın seçilmesi gerekiyor mu (oturum bitince veya açılışta).
    coach_due: bool,
    /// Son seçilen adım; Bugün sekmesinde gösterilir.
    pub coach_step: Option<coach::Step>,
    /// Seçilmiş ve henüz başlatılmamış adım.
    coach_ready: Option<coach::Step>,
    /// Son tamamlanan koç oturumunun özeti.
    pub coach_note: String,
    pub lang: Lang,
}

impl App {
    #[cfg(test)]
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root.join("data"))?;
        let canonical = fs::canonicalize(root)?;
        let store = Store::open(&canonical.join("data/studia.db"))?;
        let lang = store
            .state("lang")?
            .as_deref()
            .and_then(Lang::parse)
            .unwrap_or(Lang::Tr);
        drop(store);
        Self::open_with(&canonical, lang)
    }

    pub fn open_with(root: &Path, lang: Lang) -> Result<Self> {
        fs::create_dir_all(root.join("data"))?;
        let root = fs::canonicalize(root)?;
        let store = Store::open(&root.join("data/studia.db"))?;
        let coach_mode = CoachMode::from_state(store.state("coach_mode")?);
        let mut app = Self {
            root,
            store,
            subjects: vec![],
            selected: 0,
            tab: 0,
            input: String::new(),
            editing: None,
            topic: String::new(),
            answer: String::new(),
            scroll: 0,
            session: None,
            session_tab: 1,
            session_subject: String::new(),
            feedback: None,
            summary: String::new(),
            summary_session: None,
            pending: None,
            plan_task: None,
            awaiting: None,
            stats: vec![],
            status: String::new(),
            listener: false,
            quit: false,
            course: crate::course::CourseConfig::default(),
            plan: plan::Status::NotStarted,
            plan_view_day: None,
            task_selected: 0,
            deneme_items: vec![],
            deneme_selected: 0,
            deneme_chat: None,
            deneme_delete: None,
            lesson: None,
            lesson_topic: String::new(),
            lesson_turns: vec![],
            lesson_follow: false,
            lesson_max_scroll: Cell::new(u16::MAX),
            help: false,
            answer_question: None,
            last_wrong: vec![],
            listener_child: None,
            status_seen: String::new(),
            status_since: Instant::now(),
            accuracy: vec![],
            weakest: vec![],
            daily: vec![],
            coach_mode,
            coach_due: coach_mode == CoachMode::On,
            coach_step: None,
            coach_ready: None,
            coach_note: String::new(),
            lang,
        };
        app.refresh()?;
        Ok(app)
    }

    pub fn refresh(&mut self) -> Result<()> {
        let names = sources::subjects(&self.root)?;
        let active = self.store.state("active_subject")?;
        let mut subjects = vec![];
        for name in names {
            let (due, reviews) = self.store.review_counts(&name, Utc::now())?;
            subjects.push(Subject {
                questions: self.store.question_count(&name)?,
                name,
                due,
                reviews,
            });
        }
        self.subjects = subjects;
        self.selected = active
            .as_ref()
            .and_then(|name| self.subjects.iter().position(|s| &s.name == name))
            .unwrap_or_else(|| self.selected.min(self.subjects.len().saturating_sub(1)));
        if let Some(subject) = self.subjects.get(self.selected)
            && active.as_deref() != Some(&subject.name)
        {
            self.store.set_state("active_subject", &subject.name)?;
        }
        self.course = match self.subjects.get(self.selected) {
            Some(subject) => {
                crate::course::CourseConfig::load_or_default(&self.root, &subject.name)?
            }
            None => crate::course::CourseConfig::default(),
        };
        self.plan = match self.subjects.get(self.selected) {
            Some(subject) => plan::status(
                &self.store,
                &subject.name,
                &self.course.plan,
                Local::now().date_naive(),
            )?,
            None => plan::Status::NotStarted,
        };
        self.stats = self.store.recent_sessions()?;
        self.accuracy = self.store.subject_accuracy()?;
        self.weakest = match self.subjects.get(self.selected) {
            Some(subject) => self.store.weakest_topics(&subject.name, 3)?,
            None => vec![],
        };
        let today = Utc::now().date_naive();
        let since = today
            .checked_sub_days(Days::new(6))
            .unwrap_or(today)
            .format("%Y-%m-%d")
            .to_string();
        self.daily = self.store.daily_accuracy(&since)?;
        // The listener denies file sharing on Windows; existence alone is not status.
        self.listener = listener_running(&self.root.join("data/listen.lock"));
        Ok(())
    }

    fn subject(&self) -> Result<String> {
        self.subjects
            .get(self.selected)
            .map(|s| s.name.clone())
            .ok_or_else(|| {
                invalid(self.lang.pick(
                    "kaynaklar/<ders>/ klasörüne belge ekleyin.",
                    "Add study files under kaynaklar/<course>/.",
                ))
            })
    }

    pub fn spinner(&self) -> Option<String> {
        self.pending.as_ref().map(|pending| {
            let elapsed = pending.started.elapsed();
            let glyph = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
                [(elapsed.as_millis() / 100 % 10) as usize];
            let (label, cancellable) = match &pending.work {
                Work::Index => (
                    self.lang
                        .pick("Kaynaklar indeksleniyor", "Indexing sources"),
                    false,
                ),
                Work::Coach { .. } => (
                    self.lang.pick(
                        "Koç bir sonraki adımı seçiyor",
                        "Coach is choosing next step",
                    ),
                    true,
                ),
                // Koç ve ders sohbetinde Esc sohbetten çıkarır; iş arka planda biter, sonucu atılır.
                Work::Ai { kind, .. } => (
                    job_label(kind, self.lang),
                    !matches!(
                        kind,
                        JobKind::DenemeCoach { .. } | JobKind::LessonChat { .. }
                    ),
                ),
            };
            let mut text = format!("{glyph} {label}… {}", clock(elapsed));
            if elapsed >= LONG_WAIT {
                text.push_str(self.lang.pick(" · Uzun sürüyor", " · Taking a while"));
            }
            if cancellable {
                text.push_str(self.lang.pick(" · Esc iptal", " · Esc cancel"));
            }
            text
        })
    }

    /// Her döngüde çağrılır. Yeni bir durum mesajı gelince zamanı başlatır, süresi dolan başarı bildirimini siler.
    pub fn tick(&mut self) {
        if self.status != self.status_seen {
            self.status_seen = self.status.clone();
            self.status_since = Instant::now();
        } else if self.status.starts_with('✓') && self.status_since.elapsed() >= NOTICE_TTL {
            self.status.clear();
            self.status_seen.clear();
        }
    }

    /// Bekleyen AI işini iptal eder. Sonuç kaydedilmez; arka plan iş parçacığı kendi kendine biter.
    /// Dizinleme yarıda bırakılmaz, çünkü veritabanını yazmaya devam eder.
    fn cancel_pending(&mut self) -> Result<()> {
        match self.pending.as_ref().map(|pending| &pending.work) {
            Some(Work::Ai { .. }) => {
                self.pending = None;
                self.status = self
                    .lang
                    .pick(
                        "İptal edildi. Sonuç kaydedilmeyecek.",
                        "Cancelled. Result will be discarded.",
                    )
                    .into();
            }
            Some(Work::Coach { .. }) => {
                // The user stopped the coach mid-decision: wait for F3 instead of deciding again.
                self.pending = None;
                self.set_coach_mode(CoachMode::Paused)?;
                self.status = self
                    .lang
                    .pick(
                        "Koç duraklatıldı. F3 ile sürdürün.",
                        "Coach paused. Press F3 to resume.",
                    )
                    .into();
            }
            Some(Work::Index) => {
                self.status = self
                    .lang
                    .pick(
                        "Dizinleme bitene kadar bekleyin.",
                        "Wait for indexing to finish.",
                    )
                    .into();
            }
            None => {}
        }
        Ok(())
    }

    fn begin_ai(&mut self, kind: JobKind) -> Result<()> {
        self.begin_ai_with(kind, ai_generate(&self.root))
    }

    /// Plan görevi için AI işi: görev, içerik kaydedilince işaretlenir (bkz. `poll_pending`).
    fn begin_plan_ai(&mut self, kind: JobKind, task: (NaiveDate, usize, usize)) -> Result<()> {
        self.plan_task = Some(task);
        let result = self.begin_ai(kind);
        if self.pending.is_none() {
            self.plan_task = None;
        }
        result
    }

    fn begin_ai_with(
        &mut self,
        kind: JobKind,
        generate: impl FnOnce(String, String) -> Result<String> + Send + 'static,
    ) -> Result<()> {
        if self.pending.is_some() {
            self.status = self
                .lang
                .pick(
                    "Önceki istek sürüyor…",
                    "Previous request is still running…",
                )
                .into();
            return Ok(());
        }
        let subject = self.subject()?;
        let work_subject = subject.clone();
        let work_kind = kind.clone();
        let root = self.root.clone();
        let lang = self.lang;
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("studia-tui-ai".into())
            .spawn(move || {
                let result = (|| {
                    let store = Store::open(&root.join("data/studia.db"))?;
                    let (prompt, schema) =
                        bot::prepare_job_in(&root, &store, &work_subject, &work_kind, lang)?;
                    generate(prompt, schema)
                })();
                let _ = sender.send(result);
            })?;
        self.pending = Some(Pending {
            work: Work::Ai { subject, kind },
            receiver,
            started: Instant::now(),
        });
        self.status.clear();
        Ok(())
    }

    /// Koçun kararını AI'ye sorar. Durum özeti burada, ana iş parçacığında okunur.
    fn begin_coach_with(
        &mut self,
        generate: impl FnOnce(String, String) -> Result<String> + Send + 'static,
    ) -> Result<()> {
        if self.pending.is_some() {
            return Ok(());
        }
        let subject = self.subject()?;
        let course = crate::course::CourseConfig::load_or_default(&self.root, &subject)?;
        let today = Local::now().date_naive();
        let due = self.store.review_counts(&subject, Utc::now())?.0;
        let snapshot = coach::snapshot(&self.store, &subject, &course, due, today)?;
        if snapshot.program == coach::Program::NotStarted {
            // The coach starts the plan today; from here the plan's own day order applies.
            plan::set_start(&self.store, &subject, today)?;
            self.refresh()?;
            self.coach_due = true;
            self.status = self
                .lang
                .pick("✓ Plan bugünden başlatıldı.", "✓ Study plan started today.")
                .into();
            return Ok(());
        }
        if !snapshot.has_coachable() {
            self.status = coach::idle_message(&snapshot, self.lang);
            return Ok(());
        }
        let prompt = coach::prompt(&snapshot, self.lang);
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("studia-tui-coach".into())
            .spawn(move || {
                let _ = sender.send(generate(prompt, coach::SCHEMA.into()));
            })?;
        self.pending = Some(Pending {
            work: Work::Coach { snapshot },
            receiver,
            started: Instant::now(),
        });
        Ok(())
    }

    fn begin_coach(&mut self) -> Result<()> {
        let root = self.root.clone();
        self.begin_coach_with(move |prompt, schema| ai::generate(&root, &prompt, &schema))
    }

    /// Her döngüde çağrılır: boşsa önce seçilmiş adımı başlatır, yoksa gerekiyorsa yeni karar ister.
    pub fn coach_tick(&mut self) -> Result<()> {
        if self.coach_mode != CoachMode::On
            || self.pending.is_some()
            || self.session.is_some()
            || self.lesson.is_some()
            || self.subjects.is_empty()
        {
            return Ok(());
        }
        if let Some(step) = self.coach_ready.take() {
            return self.run_step(step);
        }
        if self.coach_due {
            self.coach_due = false;
            return self.begin_coach();
        }
        Ok(())
    }

    /// Seçilen plan görevini çalıştırır. Tekrar yerel çalışır; anlatım, test ve deneme AI üretir.
    fn run_step(&mut self, step: coach::Step) -> Result<()> {
        self.status = format!("✓ {}: {}", self.lang.pick("Koç", "Coach"), step.label);
        let subject = self.subject()?;
        let today = Local::now().date_naive();
        let start = match plan::status(&self.store, &subject, &self.course.plan, today)? {
            plan::Status::Active { start, .. } => start,
            _ => return Ok(()),
        };
        self.plan_view_day = Some(step.day);
        self.task_selected = step.task;
        if matches!(
            step.command.split_whitespace().next(),
            Some("/tekrar" | "/review")
        ) {
            self.tab = 2;
            self.quiz_enter()?;
            plan::mark_done(&self.store, &subject, start, step.day, step.task)?;
            if self.session.is_none() {
                // Tekrar kuyruğu boş; plan sırasındaki bir sonraki göreve geç.
                self.coach_due = true;
            }
            return Ok(());
        }
        // plan_enter starts AI-backed tasks and marks them done itself.
        self.tab = 4;
        self.plan_enter()
    }

    fn set_coach_mode(&mut self, mode: CoachMode) -> Result<()> {
        self.coach_mode = mode;
        self.store.set_state("coach_mode", mode.as_state())?;
        self.coach_due = mode == CoachMode::On;
        self.coach_ready = None;
        Ok(())
    }

    /// F3: koç açıksa kapatır, değilse açar (beklemedeyken de sürdürür).
    fn toggle_coach(&mut self) -> Result<()> {
        let next = if self.coach_mode == CoachMode::On {
            CoachMode::Off
        } else {
            CoachMode::On
        };
        self.set_coach_mode(next)?;
        self.status = match next {
            CoachMode::On => self
                .lang
                .pick(
                    "✓ Koç açık: sıradaki adım seçiliyor.",
                    "✓ Coach enabled: choosing next step.",
                )
                .into(),
            _ => self
                .lang
                .pick(
                    "Koç kapalı. F3 ile yeniden açın.",
                    "Coach disabled. Press F3 to enable.",
                )
                .into(),
        };
        Ok(())
    }

    fn toggle_lang(&mut self) -> Result<()> {
        let next = match self.lang {
            Lang::Tr => Lang::En,
            Lang::En => Lang::Tr,
        };
        self.lang = next;
        self.store.set_state("lang", next.code())?;
        self.status = next
            .pick(
                "✓ Dil Türkçe olarak ayarlandı.",
                "✓ Language switched to English.",
            )
            .into();
        self.refresh()
    }

    /// Bir quiz veya tekrar oturumu bittiğinde özet yazar ve bir sonraki adım için karar ister.
    fn coach_after_session(&mut self, topic: &str, correct: usize, answered: usize) {
        self.coach_note = match self.lang {
            Lang::Tr => format!("{topic}: {correct}/{answered} doğru."),
            Lang::En => format!("{topic}: {correct}/{answered} correct."),
        };
        if self.coach_mode == CoachMode::On {
            self.coach_due = true;
        }
    }

    fn begin_index(&mut self) -> Result<()> {
        if self.pending.is_some() {
            self.status = self
                .lang
                .pick(
                    "Önceki istek sürüyor…",
                    "Previous request is still running…",
                )
                .into();
            return Ok(());
        }
        let root = self.root.clone();
        let lang = self.lang;
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("studia-tui-index".into())
            .spawn(move || {
                let _ = sender.send(sources::index(&root, true).map(|count| {
                    format!(
                        "{count} {}",
                        lang.pick("dosya güncellendi.", "file(s) updated.")
                    )
                }));
            })?;
        self.pending = Some(Pending {
            work: Work::Index,
            receiver,
            started: Instant::now(),
        });
        Ok(())
    }

    pub fn poll(&mut self) -> Result<()> {
        self.check_listener_child();
        let poll_state = self
            .pending
            .as_ref()
            .map(|pending| pending.receiver.try_recv());
        let result = match poll_state {
            None => return Ok(()),
            Some(Ok(result)) => result,
            Some(Err(TryRecvError::Empty)) => return Ok(()),
            Some(Err(TryRecvError::Disconnected)) => Err(invalid(
                self.lang
                    .pick("AI iş parçacığı durdu.", "AI background worker stopped."),
            )),
        };
        let Some(pending) = self.pending.take() else {
            return Ok(());
        };
        // Hata alan ya da başka iş için başlatılan görev burada düşer; işaretlenmez.
        let plan_task = self.plan_task.take();
        if let Work::Coach { snapshot } = pending.work {
            // An unusable AI answer falls back to the same rules, so the coach never stalls.
            let parsed = result.ok().and_then(|raw| coach::parse(&raw, &snapshot));
            let step = match parsed {
                Some(step) => Some(step),
                None => coach::rule_step(&snapshot, self.lang).map(|mut step| {
                    step.reason = format!(
                        "{} {}",
                        self.lang.pick(
                            "AI yanıtı kullanılamadı; yerel kurallarla seçildi.",
                            "AI response unavailable; selected via local rules.",
                        ),
                        step.reason
                    );
                    step
                }),
            };
            match step {
                Some(step) => {
                    self.coach_step = Some(step.clone());
                    if self.coach_mode == CoachMode::On {
                        self.coach_ready = Some(step);
                    }
                }
                None => {
                    self.coach_step = None;
                    self.status = self
                        .lang
                        .pick(
                            "Koç: yapılacak bir adım bulunamadı.",
                            "Coach: no actionable step found.",
                        )
                        .into();
                }
            }
            return self.refresh();
        }
        // Koç hatası sohbet turunda gösterilir; Sor yanıtına ve durum satırına dokunmaz.
        if let Work::Ai { subject, kind } = &pending.work
            && let JobKind::DenemeCoach { session, .. } = kind
        {
            let reply = match result {
                Ok(raw) => match bot::save_job(&mut self.store, subject, kind, &raw) {
                    Ok(JobResult::Answer(answer)) => CoachReply::Answer(answer),
                    _ => CoachReply::Failed(bot::failure_text_in(kind, self.lang)),
                },
                Err(_) => CoachReply::Failed(bot::failure_text_in(kind, self.lang)),
            };
            self.finish_deneme_chat(*session, reply);
            return self.refresh();
        }
        if let Work::Ai { subject, kind } = &pending.work
            && let JobKind::LessonChat { topic, .. } = kind
        {
            let reply = match result {
                Ok(raw) => match bot::save_job(&mut self.store, subject, kind, &raw) {
                    Ok(JobResult::Answer(answer)) => CoachReply::Answer(answer),
                    _ => CoachReply::Failed(bot::failure_text_in(kind, self.lang)),
                },
                Err(_) => CoachReply::Failed(bot::failure_text_in(kind, self.lang)),
            };
            self.finish_lesson_chat(subject, topic, reply)?;
            return self.refresh();
        }
        let raw = result?;
        match pending.work {
            Work::Index => self.status = format!("✓ {raw}"),
            Work::Coach { .. } => unreachable!(),
            Work::Ai { subject, kind } => {
                let saved = bot::save_job(&mut self.store, &subject, &kind, &raw)?;
                if let Some((start, day, index)) = plan_task {
                    plan::mark_done(&self.store, &subject, start, day, index)?;
                }
                match saved {
                    JobResult::Answer(answer) => {
                        let question = match kind {
                            JobKind::Answer { question } => {
                                self.answer_question = Some(question.clone());
                                question
                            }
                            JobKind::Lesson { topic } => topic,
                            _ => unreachable!(),
                        };
                        self.store.set_state(
                            &format!("answer_cache:{subject}:{}:{question}", self.lang.code()),
                            &answer,
                        )?;
                        let course_prefix = self.lang.pick("Ders", "Course");
                        self.answer =
                            format!("{course_prefix}: {subject}\n\n{question}\n\n{answer}");
                        self.scroll = 0;
                        self.status = if self.tab == 0 {
                            self.lang.pick("✓ Yanıt hazır.", "✓ Answer ready.").into()
                        } else {
                            self.lang
                                .pick(
                                    "✓ Yanıt hazır · Sor sekmesinde.",
                                    "✓ Answer ready · see Ask tab.",
                                )
                                .into()
                        };
                    }
                    JobResult::Quiz(questions) => {
                        let is_exam = matches!(&kind, JobKind::Deneme { .. });
                        let topic = match &kind {
                            JobKind::Quiz { topic, .. } => topic.clone(),
                            JobKind::ReviewVariations { .. } => self
                                .lang
                                .pick("Tekrar Varyasyonları", "Review Variations")
                                .into(),
                            JobKind::Deneme { .. } => {
                                self.lang.pick("Deneme Sınavı", "Mock Exam").into()
                            }
                            _ => unreachable!(),
                        };
                        let count = questions.len();
                        self.start_session_with_kind(&subject, &topic, questions, 1, is_exam)?;
                        self.status = match self.lang {
                            Lang::Tr => format!("✓ Quiz hazır: {count} soru."),
                            Lang::En => format!("✓ Quiz ready: {count} question(s)."),
                        };
                        self.editing = None;
                        self.input.clear();
                        self.tab = 1;
                    }
                    JobResult::Vocab(cards) => {
                        let topic = match &kind {
                            JobKind::Vocab { topic, .. } => topic.clone(),
                            _ => unreachable!(),
                        };
                        let path = bot::write_anki(&self.root, &subject, &topic, &cards)?;
                        let preview = cards
                            .iter()
                            .take(10)
                            .map(|card| format!("• {} — {}", card.word, card.meaning))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let msg = match self.lang {
                            Lang::Tr => format!(
                                "🃏 {} kavram kartı hazır — {topic}\nAnki: Dosya → İçe Aktar → {}\n\n{preview}",
                                cards.len(),
                                path.display()
                            ),
                            Lang::En => format!(
                                "🃏 {} flashcards ready — {topic}\nAnki: File → Import → {}\n\n{preview}",
                                cards.len(),
                                path.display()
                            ),
                        };
                        self.show(&msg);
                        self.status = self
                            .lang
                            .pick("✓ Kartlar hazır.", "✓ Flashcards ready.")
                            .into();
                    }
                    JobResult::Lesson(lesson) => {
                        let raw_topic = match &kind {
                            JobKind::Lesson { topic } => topic.clone(),
                            _ => unreachable!(),
                        };
                        self.store
                            .set_state(&coach::taught_key(&subject, &raw_topic), "1")?;
                        let suffix = self.lang.pick("(Kazanım Testi)", "(Quick Check)");
                        let topic = format!("{raw_topic} {suffix}");
                        self.lesson = Some(lesson.lesson.clone());
                        self.lesson_topic = raw_topic.clone();
                        self.lesson_turns.clear();
                        self.lesson_follow = false;
                        self.lesson_max_scroll.set(u16::MAX);
                        self.editing = Some(Input::LessonChat);
                        self.input.clear();
                        let questions =
                            self.store
                                .add_questions(&subject, &topic, lesson.questions)?;
                        self.store.save_lesson(
                            &subject,
                            self.lang,
                            &raw_topic,
                            &lesson.lesson,
                            &questions,
                            &[],
                        )?;
                        let course =
                            crate::course::CourseConfig::load_or_default(&self.root, &subject)?;
                        self.session = Some(
                            Session::start_at(
                                &self.store,
                                &subject,
                                &topic,
                                questions,
                                Utc::now(),
                            )?
                            .with_course(course),
                        );
                        self.session_subject = subject;
                        // The lesson is shown on the Bugün tab, then its quiz starts there.
                        self.session_tab = 4;
                        self.tab = 4;
                        self.feedback = None;
                        self.summary.clear();
                        self.summary_session = None;
                        self.scroll = 0;
                        self.status = self
                            .lang
                            .pick(
                                "✓ Ders anlatımı hazır; quiz Bugün sekmesinde başlıyor.",
                                "✓ Lesson ready; quiz opens in Today tab.",
                            )
                            .into();
                    }
                }
            }
        }
        self.refresh()
    }

    fn start_session(
        &mut self,
        subject: &str,
        topic: &str,
        questions: Vec<StoredQuestion>,
        tab: usize,
    ) -> Result<()> {
        self.start_session_with_kind(subject, topic, questions, tab, topic.contains("Deneme"))
    }

    fn start_session_with_kind(
        &mut self,
        subject: &str,
        topic: &str,
        questions: Vec<StoredQuestion>,
        tab: usize,
        is_exam: bool,
    ) -> Result<()> {
        let course = crate::course::CourseConfig::load_or_default(&self.root, subject)?;
        let session =
            Session::start_kind_at(&self.store, subject, topic, questions, is_exam, Utc::now())?
                .with_course(course);
        self.install_session(subject, session, tab);
        Ok(())
    }

    fn install_session(&mut self, subject: &str, session: Session, tab: usize) {
        self.session = Some(session);
        self.session_tab = tab;
        self.session_subject = subject.into();
        self.feedback = None;
        self.summary.clear();
        self.summary_session = None;
        self.lesson = None;
        self.lesson_topic.clear();
        self.lesson_turns.clear();
        self.lesson_follow = false;
        self.scroll = 0;
    }

    pub fn close_session(&mut self) -> Result<()> {
        self.lesson = None;
        self.lesson_topic.clear();
        self.lesson_turns.clear();
        self.lesson_follow = false;
        if self.editing == Some(Input::LessonChat) {
            self.editing = None;
        }
        if let Some(session) = &self.session {
            self.last_wrong = session.wrong_questions();
            self.summary = session.finish_in(&self.store, self.lang, true)?;
            self.summary_session = None;
            self.session = None;
            self.feedback = None;
        }
        Ok(())
    }

    /// Dinleyici çalışmıyorsa `studia listen` olarak başlatır; çalışıyorsa ve TUI başlatmışsa durdurur.
    fn toggle_listener(&mut self) -> Result<()> {
        if let Some(child) = self.listener_child.as_mut() {
            if child.try_wait()?.is_none() {
                child.kill()?;
                child.wait()?;
                self.listener_child = None;
                self.refresh()?;
                self.status = self
                    .lang
                    .pick(
                        "✓ Telegram dinleyici durduruldu.",
                        "✓ Telegram listener stopped.",
                    )
                    .into();
                return Ok(());
            }
            self.listener_child = None;
        }
        if self.listener {
            self.status = self
                .lang
                .pick(
                    "Dinleyici başka bir pencerede çalışıyor; orada Ctrl+C ile kapatın.",
                    "Listener is running in another window; stop it there with Ctrl+C.",
                )
                .into();
            return Ok(());
        }
        let child = Command::new(std::env::current_exe()?)
            .arg("--root")
            .arg(&self.root)
            .arg("--lang")
            .arg(self.lang.code())
            .arg("listen")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        self.listener_child = Some(child);
        self.status = self
            .lang
            .pick(
                "Telegram dinleyici başlatılıyor…",
                "Starting Telegram listener…",
            )
            .into();
        Ok(())
    }

    /// TUI'den başlatılan dinleyiciyi kapatır; TUI kapanırken çağrılır.
    pub fn stop_listener(&mut self) {
        if let Some(mut child) = self.listener_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// TUI'den başlatılan dinleyici beklenmedik biçimde kapandıysa bunu durum satırına yazar.
    fn check_listener_child(&mut self) {
        let Some(child) = self.listener_child.as_mut() else {
            return;
        };
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            self.listener_child = None;
            self.status = self
                .lang
                .pick(
                    "Telegram dinleyici durdu. Ayarları ve data/bot.log dosyasını kontrol edin.",
                    "Telegram listener stopped. Check your credentials and data/bot.log.",
                )
                .into();
        }
    }

    /// Sor yanıtının konusundan beş soruluk quiz üretir (Ctrl+Q).
    fn quiz_from_answer(&mut self) -> Result<()> {
        let Some(question) = self.answer_question.clone() else {
            self.status = self
                .lang
                .pick(
                    "Önce Sor sekmesinde bir soru sorun.",
                    "Ask a question in the Ask tab first.",
                )
                .into();
            return Ok(());
        };
        if self.pending.is_some() {
            self.status = self
                .lang
                .pick(
                    "Önceki istek sürüyor…",
                    "Previous request is still running…",
                )
                .into();
            return Ok(());
        }
        self.begin_ai(JobKind::Quiz {
            topic: question,
            count: 5,
        })
    }

    /// Son quizde yanlış cevaplanan soruları yeniden başlatır (w).
    fn retry_wrong(&mut self) -> Result<()> {
        if self.last_wrong.is_empty() {
            self.status = self
                .lang
                .pick(
                    "Son quizde yanlış cevap yok.",
                    "No mistakes from the last quiz.",
                )
                .into();
            return Ok(());
        }
        let subject = self.subject()?;
        let questions = std::mem::take(&mut self.last_wrong);
        self.start_session(
            &subject,
            self.lang.pick("Yanlışlar", "Mistakes"),
            questions,
            1,
        )
    }

    /// Plan henüz başlamadıysa bugünden başlatır (b). Plan zaten aktifse bugüne döner.
    fn plan_start(&mut self) -> Result<()> {
        if self.course.plan.is_empty() {
            self.status = self
                .lang
                .pick(
                    "Bu ders için çalışma planı tanımlı değil.",
                    "No study plan defined for this course.",
                )
                .into();
            return Ok(());
        }
        if matches!(self.plan, plan::Status::NotStarted) {
            let subject = self.subject()?;
            plan::set_start(&self.store, &subject, Local::now().date_naive())?;
            self.plan_view_day = None;
            self.refresh()?;
            self.status = self
                .lang
                .pick("✓ Plan bugünden başladı.", "✓ Study plan started today.")
                .into();
        } else {
            self.plan_today();
        }
        Ok(())
    }

    /// Bugün/Plan sekmesinde görüntülenen gün numarası.
    pub fn plan_day(&self) -> usize {
        let len = self.course.plan.len();
        if len == 0 {
            return 1;
        }
        self.plan_view_day
            .filter(|&day| (1..=len).contains(&day))
            .unwrap_or(match self.plan {
                plan::Status::Active { day, .. } => day,
                _ => 1,
            })
    }

    pub fn plan_prev_day(&mut self) {
        let len = self.course.plan.len();
        let current = self.plan_day();
        if current > 1 && len > 0 {
            self.plan_view_day = Some(current - 1);
            self.task_selected = 0;
            self.status = match self.lang {
                Lang::Tr => format!("Gün {}/{len} gösteriliyor.", current - 1),
                Lang::En => format!("Showing Day {}/{len}.", current - 1),
            };
        }
    }

    pub fn plan_next_day(&mut self) {
        let len = self.course.plan.len();
        let current = self.plan_day();
        if current < len {
            self.plan_view_day = Some(current + 1);
            self.task_selected = 0;
            self.status = match self.lang {
                Lang::Tr => format!("Gün {}/{len} gösteriliyor.", current + 1),
                Lang::En => format!("Showing Day {}/{len}.", current + 1),
            };
        }
    }

    pub fn plan_today(&mut self) {
        let len = self.course.plan.len();
        self.plan_view_day = None;
        self.task_selected = 0;
        match self.plan {
            plan::Status::Active { day, .. } if len > 0 => {
                self.status = match self.lang {
                    Lang::Tr => format!("Bugüne dönüldü: Gün {day}/{len}."),
                    Lang::En => format!("Returned to today: Day {day}/{len}."),
                };
            }
            _ => {
                self.status = self
                    .lang
                    .pick("Gün 1 gösteriliyor.", "Showing Day 1.")
                    .into();
            }
        }
    }

    /// Next unused exam bank discovered for `subject`.
    fn next_bank(&self, subject: &str) -> Result<Option<(exam::DiscoveredBank, exam::Bank)>> {
        exam::next_unused_bank(&self.store, &self.root, subject)
    }

    fn start_unused_bank_exam(&mut self, subject: &str) -> Result<bool> {
        if let Some((meta, bank)) = self.next_bank(subject)? {
            self.start_bank_exam(subject, &meta.slug, &bank)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Starts an exam bank sequentially without calling AI.
    fn start_bank_exam(&mut self, subject: &str, slug: &str, bank: &exam::Bank) -> Result<()> {
        let questions =
            self.store
                .add_exam_questions(subject, &bank.title, bank.questions.clone())?;
        self.store.set_state(&exam::used_key(subject, slug), "1")?;
        let course = crate::course::CourseConfig::load_or_default(&self.root, subject)?;
        let session =
            Session::start_exam(&self.store, subject, &bank.title, questions)?.with_course(course);
        self.store
            .set_state(&format!("deneme_bank:{}", session.id), slug)?;
        self.install_session(subject, session, 4);
        self.tab = 4;
        let broken = bank
            .questions
            .iter()
            .filter(|question| exam::is_broken(&question.question))
            .count();
        self.status = match self.lang {
            Lang::Tr => format!(
                "✓ {} başladı: {} soru, {broken} tanesi OCR bozuk işaretli.",
                bank.title,
                bank.questions.len()
            ),
            Lang::En => format!(
                "✓ {} started: {} questions ({broken} marked OCR-flagged).",
                bank.title,
                bank.questions.len()
            ),
        };
        Ok(())
    }

    /// Son quizde yanlış cevaplanan soru sayısı.
    pub fn wrong_count(&self) -> usize {
        self.last_wrong.len()
    }

    /// Seçili plan günündeki görev sayısı.
    pub fn plan_task_count(&self) -> usize {
        plan::day(&self.course.plan, self.plan_day()).map_or(0, |info| info.tasks.len())
    }

    /// Ders daha önce üretildiyse veritabanından anında açar; yoksa AI işini başlatır.
    fn open_or_begin_lesson(
        &mut self,
        topic: &str,
        plan_key: Option<(NaiveDate, usize, usize)>,
    ) -> Result<()> {
        if !self.free() {
            return Ok(());
        }
        let subject = self.subject()?;
        if let Some((lesson_text, questions, saved_turns)) =
            self.store.load_lesson(&subject, self.lang, topic)?
        {
            if let Some((start, day, index)) = plan_key {
                plan::mark_done(&self.store, &subject, start, day, index)?;
            }
            self.store
                .set_state(&coach::taught_key(&subject, topic), "1")?;
            let suffix = self.lang.pick("(Kazanım Testi)", "(Quick Check)");
            let quiz_topic = format!("{topic} {suffix}");
            self.lesson = Some(lesson_text);
            self.lesson_topic = topic.into();
            self.lesson_turns = saved_turns
                .into_iter()
                .map(|(question, answer)| DenemeTurn {
                    question,
                    reply: CoachReply::Answer(answer),
                })
                .collect();
            self.lesson_follow = false;
            self.lesson_max_scroll.set(u16::MAX);
            self.editing = Some(Input::LessonChat);
            self.input.clear();
            let course = crate::course::CourseConfig::load_or_default(&self.root, &subject)?;
            self.session = Some(
                Session::start_at(&self.store, &subject, &quiz_topic, questions, Utc::now())?
                    .with_course(course),
            );
            self.session_subject = subject;
            self.session_tab = 4;
            self.tab = 4;
            self.feedback = None;
            self.summary.clear();
            self.summary_session = None;
            self.scroll = 0;
            self.status = self
                .lang
                .pick(
                    "✓ Kayıtlı ders anlatımı açıldı.",
                    "✓ Saved lesson explanation opened.",
                )
                .into();
            return self.refresh();
        }
        match plan_key {
            Some(key) => self.begin_plan_ai(
                JobKind::Lesson {
                    topic: topic.into(),
                },
                key,
            ),
            None => self.begin_ai(JobKind::Lesson {
                topic: topic.into(),
            }),
        }
    }

    /// Bugün sekmesinde seçili plan görevini çalıştırır. Telegram'daki ▶ düğmesiyle aynı komutlar.
    fn plan_enter(&mut self) -> Result<()> {
        if self.lesson.take().is_some() {
            self.editing = None;
            self.lesson_follow = false;
            self.scroll = 0;
            return Ok(());
        }
        if self.session.is_some() {
            return self.quiz_enter();
        }
        if self.pending.is_some() {
            self.status = self
                .lang
                .pick(
                    "Önceki istek sürüyor…",
                    "Previous request is still running…",
                )
                .into();
            return Ok(());
        }
        let subject = self.subject()?;
        let Some(start) = plan::start_date(&self.store, &subject)? else {
            self.status = self
                .lang
                .pick(
                    "Plan başlatılmadı. Bugünden başlatmak için 'b' tuşuna basın.",
                    "Study plan not started. Press 'b' to start today.",
                )
                .into();
            return Ok(());
        };
        let day = self.plan_day();
        let Some(task) = plan::task(&self.course.plan, day, self.task_selected).cloned() else {
            self.status = self
                .lang
                .pick("Görev seçilemedi.", "No task selected.")
                .into();
            return Ok(());
        };
        if task.command.is_empty() {
            // Elle yapılan görev (kitap, soru bankası): hemen işaretlenir; daha önce
            // açıklandıysa kayıttan açılır, yoksa AI görevi anlatır.
            plan::mark_done(&self.store, &subject, start, day, self.task_selected)?;
            // The coach continues with the next plan task once a manual one is done.
            if self.coach_mode == CoachMode::On {
                self.coach_due = true;
            }
            let question = bot::manual_task_question_in(&task.label, self.lang);
            if let Some(cached) = self.store.state(&format!(
                "answer_cache:{subject}:{}:{question}",
                self.lang.code()
            ))? {
                self.answer_question = Some(question.clone());
                let course_prefix = self.lang.pick("Ders", "Course");
                self.answer = format!("{course_prefix}: {subject}\n\n{question}\n\n{cached}");
                self.scroll = 0;
                self.tab = 0;
                self.status = format!(
                    "{}: {}",
                    self.lang.pick(
                        "✓ Kayıtlı görev açıklaması açıldı",
                        "✓ Saved task guide opened",
                    ),
                    task.label
                );
                return self.refresh();
            }
            self.begin_ai(JobKind::Answer { question })?;
            // begin_ai_with clears the status line, so the mark is set after it.
            self.status = format!(
                "{}: {}",
                self.lang.pick("İşaretlendi", "Marked done"),
                task.label
            );
            return Ok(());
        }
        let (name, args) = task.command.split_once(' ').unwrap_or((&task.command, ""));
        let plan_key = (start, day, self.task_selected);
        let started = match name {
            "/kelime" | "/cards" => {
                let (topic, count) = bot::vocab_request_in(args, self.lang)?;
                self.begin_plan_ai(JobKind::Vocab { topic, count }, plan_key)?;
                false
            }
            "/tekrar" | "/review" => {
                self.tab = 2;
                self.status = self
                    .lang
                    .pick(
                        "Enter: zamanı gelen en fazla 10 soruyu tekrar et.",
                        "Enter: review up to 10 due questions.",
                    )
                    .into();
                true
            }
            "/anlat" | "/explain" | "/lesson" => {
                self.open_or_begin_lesson(args, Some(plan_key))?;
                false
            }
            "/quiz" => {
                let (topic, count) = bot::quiz_request_in(args, self.lang)?;
                self.begin_plan_ai(JobKind::Quiz { topic, count }, plan_key)?;
                false
            }
            "/kazanim" | "/practice" => {
                let (topic, count) = bot::kazanim_request_in(args, self.lang)?;
                let suffix = self.lang.pick("(Kazanım Testi)", "(Quick Check)");
                self.begin_plan_ai(
                    JobKind::Quiz {
                        topic: format!("{topic} {suffix}"),
                        count,
                    },
                    plan_key,
                )?;
                false
            }
            "/deneme" | "/exam" => {
                if self.start_unused_bank_exam(&subject)? {
                    true
                } else {
                    let course =
                        crate::course::CourseConfig::load_or_default(&self.root, &subject)?;
                    let count = bot::deneme_count_for_course(&course, args, self.lang)?;
                    self.begin_plan_ai(JobKind::Deneme { count }, plan_key)?;
                    false
                }
            }
            _ => {
                self.status = self
                    .lang
                    .pick(
                        "Bu görev TUI'de çalıştırılamıyor; Telegram'dan deneyin.",
                        "This task is not supported in the TUI; run it from Telegram.",
                    )
                    .into();
                return Ok(());
            }
        };
        if started {
            plan::mark_done(&self.store, &subject, start, day, self.task_selected)?;
        }
        Ok(())
    }

    fn quiz_enter(&mut self) -> Result<()> {
        if self.session.is_some() && self.session_tab != self.tab {
            self.status = self
                .lang
                .pick(
                    "Diğer sekmede aktif quiz var. Esc ile bitirin.",
                    "An active quiz is open in another tab. Press Esc to finish it.",
                )
                .into();
            return Ok(());
        }
        if let Some(session) = &self.session {
            if self.feedback.is_some() {
                if session.position == session.questions.len() {
                    self.last_wrong = session.wrong_questions();
                    let (topic, correct, answered) =
                        (session.topic.clone(), session.correct(), session.position);
                    self.summary = session.finish_in(&self.store, self.lang, false)?;
                    self.summary_session = Some(session.id);
                    self.session = None;
                    self.coach_after_session(&topic, correct, answered);
                }
                self.feedback = None;
                self.scroll = 0;
                self.refresh()?;
            }
            return Ok(());
        }
        if self.pending.is_some() {
            self.status = self
                .lang
                .pick(
                    "Önceki istek sürüyor…",
                    "Previous request is still running…",
                )
                .into();
            return Ok(());
        }
        let subject = self.subject()?;
        let (topic, questions) = if self.tab == 2 {
            (
                self.lang.pick("tekrar", "review"),
                self.store.due_original_reviews(&subject, 10, Utc::now())?,
            )
        } else {
            (
                self.lang.pick("Kayıtlı sorular", "Saved questions"),
                self.store.unanswered(&subject)?,
            )
        };
        if questions.is_empty() {
            self.status = if self.tab == 2 {
                match self.store.next_original_review(&subject)? {
                    Some(date) => match self.lang {
                        Lang::Tr => format!("Bugün tekrar yok. Sonraki tekrar: {date} (UTC)."),
                        Lang::En => format!("No reviews due today. Next review: {date} (UTC)."),
                    },
                    None => self
                        .lang
                        .pick(
                            "Tekrar edilecek soru yok. Planlanmış tekrar tarihi yok.",
                            "No questions to review. No review scheduled.",
                        )
                        .into(),
                }
            } else {
                self.lang
                    .pick(
                        "Cevaplanmamış soru yok. n ile yeni quiz üretin.",
                        "No unanswered questions. Press n to generate a quiz.",
                    )
                    .into()
            };
            return Ok(());
        }
        self.status.clear();
        self.start_session(&subject, topic, questions, self.tab)
    }

    /// Sor metni ve `/` komutları, Telegram ile aynı sırayla: komut, bekleyen konu, doğal dil
    /// isteği ya da AI sorusu.
    fn route(&mut self, text: &str) -> Result<()> {
        let text = text.trim();
        let awaiting = self.awaiting.take();
        if text.is_empty() {
            return Ok(());
        }
        if let Some(command) = text.strip_prefix('/') {
            let (name, args) = command
                .split_once(char::is_whitespace)
                .unwrap_or((command, ""));
            return self.command(&name.to_lowercase(), args.trim());
        }
        match awaiting {
            Some("anlat") => return self.open_or_begin_lesson(text, None),
            Some(_) => {
                let (topic, count) = bot::vocab_request_in(text, self.lang)?;
                return self.begin_ai(JobKind::Vocab { topic, count });
            }
            None => {}
        }
        if self.session.is_none() && bot::text_choice(text).is_some() {
            self.status = self
                .lang
                .pick(
                    "Şu an aktif quiz yok. Soru çözmek için /quiz ya da /deneme yaz.",
                    "No active quiz right now. Type /quiz or /exam to start.",
                )
                .into();
            return Ok(());
        }
        if let Some((count, is_exact)) = bot::detect_review_request(text) {
            return self.review(count, is_exact);
        }
        if bot::detect_deneme_history_request(text) {
            return self.history();
        }
        if let Some(count) = bot::detect_deneme_request_for_course(&self.course, text) {
            return self.deneme(count);
        }
        if let Some((topic, count)) = bot::detect_kazanim_request(text) {
            return self.kazanim(&topic, count);
        }
        if let Some((topic, count)) = bot::detect_quiz_request(text) {
            return self.quiz(&topic, count);
        }
        if let Some(topic) = bot::detect_lesson_request(text) {
            return self.open_or_begin_lesson(&topic, None);
        }
        self.begin_ai(JobKind::Answer {
            question: text.into(),
        })
    }

    pub(crate) fn command(&mut self, name: &str, args: &str) -> Result<()> {
        let name = name.strip_prefix('/').unwrap_or(name);
        match name {
            "yardim" | "start" | "help" => self.help = true,
            "lang" => match Lang::parse(args) {
                Some(new_lang) => {
                    self.lang = new_lang;
                    self.store.set_state("lang", new_lang.code())?;
                    self.status = new_lang
                        .pick(
                            "✓ Dil Türkçe olarak ayarlandı.",
                            "✓ Language switched to English.",
                        )
                        .into();
                    self.refresh()?;
                }
                None => {
                    self.status = self
                        .lang
                        .pick("Kullanım: /lang tr|en", "Usage: /lang tr|en")
                        .into();
                }
            },
            "bugun" | "today" => {
                self.tab = 4;
                self.plan_today();
                self.scroll = 0;
                if self.lesson.is_some() {
                    self.editing = Some(Input::LessonChat);
                }
            }
            "plan" => {
                let subject = self.subject()?;
                let course = crate::course::CourseConfig::load_or_default(&self.root, &subject)?;
                let len = course.plan.len();
                let (sub, rest) = args
                    .trim()
                    .split_once(char::is_whitespace)
                    .unwrap_or((args.trim(), ""));
                if matches!(sub, "gun" | "gec" | "day" | "jump" | "goto")
                    && let Ok(number) = rest.trim().parse::<usize>()
                    && (1..=len).contains(&number)
                {
                    self.plan_view_day = Some(number);
                    self.task_selected = 0;
                }
                let text = bot::plan_text_in(
                    &self.store,
                    &subject,
                    &course.plan,
                    args,
                    Local::now().date_naive(),
                    self.lang,
                )?;
                self.show(&text);
                self.refresh()?;
            }
            "anlat" | "explain" | "lesson" if args.is_empty() => self.ask_topic("anlat"),
            "anlat" | "explain" | "lesson" => self.open_or_begin_lesson(args, None)?,
            "quiz" if args.is_empty() => self.saved_quiz()?,
            "quiz" => {
                let (topic, count) = bot::quiz_request_in(args, self.lang)?;
                self.quiz(&topic, count)?;
            }
            "tekrar" | "review" => {
                let (count, is_exact) = bot::review_request_in(args, self.lang)?;
                self.review(count, is_exact)?;
            }
            "kazanim" | "practice" => {
                let (topic, count) = bot::kazanim_request_in(args, self.lang)?;
                self.kazanim(&topic, count)?;
            }
            "kelime" | "cards" if args.is_empty() => self.ask_topic("kelime"),
            "kelime" | "cards" => {
                let (topic, count) = bot::vocab_request_in(args, self.lang)?;
                self.begin_ai(JobKind::Vocab { topic, count })?;
            }
            "deneme" | "exam" => {
                let subject = self.subject()?;
                let course = crate::course::CourseConfig::load_or_default(&self.root, &subject)?;
                let count = bot::deneme_count_for_course(&course, args, self.lang)?;
                self.deneme(count)?;
            }
            "denemeler" | "exams" if args.is_empty() => self.history()?,
            "denemeler" | "exams" => {
                let subject = self.subject()?;
                let text = bot::deneme_analysis_request_for(
                    &self.store,
                    Some(&self.course),
                    &subject,
                    args,
                    self.lang,
                )?;
                self.show(&text);
            }
            "dersler" | "courses" | "yanlislar" | "mistakes" => {
                let wrong = matches!(name, "yanlislar" | "mistakes");
                let text = bot::subjects_text_in(&self.store, &self.root, wrong, self.lang)?;
                self.show(&text);
            }
            "ders" | "course" => self.select_subject(args)?,
            "iptal" | "cancel" => {
                if self.session.is_some() {
                    self.end_session_early()?;
                } else if self.pending.is_some() {
                    self.cancel_pending()?;
                } else {
                    self.status = self
                        .lang
                        .pick("Aktif quiz yok.", "No active quiz session.")
                        .into();
                }
            }
            "test" => self.sample_quiz()?,
            _ => {
                self.status = self
                    .lang
                    .pick(
                        "Bilinmeyen komut. /yardim kullanın.",
                        "Unknown command. Use /help.",
                    )
                    .into();
            }
        }
        Ok(())
    }

    /// Yeni quiz, tekrar ya da deneme başlamadan önce: açık oturum ya da bekleyen iş olmamalı.
    fn free(&mut self) -> bool {
        if self.session.is_some() {
            self.status = self
                .lang
                .pick(
                    "Aktif quiz var. Önce /iptal ile bitirin.",
                    "A quiz session is active. Finish it first with /cancel.",
                )
                .into();
            false
        } else if self.pending.is_some() {
            self.status = self
                .lang
                .pick(
                    "Önceki istek sürüyor…",
                    "Previous request is still running…",
                )
                .into();
            false
        } else {
            true
        }
    }

    fn ask_topic(&mut self, command: &'static str) {
        self.awaiting = Some(command);
        self.status = if command == "anlat" {
            self.lang
                .pick(
                    "Hangi konuyu anlatayım? Konuyu yaz.",
                    "Which topic should I explain? Type it below.",
                )
                .into()
        } else {
            self.lang
                .pick(
                    "Hangi konu için kart hazırlayayım? Konuyu yaz (ör. Çevre 30).",
                    "Which topic for flashcards? Type topic [count] (e.g. Memory 20).",
                )
                .into()
        };
    }

    fn quiz(&mut self, topic: &str, count: usize) -> Result<()> {
        if self.free() {
            self.begin_ai(JobKind::Quiz {
                topic: topic.into(),
                count,
            })?;
        }
        Ok(())
    }

    fn kazanim(&mut self, topic: &str, count: usize) -> Result<()> {
        if self.free() {
            let suffix = self.lang.pick("(Kazanım Testi)", "(Quick Check)");
            self.begin_ai(JobKind::Quiz {
                topic: format!("{topic} {suffix}"),
                count,
            })?;
        }
        Ok(())
    }

    fn review(&mut self, count: usize, is_exact: bool) -> Result<()> {
        if !self.free() {
            return Ok(());
        }
        let subject = self.subject()?;
        let questions = if is_exact {
            self.store
                .due_original_reviews(&subject, count, Utc::now())?
        } else {
            self.store.due_reviews(&subject, count, Utc::now())?
        };
        if questions.is_empty() {
            let next = if is_exact {
                self.store.next_original_review(&subject)?
            } else {
                self.store.next_review(&subject)?
            };
            self.status = match next {
                Some(date) => match self.lang {
                    Lang::Tr => format!("Bugün tekrar yok. Sonraki tekrar: {date} (UTC)."),
                    Lang::En => format!("No reviews due today. Next review: {date} (UTC)."),
                },
                None => self
                    .lang
                    .pick(
                        "Tekrar edilecek soru yok. Planlanmış tekrar tarihi yok.",
                        "No questions to review. No review scheduled.",
                    )
                    .into(),
            };
        } else if is_exact {
            self.start_session(
                &subject,
                self.lang.pick("tekrar (orijinal)", "review (original)"),
                questions,
                2,
            )?;
            self.tab = 2;
        } else {
            self.begin_ai(JobKind::ReviewVariations {
                due_questions: questions,
            })?;
        }
        Ok(())
    }

    fn deneme(&mut self, count: usize) -> Result<()> {
        if !self.free() {
            return Ok(());
        }
        let subject = self.subject()?;
        if count == self.course.exam_question_count() && self.start_unused_bank_exam(&subject)? {
            return Ok(());
        }
        self.begin_ai(JobKind::Deneme { count })
    }

    fn history(&mut self) -> Result<()> {
        let subject = self.subject()?;
        let text =
            bot::deneme_history_text_for(&self.store, Some(&self.root), &subject, self.lang)?;
        self.show(&text);
        Ok(())
    }

    fn saved_quiz(&mut self) -> Result<()> {
        if !self.free() {
            return Ok(());
        }
        let subject = self.subject()?;
        let questions = self.store.unanswered(&subject)?;
        if questions.is_empty() {
            self.status = self
                .lang
                .pick(
                    "Cevaplanmamış soru yok. /quiz <konu> <n> ile soru üretin.",
                    "No unanswered questions. Generate some with /quiz <topic> <n>.",
                )
                .into();
        } else {
            self.start_session(
                &subject,
                self.lang.pick("Kayıtlı sorular", "Saved questions"),
                questions,
                1,
            )?;
            self.tab = 1;
        }
        Ok(())
    }

    fn sample_quiz(&mut self) -> Result<()> {
        if !self.free() {
            return Ok(());
        }
        let subject = self.subject()?;
        let stored =
            self.store
                .add_questions(&subject, "Test", vec![bot::sample_question_in(self.lang)])?;
        self.start_session(&subject, "Test", stored, 1)?;
        self.tab = 1;
        Ok(())
    }

    fn select_subject(&mut self, name: &str) -> Result<()> {
        if self.session.is_some() || self.pending.is_some() {
            self.status = self
                .lang
                .pick(
                    "Ders değiştirmeden önce /iptal ile bitirin.",
                    "Finish the active task with /cancel before switching courses.",
                )
                .into();
            return Ok(());
        }
        let trimmed = name.trim();
        let Some(index) = self
            .subjects
            .iter()
            .position(|subject| subject.name == trimmed)
            .or_else(|| {
                self.subjects
                    .iter()
                    .position(|subject| subject.name.eq_ignore_ascii_case(trimmed))
            })
        else {
            self.status = self
                .lang
                .pick(
                    "Ders bulunamadı. /dersler ile adları görün.",
                    "Course not found. Run /courses to list available courses.",
                )
                .into();
            return Ok(());
        };
        let matched = self.subjects[index].name.clone();
        self.selected = index;
        self.store.set_state("active_subject", &matched)?;
        self.plan_view_day = None;
        self.task_selected = 0;
        self.coach_step = None;
        self.coach_ready = None;
        self.deneme_chat = None;
        self.lesson = None;
        self.refresh()?;
        self.status = format!(
            "{}: {matched}",
            self.lang.pick("✓ Aktif ders", "✓ Active course")
        );
        Ok(())
    }

    /// Metni Sor sekmesinin yanıt alanında gösterir (Telegram'daki mesajın karşılığı).
    fn show(&mut self, text: &str) {
        self.answer = text.replace("**", "");
        self.answer_question = None;
        self.tab = 0;
        self.scroll = 0;
    }

    /// Oturumu erken bitirir (Esc ve `/iptal`). Yarıda kesmek koçu duraklatır; F3 devam ettirir.
    fn end_session_early(&mut self) -> Result<()> {
        let interrupted = self.session.is_some() && self.coach_mode == CoachMode::On;
        self.input.clear();
        self.close_session()?;
        if interrupted {
            self.set_coach_mode(CoachMode::Paused)?;
        }
        Ok(())
    }

    fn submit(&mut self, input: Input) -> Result<()> {
        match input {
            Input::Answer => {
                let text = self.input.trim().to_string();
                self.input.clear();
                self.editing = None;
                self.route(&text)?;
            }
            Input::Topic => {
                self.topic = if self.input.trim().is_empty() {
                    self.lang.pick("Genel", "General").into()
                } else {
                    self.input.trim().into()
                };
                self.input.clear();
                self.editing = Some(Input::Count);
            }
            Input::Count => {
                let value = if self.input.trim().is_empty() {
                    "10"
                } else {
                    self.input.trim()
                };
                let Some(count) = value.parse::<usize>().ok().filter(|n| (1..=30).contains(n))
                else {
                    self.status = self
                        .lang
                        .pick(
                            "Soru sayısı 1–30 arasında olmalı.",
                            "Question count must be between 1 and 30.",
                        )
                        .into();
                    return Ok(());
                };
                let (topic, count) =
                    bot::quiz_request_in(&format!("{} {count}", self.topic), self.lang)?;
                self.begin_ai(JobKind::Quiz { topic, count })?;
                self.input.clear();
                self.editing = None;
            }
            Input::DenemeChat => self.send_deneme_chat(ai_generate(&self.root))?,
            Input::LessonChat => self.send_lesson_chat(ai_generate(&self.root))?,
        }
        Ok(())
    }

    /// Sekme değiştirir. Denemeler sekmesi her açılışta güncel listeyi, liste görünümünü gösterir.
    fn select_tab(&mut self, tab: usize) -> Result<()> {
        self.tab = tab;
        self.scroll = 0;
        if tab == DENEME_TAB {
            self.deneme_selected = 0;
            self.deneme_chat = None;
            self.deneme_delete = None;
            self.load_denemeler()?;
        } else if tab == 4 && self.lesson.is_some() {
            self.editing = Some(Input::LessonChat);
        }
        Ok(())
    }

    fn load_denemeler(&mut self) -> Result<()> {
        let Ok(subject) = self.subject() else {
            self.deneme_items.clear();
            self.deneme_selected = 0;
            return Ok(());
        };
        self.deneme_items = self.store.deneme_sessions(&subject, 10)?;
        self.deneme_selected = self
            .deneme_selected
            .min(self.deneme_items.len().saturating_sub(1));
        Ok(())
    }

    /// Denemeler'de Enter: seçili denemenin dökümünü açar ve koçun ilk değerlendirmesini ister.
    fn open_deneme_chat(
        &mut self,
        generate: impl FnOnce(String, String) -> Result<String> + Send + 'static,
    ) -> Result<()> {
        if self.deneme_selected >= self.deneme_items.len() {
            self.status = self
                .lang
                .pick("Henüz tamamlanmış deneme yok.", "No completed exams yet.")
                .into();
            return Ok(());
        }
        let subject = self.subject()?;
        let item = &self.deneme_items[self.deneme_selected];
        let label = match self.lang {
            Lang::Tr => format!(
                "Deneme {}: {}/{} doğru",
                self.deneme_selected + 1,
                item.correct,
                item.total
            ),
            Lang::En => format!(
                "Exam {}: {}/{} correct",
                self.deneme_selected + 1,
                item.correct,
                item.total
            ),
        };
        let chat = DenemeChat {
            session: item.id,
            label,
            breakdown: bot::deneme_analysis_by_id_for(
                &self.store,
                Some(&self.course),
                &subject,
                item.id,
                self.lang,
            )?,
            turns: vec![],
            wrongs: bot::deneme_wrongs_for(&self.store, Some(&self.course), item, self.lang)?,
            focus: None,
            follow: true,
            scroll: 0,
            max_scroll: Cell::new(0),
        };
        self.deneme_chat = Some(chat);
        self.editing = Some(Input::DenemeChat);
        self.input.clear();
        self.deneme_turn(String::new(), None, generate)
    }

    /// Koça yeni bir tur gönderir; boş soru ilk değerlendirmedir. `focus` bu turda konuşulan yanlışın
    /// dizinidir ve yalnızca iş başlarsa sohbetin odağı olur. Başka bir AI işi sürüyorsa ya da iş
    /// başlamazsa tur eklenmez, odak değişmez. Kutudaki metne dokunmaz.
    fn deneme_turn(
        &mut self,
        question: String,
        focus: Option<usize>,
        generate: impl FnOnce(String, String) -> Result<String> + Send + 'static,
    ) -> Result<()> {
        if self.pending.is_some() {
            self.status = self
                .lang
                .pick(
                    "Önceki istek sürüyor…",
                    "Previous request is still running…",
                )
                .into();
            return Ok(());
        }
        let Some(chat) = self.deneme_chat.as_mut() else {
            return Ok(());
        };
        let kind = JobKind::DenemeCoach {
            session: chat.session,
            question: question.clone(),
            history: coach_history(chat, self.lang),
            focus: focus
                .and_then(|index| chat.wrongs.get(index))
                .map(|wrong| wrong.number),
        };
        chat.turns.push(DenemeTurn {
            question,
            reply: CoachReply::Waiting,
        });
        let result = self.begin_ai_with(kind, generate);
        if result.is_err() || self.pending.is_none() {
            if let Some(chat) = self.deneme_chat.as_mut() {
                chat.turns.pop();
            }
            return result;
        }
        if let Some(chat) = self.deneme_chat.as_mut() {
            chat.focus = focus;
            chat.follow = true;
        }
        result
    }

    /// Koç sohbetinde bir adım kaydırır. Takip açıkken önce en alta sabitlenir; en alta inilince
    /// takip yeniden açılır.
    fn scroll_chat(&mut self, delta: i32) {
        let Some(chat) = self.deneme_chat.as_mut() else {
            return;
        };
        let max = chat.max_scroll.get();
        let current = if chat.follow {
            max
        } else {
            chat.scroll.min(max)
        };
        let next = (i32::from(current) + delta).clamp(0, i32::from(max)) as u16;
        chat.scroll = next;
        chat.follow = next >= max;
    }

    /// Koç sohbetinde Enter. Yalnızca rakamlardan oluşan ve yanlışlardan biri olan bir numara o soruya
    /// geçer; diğer yazılar serbest soru olarak, o an odaktaki soruyla birlikte gider.
    fn send_deneme_chat(
        &mut self,
        generate: impl FnOnce(String, String) -> Result<String> + Send + 'static,
    ) -> Result<()> {
        let text = self.input.trim().to_string();
        let Some(chat) = self.deneme_chat.as_ref() else {
            return Ok(());
        };
        if text.is_empty() {
            return Ok(());
        }
        let jump = text
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| text.parse::<usize>().ok())
            .flatten()
            .and_then(|number| chat.wrongs.iter().position(|wrong| wrong.number == number));
        let (question, focus) = match jump {
            Some(index) => {
                let number = chat.wrongs[index].number;
                (
                    match self.lang {
                        Lang::Tr => format!("Soru {number}'yi birlikte inceleyelim."),
                        Lang::En => format!("Let's examine Question {number} together."),
                    },
                    Some(index),
                )
            }
            None => (text, chat.focus),
        };
        let idle = self.pending.is_none();
        self.deneme_turn(question, focus, generate)?;
        if idle && self.pending.is_some() {
            self.input.clear();
        }
        Ok(())
    }

    /// Tab / Shift+Tab: yanlış sorular arasında bir adım gider ve o soruyu koçla birlikte inceletir.
    /// Uçlarda sarmalamaz; durum satırı bunu söyler.
    fn step_wrong(
        &mut self,
        forward: bool,
        generate: impl FnOnce(String, String) -> Result<String> + Send + 'static,
    ) -> Result<()> {
        let Some(chat) = self.deneme_chat.as_ref() else {
            return Ok(());
        };
        let (count, focus) = (chat.wrongs.len(), chat.focus);
        if count == 0 {
            self.status = self
                .lang
                .pick("Bu denemede yanlış yok.", "No mistakes in this exam.")
                .into();
            return Ok(());
        }
        let target = match focus {
            None if forward => Some(0),
            None => Some(count - 1),
            Some(index) if forward => (index + 1 < count).then_some(index + 1),
            Some(index) => index.checked_sub(1),
        };
        let Some(target) = target else {
            self.status = if forward {
                self.lang.pick("Son yanlış soru.", "Last mistake.")
            } else {
                self.lang.pick("İlk yanlış soru.", "First mistake.")
            }
            .into();
            return Ok(());
        };
        let number = chat.wrongs[target].number;
        let q = match self.lang {
            Lang::Tr => format!("Soru {number}'yi birlikte inceleyelim."),
            Lang::En => format!("Let's examine Question {number} together."),
        };
        self.deneme_turn(q, Some(target), generate)
    }

    /// Koç yanıtını bekleyen son tura yazar. Sohbet kapanmış ya da başka bir denemeye geçilmişse
    /// sonuç atılır.
    fn finish_deneme_chat(&mut self, session: i64, reply: CoachReply) {
        let Some(chat) = self
            .deneme_chat
            .as_mut()
            .filter(|chat| chat.session == session)
        else {
            return;
        };
        if let Some(turn) = chat
            .turns
            .last_mut()
            .filter(|turn| turn.reply == CoachReply::Waiting)
        {
            turn.reply = reply;
        }
    }

    /// Ders anlatımı ekranında kaydırır.
    fn scroll_lesson(&mut self, delta: i32) {
        if self.lesson.is_none() {
            return;
        }
        let max = self.lesson_max_scroll.get();
        let current = if self.lesson_follow {
            max
        } else {
            self.scroll.min(max)
        };
        let next = (i32::from(current) + delta).clamp(0, i32::from(max)) as u16;
        self.scroll = next;
        self.lesson_follow = max < u16::MAX && next >= max;
    }

    /// Ders anlatımında Enter: kutu boşsa quize geçer, doluysa soruyu AI'ye gönderir.
    fn send_lesson_chat(
        &mut self,
        generate: impl FnOnce(String, String) -> Result<String> + Send + 'static,
    ) -> Result<()> {
        let text = self.input.trim().to_string();
        if self.lesson.is_none() {
            self.editing = None;
            return Ok(());
        }
        if text.is_empty() {
            self.lesson = None;
            self.editing = None;
            self.lesson_follow = false;
            self.scroll = 0;
            return Ok(());
        }
        let idle = self.pending.is_none();
        self.lesson_turn(text, generate)?;
        if idle && self.pending.is_some() {
            self.input.clear();
        }
        Ok(())
    }

    /// Ders anlatımı altında yeni bir soru-cevap turu başlatır.
    fn lesson_turn(
        &mut self,
        question: String,
        generate: impl FnOnce(String, String) -> Result<String> + Send + 'static,
    ) -> Result<()> {
        if self.pending.is_some() {
            self.status = self
                .lang
                .pick(
                    "Önceki istek sürüyor…",
                    "Previous request is still running…",
                )
                .into();
            return Ok(());
        }
        let Some(lesson) = self.lesson.clone() else {
            return Ok(());
        };
        let kind = JobKind::LessonChat {
            topic: self.lesson_topic.clone(),
            lesson,
            question: question.clone(),
            history: lesson_history(&self.lesson_turns),
        };
        self.lesson_turns.push(DenemeTurn {
            question,
            reply: CoachReply::Waiting,
        });
        let result = self.begin_ai_with(kind, generate);
        if result.is_err() || self.pending.is_none() {
            self.lesson_turns.pop();
            return result;
        }
        self.lesson_follow = true;
        result
    }

    /// Ders sohbeti yanıtını son tura yazar ve güncel sohbet geçmişini veritabanına kaydeder.
    fn finish_lesson_chat(&mut self, subject: &str, topic: &str, reply: CoachReply) -> Result<()> {
        if self.lesson.is_none() || self.lesson_topic != topic {
            return Ok(());
        }
        if let Some(turn) = self
            .lesson_turns
            .last_mut()
            .filter(|turn| turn.reply == CoachReply::Waiting)
        {
            turn.reply = reply;
        }
        if let (Some(lesson), Some(session)) = (&self.lesson, &self.session) {
            let turns: Vec<(String, String)> = self
                .lesson_turns
                .iter()
                .filter_map(|turn| match &turn.reply {
                    CoachReply::Answer(answer) => Some((turn.question.clone(), answer.clone())),
                    _ => None,
                })
                .collect();
            self.store.save_lesson(
                subject,
                self.lang,
                topic,
                lesson,
                &session.questions,
                &turns,
            )?;
        }
        Ok(())
    }

    /// `d`: seçili denemeyi silmek için onay ister; bir sonraki tuş karar verir.
    fn ask_deneme_delete(&mut self) {
        let Some(item) = self.deneme_items.get(self.deneme_selected) else {
            self.status = self
                .lang
                .pick("Silinecek deneme yok.", "No exam to delete.")
                .into();
            return;
        };
        self.deneme_delete = Some(item.id);
        self.status = match self.lang {
            Lang::Tr => format!(
                "Deneme {} silinsin mi? Geri alınamaz; istatistiklerden de çıkar. y: evet · başka tuş: vazgeç",
                self.deneme_selected + 1
            ),
            Lang::En => format!(
                "Delete Exam {}? This cannot be undone and removes it from stats. y: yes · any key: cancel",
                self.deneme_selected + 1
            ),
        };
    }

    /// Silme onayı: yalnızca `y` siler; diğer tuşlar iptal eder.
    fn finish_deneme_delete(&mut self, code: KeyCode) -> Result<()> {
        let Some(id) = self.deneme_delete.take() else {
            return Ok(());
        };
        if code != KeyCode::Char('y') {
            self.status = self
                .lang
                .pick("Silme iptal edildi.", "Deletion cancelled.")
                .into();
            return Ok(());
        }
        if let Some(item) = self.deneme_items.iter().find(|item| item.id == id) {
            bot::delete_deneme(&mut self.store, &self.root, item)?;
        }
        self.load_denemeler()?;
        self.refresh()?;
        self.status = self
            .lang
            .pick("✓ Deneme silindi.", "✓ Exam deleted.")
            .into();
        Ok(())
    }

    pub fn key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }
        // Each keypress is an action of its own: the previous message goes away.
        self.status.clear();
        self.status_seen.clear();
        // Windows reports AltGr as Ctrl+Alt, and AltGr types characters (ş, ı, i …).
        // Only plain Ctrl is a shortcut.
        let control = key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        if self.help {
            if matches!(key.code, KeyCode::F(1) | KeyCode::Char('?') | KeyCode::Esc) {
                self.help = false;
            }
            return Ok(());
        }
        if let Some(input) = self.editing {
            match key.code {
                // Koç sohbetinde Esc önce yazılanı siler, boşsa sohbetten çıkar; ↑/↓ ve PgUp/PgDn kaydırır.
                KeyCode::Esc if input == Input::DenemeChat && !self.input.is_empty() => {
                    self.input.clear();
                }
                KeyCode::Esc if input == Input::DenemeChat => {
                    self.deneme_chat = None;
                    self.editing = None;
                    self.scroll = 0;
                }
                KeyCode::Up if input == Input::DenemeChat => self.scroll_chat(-1),
                KeyCode::Down if input == Input::DenemeChat => self.scroll_chat(1),
                KeyCode::PageUp if input == Input::DenemeChat => self.scroll_chat(-10),
                KeyCode::PageDown if input == Input::DenemeChat => self.scroll_chat(10),
                // Ders anlatımı sohbetinde Esc önce yazılanı siler, boşsa dersi kapatır; ↑/↓ ve PgUp/PgDn kaydırır.
                KeyCode::Esc if input == Input::LessonChat && !self.input.is_empty() => {
                    self.input.clear();
                }
                KeyCode::Esc if input == Input::LessonChat && self.pending.is_some() => {
                    self.cancel_pending()?;
                }
                KeyCode::Esc if input == Input::LessonChat => {
                    self.editing = None;
                    self.end_session_early()?;
                }
                KeyCode::Up if input == Input::LessonChat => self.scroll_lesson(-1),
                KeyCode::Down if input == Input::LessonChat => self.scroll_lesson(1),
                KeyCode::PageUp if input == Input::LessonChat => self.scroll_lesson(-10),
                KeyCode::PageDown if input == Input::LessonChat => self.scroll_lesson(10),
                KeyCode::Home if input == Input::LessonChat => {
                    self.scroll = 0;
                    self.lesson_follow = false;
                }
                KeyCode::Up if input == Input::Answer => {
                    self.scroll = self.scroll.saturating_sub(1);
                }
                KeyCode::Down if input == Input::Answer => {
                    self.scroll = self.scroll.saturating_add(1);
                }
                KeyCode::Esc => {
                    self.input.clear();
                    self.editing = None;
                    self.status.clear();
                }
                KeyCode::Enter => self.submit(input)?,
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Char('u') if control => self.input.clear(),
                KeyCode::Char(_) if control => self.status = ignored_key(&key, self.lang),
                KeyCode::Char(ch) => self.input.push(ch),
                KeyCode::Tab if matches!(input, Input::Answer | Input::LessonChat) => {
                    self.editing = None;
                    self.select_tab((self.tab + 1) % TABS.len())?;
                }
                KeyCode::BackTab if input == Input::LessonChat => {
                    self.editing = None;
                    self.select_tab((self.tab + TABS.len() - 1) % TABS.len())?;
                }
                KeyCode::Tab if input == Input::DenemeChat => {
                    self.step_wrong(true, ai_generate(&self.root))?
                }
                KeyCode::BackTab if input == Input::DenemeChat => {
                    self.step_wrong(false, ai_generate(&self.root))?
                }
                KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
                KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
                _ => {}
            }
            return Ok(());
        }
        match key.code {
            KeyCode::F(1) | KeyCode::Char('?') => {
                self.help = true;
                return Ok(());
            }
            KeyCode::F(2) => return self.toggle_listener(),
            KeyCode::F(3) => return self.toggle_coach(),
            KeyCode::F(4) => return self.toggle_lang(),
            _ => {}
        }
        // Ctrl+Q is checked before the quit key: Ctrl+Q must not quit.
        if control && key.code == KeyCode::Char('q') {
            return if self.tab == 0 {
                self.quiz_from_answer()
            } else {
                Ok(())
            };
        }
        if (key.code == KeyCode::Char('q') && !control)
            || (control && key.code == KeyCode::Char('c'))
        {
            self.quit = true;
            return Ok(());
        }
        if control {
            if let KeyCode::Char(_) = key.code {
                self.status = ignored_key(&key, self.lang);
            }
            return Ok(());
        }
        if self.deneme_delete.is_some() {
            return self.finish_deneme_delete(key.code);
        }
        let choice = match key.code {
            KeyCode::Char(ch @ 'a'..='e') => Some((ch as u8 - b'a') as usize),
            KeyCode::Char(ch @ 'A'..='E') => Some((ch as u8 - b'A') as usize),
            KeyCode::Char(ch @ '1'..='5') => Some((ch as u8 - b'1') as usize),
            _ => None,
        };
        if self.tab == self.session_tab
            && self.feedback.is_none()
            && self.lesson.is_none()
            && let Some(choice) = choice
            && let Some(session) = self.session.as_mut()
            && session
                .questions
                .get(session.position)
                .is_some_and(|question| choice < question.value.choices.len())
        {
            let feedback = session.answer_in(&mut self.store, choice, self.lang)?;
            self.feedback = Some(feedback);
            self.scroll = 0;
            self.refresh()?;
            return Ok(());
        }
        match key.code {
            KeyCode::Tab => self.select_tab((self.tab + 1) % TABS.len())?,
            KeyCode::BackTab => self.select_tab((self.tab + TABS.len() - 1) % TABS.len())?,
            KeyCode::Char(ch @ '1'..='6') => self.select_tab((ch as u8 - b'1') as usize)?,
            KeyCode::Up | KeyCode::Down
                if self.tab == DENEME_TAB && !self.deneme_items.is_empty() =>
            {
                let len = self.deneme_items.len();
                self.deneme_selected = if key.code == KeyCode::Down {
                    (self.deneme_selected + 1) % len
                } else {
                    (self.deneme_selected + len - 1) % len
                };
            }
            KeyCode::Up if self.tab == 4 && self.lesson.is_some() => self.scroll_lesson(-1),
            KeyCode::Down if self.tab == 4 && self.lesson.is_some() => self.scroll_lesson(1),
            KeyCode::Up if self.session.is_some() && self.tab == self.session_tab => {
                self.scroll = self.scroll.saturating_sub(1);
            }
            KeyCode::Down if self.session.is_some() && self.tab == self.session_tab => {
                self.scroll = self.scroll.saturating_add(1);
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('[')
                if self.tab == 4 && self.lesson.is_none() && self.session.is_none() =>
            {
                self.plan_prev_day();
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(']')
                if self.tab == 4 && self.lesson.is_none() && self.session.is_none() =>
            {
                self.plan_next_day();
            }
            KeyCode::Up | KeyCode::Down
                if self.tab == 4
                    && self.lesson.is_none()
                    && self.session.is_none()
                    && self.plan_task_count() > 0 =>
            {
                let len = self.plan_task_count();
                self.task_selected = if key.code == KeyCode::Down {
                    (self.task_selected + 1) % len
                } else {
                    (self.task_selected + len - 1) % len
                };
            }
            KeyCode::Up | KeyCode::Down if !self.subjects.is_empty() => {
                let len = self.subjects.len();
                self.selected = if key.code == KeyCode::Down {
                    (self.selected + 1) % len
                } else {
                    (self.selected + len - 1) % len
                };
                self.store
                    .set_state("active_subject", &self.subjects[self.selected].name)?;
                self.input.clear();
                self.scroll = 0;
                self.plan_view_day = None;
                self.task_selected = 0;
                self.coach_step = None;
                self.coach_ready = None;
                self.deneme_chat = None;
                self.lesson = None;
                self.refresh()?;
            }
            KeyCode::Char('r') => self.begin_index()?,
            KeyCode::Char('n') if self.tab == 1 => {
                if self.session.is_some() || self.pending.is_some() {
                    self.status = self
                        .lang
                        .pick(
                            "Önce aktif quiz veya isteği bitirin.",
                            "Finish the active quiz or task first.",
                        )
                        .into();
                } else {
                    self.editing = Some(Input::Topic);
                    self.input.clear();
                    self.status.clear();
                }
            }
            KeyCode::Esc if self.pending.is_some() => self.cancel_pending()?,
            KeyCode::Char('w') if self.tab == 1 && self.session.is_none() => self.retry_wrong()?,
            KeyCode::Char('t') if self.tab == 2 && self.session.is_none() => self.quiz_enter()?,
            KeyCode::Char('b')
                if self.tab == 4 && self.session.is_none() && self.lesson.is_none() =>
            {
                self.plan_start()?
            }
            KeyCode::Enter if matches!(self.tab, 1 | 2) => self.quiz_enter()?,
            KeyCode::Enter if self.tab == 4 => self.plan_enter()?,
            KeyCode::Enter if self.tab == DENEME_TAB => {
                self.open_deneme_chat(ai_generate(&self.root))?
            }
            KeyCode::Char('d') if self.tab == DENEME_TAB => self.ask_deneme_delete(),
            KeyCode::Esc => self.end_session_early()?,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::Home if self.tab == 4 && self.lesson.is_none() && self.session.is_none() => {
                self.plan_today();
            }
            KeyCode::Home => self.scroll = 0,
            // `i` is not a shortcut here: it is typed like any other letter (the arm below adds it).
            KeyCode::Enter if self.tab == 0 => self.editing = Some(Input::Answer),
            // `/` her sekmeden komut girişini açar (Telegram'daki komutların TUI karşılığı).
            KeyCode::Char('/') => {
                self.editing = Some(Input::Answer);
                self.input.clear();
                self.input.push('/');
                self.status.clear();
            }
            KeyCode::Char(ch) if self.tab == 0 => {
                self.editing = Some(Input::Answer);
                self.input.push(ch);
            }
            _ => {}
        }
        Ok(())
    }
}

fn listener_running(path: &Path) -> bool {
    telegram::listener_running(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn setup() -> (tempfile::TempDir, App) {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("kaynaklar/Ders")).unwrap();
        fs::create_dir_all(root.path().join("kaynaklar/Diğer")).unwrap();
        fs::write(root.path().join("kaynaklar/Ders/ders.md"), "Kaynak bilgi.").unwrap();
        let app = App::open(root.path()).unwrap();
        (root, app)
    }

    fn write_test_course(root: &Path, subject: &str) -> Vec<crate::course::DayConfig> {
        let cfg = crate::course::CourseConfig {
            title: None,
            persona: None,
            exam: None,
            plan: vec![
                crate::course::DayConfig {
                    day: 1,
                    title: "Tanı".into(),
                    tasks: vec![
                        crate::course::TaskConfig {
                            command: "/tekrar".into(),
                            label: "Tekrar".into(),
                        },
                        crate::course::TaskConfig {
                            command: "/deneme 80".into(),
                            label: "Teşhis denemesi".into(),
                        },
                        crate::course::TaskConfig {
                            command: "".into(),
                            label: "Elle: Okuma 1".into(),
                        },
                        crate::course::TaskConfig {
                            command: "".into(),
                            label: "Elle: Okuma 2".into(),
                        },
                    ],
                },
                crate::course::DayConfig {
                    day: 2,
                    title: "Konu".into(),
                    tasks: vec![
                        crate::course::TaskConfig {
                            command: "/tekrar".into(),
                            label: "Tekrar".into(),
                        },
                        crate::course::TaskConfig {
                            command: "/anlat G00 Sentence Architecture".into(),
                            label: "Konu: Cümle yapısı (G00)".into(),
                        },
                        crate::course::TaskConfig {
                            command: "/kelime vocab 20".into(),
                            label: "Kelime kartları".into(),
                        },
                        crate::course::TaskConfig {
                            command: "".into(),
                            label: "Elle: Makale okuma".into(),
                        },
                    ],
                },
                crate::course::DayConfig {
                    day: 3,
                    title: "Pekiştirme".into(),
                    tasks: vec![
                        crate::course::TaskConfig {
                            command: "/tekrar".into(),
                            label: "Tekrar".into(),
                        },
                        crate::course::TaskConfig {
                            command: "/quiz G00 5".into(),
                            label: "Quiz".into(),
                        },
                        crate::course::TaskConfig {
                            command: "".into(),
                            label: "Elle: Tekrar".into(),
                        },
                    ],
                },
                crate::course::DayConfig {
                    day: 4,
                    title: "Sınav".into(),
                    tasks: vec![],
                },
            ],
        };
        fs::write(
            root.join("kaynaklar").join(subject).join("course.json"),
            serde_json::to_string_pretty(&cfg).unwrap(),
        )
        .unwrap();
        cfg.plan
    }

    fn key(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
    }

    /// İki tamamlanmış deneme; sıra 1 en yenisidir.
    fn two_denemes(app: &mut App) {
        let stored = app
            .store
            .add_exam_questions("Ders", "Deneme Sınavı", numbered_questions(2))
            .unwrap();
        for choice in [1, 0] {
            let mut session =
                bot::Session::start(&app.store, "Ders", "Deneme Sınavı", stored.clone()).unwrap();
            session.answer(&mut app.store, choice).unwrap();
            session.finish(&app.store, true).unwrap();
        }
    }

    fn answer_reply(text: &str) -> String {
        serde_json::json!({ "answer": text }).to_string()
    }

    #[test]
    fn denemeler_tab_lists_denemes_and_opens_coach_chat_with_breakdown() {
        let (_root, mut app) = setup();
        two_denemes(&mut app);

        key(&mut app, KeyCode::Char('6'));
        assert_eq!(app.tab, DENEME_TAB);
        assert_eq!(app.deneme_items.len(), 2);
        assert!(app.deneme_chat.is_none());

        app.open_deneme_chat(|_, _| Ok(answer_reply("Gramer zayıf.")))
            .unwrap();
        complete(&mut app);
        let chat = app.deneme_chat.as_ref().expect("sohbet açık olmalı");
        assert!(chat.breakdown.contains("Deneme 1 analizi"));
        assert_eq!(
            chat.turns[0].reply,
            CoachReply::Answer("Gramer zayıf.".into())
        );
        assert!(app.answer.is_empty());
    }

    #[test]
    fn coach_chat_arrows_scroll_the_chat_and_keep_the_selection() {
        let (_root, mut app) = setup();
        two_denemes(&mut app);
        key(&mut app, KeyCode::Char('6'));
        app.open_deneme_chat(|_, _| Ok(answer_reply("Tamam.")))
            .unwrap();
        complete(&mut app);
        // Çizimin hesaplayacağı en büyük ofseti taklit et.
        app.deneme_chat.as_ref().unwrap().max_scroll.set(20);
        assert!(app.deneme_chat.as_ref().unwrap().follow);

        key(&mut app, KeyCode::Up);
        let chat = app.deneme_chat.as_ref().unwrap();
        assert!((chat.follow, chat.scroll) == (false, 19));
        key(&mut app, KeyCode::PageUp);
        assert_eq!(app.deneme_chat.as_ref().unwrap().scroll, 9);
        key(&mut app, KeyCode::PageDown);
        key(&mut app, KeyCode::PageDown);
        let chat = app.deneme_chat.as_ref().unwrap();
        assert!(
            (chat.follow, chat.scroll) == (true, 20),
            "en alta inince takip açılır"
        );
        assert_eq!(app.deneme_selected, 0);
        assert_eq!(app.editing, Some(Input::DenemeChat));

        // Yukarıdayken yeni tur gelince takip yeniden açılır.
        key(&mut app, KeyCode::Up);
        assert!(!app.deneme_chat.as_ref().unwrap().follow);
        app.deneme_turn("Neden?".into(), None, |_, _| Ok(answer_reply("Gramer.")))
            .unwrap();
        assert!(app.deneme_chat.as_ref().unwrap().follow);
        complete(&mut app);

        // Çizilmemiş (max 0) sohbette kaydırma ofseti bozmaz.
        app.deneme_chat.as_ref().unwrap().max_scroll.set(0);
        key(&mut app, KeyCode::Up);
        let chat = app.deneme_chat.as_ref().unwrap();
        assert!((chat.follow, chat.scroll) == (true, 0));
        key(&mut app, KeyCode::Esc);
        assert!(app.deneme_chat.is_none());
        assert!(app.editing.is_none());
    }

    #[test]
    fn failed_coach_reply_is_a_chat_turn_not_an_answer() {
        let (_root, mut app) = setup();
        two_denemes(&mut app);
        key(&mut app, KeyCode::Char('6'));
        app.open_deneme_chat(|_, _| Err(invalid("AI hatası")))
            .unwrap();
        complete(&mut app);

        let chat = app.deneme_chat.as_ref().unwrap();
        assert!(matches!(chat.turns[0].reply, CoachReply::Failed(_)));
        assert!(app.answer.is_empty());
    }

    #[test]
    fn closing_the_chat_drops_a_late_coach_reply() {
        let (_root, mut app) = setup();
        two_denemes(&mut app);
        key(&mut app, KeyCode::Char('6'));
        app.open_deneme_chat(|_, _| Ok(answer_reply("Geç gelen yanıt.")))
            .unwrap();
        key(&mut app, KeyCode::Esc);

        complete(&mut app);
        assert!(app.deneme_chat.is_none());
        assert!(app.answer.is_empty());
    }

    #[test]
    fn denemeler_delete_needs_y_and_keeps_selection_in_range() {
        let (_root, mut app) = setup();
        two_denemes(&mut app);
        key(&mut app, KeyCode::Char('6'));
        key(&mut app, KeyCode::Down);
        assert_eq!(app.deneme_selected, 1);

        key(&mut app, KeyCode::Char('d'));
        key(&mut app, KeyCode::Char('n'));
        assert!(app.deneme_delete.is_none());
        assert_eq!(app.deneme_items.len(), 2);

        key(&mut app, KeyCode::Char('d'));
        key(&mut app, KeyCode::Char('y'));
        assert_eq!(app.deneme_items.len(), 1);
        assert_eq!(app.deneme_selected, 0);
        assert_eq!(app.store.deneme_sessions("Ders", 10).unwrap().len(), 1);
    }

    #[test]
    fn letter_i_typed_first_in_chat_is_kept() {
        let (_root, mut app) = setup();
        assert_eq!(app.tab, 0);
        key(&mut app, KeyCode::Char('i'));
        key(&mut app, KeyCode::Char('l'));
        key(&mut app, KeyCode::Char('k'));
        assert_eq!(app.input, "ilk");
    }

    #[test]
    fn altgr_letters_are_typed_and_plain_ctrl_still_shortcuts() {
        fn altgr(app: &mut App, ch: char) {
            app.key(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ))
            .unwrap();
        }
        let (_root, mut app) = setup();
        altgr(&mut app, 'i');
        altgr(&mut app, 't');
        assert_eq!(app.input, "it");
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(app.input.is_empty());

        key(&mut app, KeyCode::Esc);
        two_denemes(&mut app);
        key(&mut app, KeyCode::Char('6'));
        app.open_deneme_chat(|_, _| Ok(answer_reply("Tamam.")))
            .unwrap();
        complete(&mut app);
        altgr(&mut app, 'i');
        assert_eq!(app.input, "i");
    }

    #[test]
    fn ignored_ctrl_letter_is_named_in_status() {
        let (_root, mut app) = setup();
        key(&mut app, KeyCode::Enter);
        app.key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL))
            .unwrap();
        assert_eq!(app.status, "Yok sayılan tuş: Ctrl+x");
        assert!(app.input.is_empty());
    }

    /// Tamamlanmış bir deneme; her soruda verilen şık `choices` sırasıyla. Doğru şık indeks 1.
    fn deneme_with_choices(app: &mut App, choices: &[usize]) {
        let stored = app
            .store
            .add_questions("Ders", "Deneme Sınavı", numbered_questions(choices.len()))
            .unwrap();
        let mut session = bot::Session::start(&app.store, "Ders", "Deneme Sınavı", stored).unwrap();
        for &choice in choices {
            session.answer(&mut app.store, choice).unwrap();
        }
        session.finish(&app.store, true).unwrap();
    }

    /// Koçun gördüğü istemleri kaydeden sahte üretici.
    fn recording_coach(
        prompts: &Arc<Mutex<Vec<String>>>,
    ) -> impl FnOnce(String, String) -> Result<String> + Send + 'static {
        let prompts = Arc::clone(prompts);
        move |prompt, _schema| {
            prompts.lock().unwrap().push(prompt);
            Ok(answer_reply("Tamam."))
        }
    }

    fn last_prompt(prompts: &Arc<Mutex<Vec<String>>>) -> String {
        prompts.lock().unwrap().last().cloned().unwrap_or_default()
    }

    /// Üç yanlışlı deneme, koç sohbeti açık ve ilk değerlendirmesi bitmiş.
    fn open_chat_with_three_wrongs(app: &mut App, prompts: &Arc<Mutex<Vec<String>>>) {
        deneme_with_choices(app, &[0, 0, 0]);
        key(app, KeyCode::Char('6'));
        app.open_deneme_chat(recording_coach(prompts)).unwrap();
        complete(app);
    }

    #[test]
    fn tab_walks_the_wrong_answers_and_the_coach_sees_each_question() {
        let (_root, mut app) = setup();
        let prompts = Arc::new(Mutex::new(Vec::new()));
        open_chat_with_three_wrongs(&mut app, &prompts);
        let focus = |app: &App| app.deneme_chat.as_ref().unwrap().focus;

        for (expected, text) in [(0, "Soru 0?"), (1, "Soru 1?"), (2, "Soru 2?")] {
            app.step_wrong(true, recording_coach(&prompts)).unwrap();
            complete(&mut app);
            assert_eq!(focus(&app), Some(expected));
            let prompt = last_prompt(&prompts);
            assert!(prompt.contains("ŞU ANKİ SORU"), "{prompt}");
            assert!(prompt.contains(&format!("Soru metni: {text}")), "{prompt}");
        }

        let turns = app.deneme_chat.as_ref().unwrap().turns.len();
        app.step_wrong(true, recording_coach(&prompts)).unwrap();
        assert_eq!(app.status, "Son yanlış soru.");
        assert_eq!(focus(&app), Some(2));
        assert_eq!(app.deneme_chat.as_ref().unwrap().turns.len(), turns);

        for expected in [1, 0] {
            app.step_wrong(false, recording_coach(&prompts)).unwrap();
            complete(&mut app);
            assert_eq!(focus(&app), Some(expected));
        }
        app.step_wrong(false, recording_coach(&prompts)).unwrap();
        assert_eq!(app.status, "İlk yanlış soru.");
        assert_eq!(focus(&app), Some(0));
    }

    #[test]
    fn typed_wrong_number_jumps_and_a_free_question_keeps_the_focus() {
        let (_root, mut app) = setup();
        let prompts = Arc::new(Mutex::new(Vec::new()));
        open_chat_with_three_wrongs(&mut app, &prompts);
        let focus = |app: &App| app.deneme_chat.as_ref().unwrap().focus;

        app.input = "3".into();
        app.send_deneme_chat(recording_coach(&prompts)).unwrap();
        complete(&mut app);
        assert_eq!(focus(&app), Some(2));
        assert!(app.input.is_empty());
        assert!(last_prompt(&prompts).contains("Soru metni: Soru 2?"));

        app.input = "9".into();
        app.send_deneme_chat(recording_coach(&prompts)).unwrap();
        complete(&mut app);
        assert_eq!(
            focus(&app),
            Some(2),
            "9 yanlış numarası değil; serbest soru"
        );

        app.input = "Neden yanlış yaptım?".into();
        app.send_deneme_chat(recording_coach(&prompts)).unwrap();
        complete(&mut app);
        let prompt = last_prompt(&prompts);
        assert_eq!(focus(&app), Some(2));
        assert!(prompt.contains("Soru metni: Soru 2?"), "{prompt}");
        assert!(prompt.contains("Neden yanlış yaptım?"), "{prompt}");
    }

    #[test]
    fn tab_keeps_the_draft_and_does_nothing_while_a_reply_is_pending() {
        let (_root, mut app) = setup();
        let prompts = Arc::new(Mutex::new(Vec::new()));
        open_chat_with_three_wrongs(&mut app, &prompts);
        app.input = "yarım kalan soru".into();

        app.step_wrong(true, recording_coach(&prompts)).unwrap();
        assert_eq!(app.input, "yarım kalan soru");
        assert_eq!(app.deneme_chat.as_ref().unwrap().focus, Some(0));
        let turns = app.deneme_chat.as_ref().unwrap().turns.len();

        app.step_wrong(true, recording_coach(&prompts)).unwrap();
        assert_eq!(app.status, "Önceki istek sürüyor…");
        assert_eq!(app.deneme_chat.as_ref().unwrap().focus, Some(0));
        assert_eq!(app.deneme_chat.as_ref().unwrap().turns.len(), turns);
        complete(&mut app);
    }

    #[test]
    fn tab_without_wrong_answers_says_so_and_starts_nothing() {
        let (_root, mut app) = setup();
        deneme_with_choices(&mut app, &[1, 1]);
        key(&mut app, KeyCode::Char('6'));
        let prompts = Arc::new(Mutex::new(Vec::new()));
        app.open_deneme_chat(recording_coach(&prompts)).unwrap();
        complete(&mut app);

        app.step_wrong(true, recording_coach(&prompts)).unwrap();
        assert_eq!(app.status, "Bu denemede yanlış yok.");
        let chat = app.deneme_chat.as_ref().unwrap();
        assert_eq!(chat.turns.len(), 1);
        assert_eq!(chat.focus, None);
    }

    fn question() -> ai::Question {
        ai::Question {
            question: "Soru?".into(),
            choices: ["a", "b", "c", "d"].map(String::from).into(),
            answer_index: 1,
            explanation: "Doğru cevap: 'b'. Açıklama".into(),
            source: "ders.md".into(),
            topic: None,
        }
    }

    /// `count` questions with distinct text (duplicates are rejected by validation).
    fn numbered_questions(count: usize) -> Vec<ai::Question> {
        (0..count)
            .map(|i| ai::Question {
                question: format!("Soru {i}?"),
                ..question()
            })
            .collect()
    }

    fn complete(app: &mut App) {
        let started = Instant::now();
        while app.pending.is_some() {
            app.poll().unwrap();
            assert!(started.elapsed() < Duration::from_secs(5));
            thread::yield_now();
        }
    }

    #[test]
    fn plan_task_is_marked_only_after_its_ai_content_is_saved() {
        let (_root, mut app) = setup();
        start_plan_today(&mut app);
        let today = Local::now().date_naive();
        let done_at =
            |app: &App| match plan::status(&app.store, "Ders", &app.course.plan, today).unwrap() {
                plan::Status::Active { done, .. } => done[1],
                _ => panic!("plan aktif olmalı"),
            };
        // Başarısız AI işi: görev işaretlenmez.
        app.plan_task = Some((today, 1, 1));
        app.begin_ai_with(
            JobKind::Lesson {
                topic: "G00 Sentence Architecture".into(),
            },
            |_, _| Err(invalid("AI hatası")),
        )
        .unwrap();
        let started = Instant::now();
        while app.pending.is_some() {
            let _ = app.poll();
            assert!(started.elapsed() < Duration::from_secs(5));
            thread::yield_now();
        }
        assert!(!done_at(&app));
        // Başarılı AI işi: içerik kaydedilince görev işaretlenir.
        app.plan_task = Some((today, 1, 1));
        app.begin_ai_with(
            JobKind::Lesson {
                topic: "G00 Sentence Architecture".into(),
            },
            |_, _| {
                Ok(serde_json::json!({"lesson":"Anlatım. Kaynak: ders.md", "questions":numbered_questions(5)}).to_string())
            },
        )
        .unwrap();
        complete(&mut app);
        assert!(done_at(&app));
    }

    #[test]
    fn slash_commands_without_ai_switch_tabs_and_show_plan_subjects_and_history() {
        let (_root, mut app) = setup();
        start_plan_today(&mut app);
        app.route("/bugun").unwrap();
        assert_eq!(app.tab, 4);
        app.route("/plan gun 2").unwrap();
        assert_eq!(app.tab, 0);
        assert!(app.answer.contains("Gün 2/4"));
        assert!(app.answer.contains("Önizleme"));
        assert!(!app.answer.contains("**"));
        app.route("/dersler").unwrap();
        assert!(app.answer.contains("Ders:"));
        app.route("/denemeler").unwrap();
        assert!(app.answer.contains("Henüz tamamlanmış"));
        app.route("/ders Yok").unwrap();
        assert!(app.status.contains("Ders bulunamadı"));
        app.route("/bilinmeyen").unwrap();
        assert!(app.status.contains("Bilinmeyen komut"));
        assert!(app.pending.is_none());
    }

    #[test]
    fn vocab_job_saves_an_anki_file_and_shows_the_cards() {
        let (_root, mut app) = setup();
        app.begin_ai_with(
            JobKind::Vocab {
                topic: "Çevre".into(),
                count: 2,
            },
            |_, _| {
                Ok(serde_json::json!({"cards":[
                    {"word":"pollution","meaning":"kirlilik","synonyms":"contamination","example":"Pollution harms rivers.","example_tr":"Kirlilik nehirlere zarar verir."},
                    {"word":"habitat","meaning":"yaşam alanı","synonyms":"environment","example":"Forests are a habitat.","example_tr":"Ormanlar bir yaşam alanıdır."}
                ]})
                .to_string())
            },
        )
        .unwrap();
        complete(&mut app);
        assert!(app.answer.contains("2 kavram kartı hazır"));
        assert!(app.answer.contains("• pollution — kirlilik"));
        assert!(app.answer.contains("Anki:"));
        assert!(app.status.contains("Kartlar hazır"));
    }

    #[test]
    fn escape_after_last_answer_cancels_the_quiz() {
        let (_root, mut app) = setup();
        app.store
            .add_questions("Ders", "Konu", vec![question()])
            .unwrap();
        key(&mut app, KeyCode::Char('2'));
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('b'));
        key(&mut app, KeyCode::Esc);
        assert!(app.session.is_none());
        assert!(app.summary.contains("iptal"));
    }

    #[test]
    fn tabs_and_subject_selection_persist_and_follow_the_listener() {
        let (_root, mut app) = setup();
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.tab, 1);
        key(&mut app, KeyCode::Char('4'));
        assert_eq!(app.tab, 3);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.tab, 4);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.tab, DENEME_TAB);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.tab, 0);
        key(&mut app, KeyCode::BackTab);
        assert_eq!(app.tab, DENEME_TAB);
        key(&mut app, KeyCode::BackTab);
        assert_eq!(app.tab, 4);
        key(&mut app, KeyCode::BackTab);
        assert_eq!(app.tab, 3);
        key(&mut app, KeyCode::Down);
        assert_eq!(
            app.store.state("active_subject").unwrap().as_deref(),
            Some("Diğer")
        );
        app.store.set_state("active_subject", "Ders").unwrap();
        app.refresh().unwrap();
        assert_eq!(app.selected, 0);
        key(&mut app, KeyCode::Up);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn typing_protects_q_control_c_and_numbers_until_escape() {
        let (_root, mut app) = setup();
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('q'));
        key(&mut app, KeyCode::Char('4'));
        app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(!app.quit);
        assert_eq!(app.input, "q4");
        assert_eq!(app.tab, 0);
        key(&mut app, KeyCode::Esc);
        assert!(app.editing.is_none());
        assert!(app.input.is_empty());
        key(&mut app, KeyCode::Char('q'));
        assert!(app.quit);

        let (_root, mut app) = setup();
        key(&mut app, KeyCode::Char('2'));
        key(&mut app, KeyCode::Char('n'));
        key(&mut app, KeyCode::Char('q'));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.editing, Some(Input::Count));
        app.input = "31".into();
        key(&mut app, KeyCode::Enter);
        assert!(app.pending.is_none());
        assert!(app.status.contains("1–30"));
        key(&mut app, KeyCode::Esc);
        assert!(app.editing.is_none());
        app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(app.quit);
    }

    #[test]
    fn quiz_answers_advance_once_record_reviews_and_finish_with_statistics() {
        let (_root, mut app) = setup();
        app.store
            .add_questions("Ders", "Konu", vec![question(); 3])
            .unwrap();
        key(&mut app, KeyCode::Char('2'));
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('2'));
        assert_eq!(app.session.as_ref().unwrap().position, 1);
        assert_eq!(app.tab, 1);
        assert!(app.feedback.as_ref().unwrap().contains("✅ Doğru"));
        key(&mut app, KeyCode::Char('b'));
        assert_eq!(app.session.as_ref().unwrap().position, 1);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('a'));
        assert!(app.feedback.as_ref().unwrap().contains("❌ Yanlış"));
        assert_eq!(app.store.review_counts("Ders", Utc::now()).unwrap(), (0, 1));
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('b'));
        key(&mut app, KeyCode::Enter);
        assert!(app.session.is_none());
        assert!(app.summary.contains("2/3"));
        assert!(app.summary.contains("dakika"));
        assert!(app.store.unanswered("Ders").unwrap().is_empty());
        assert_eq!((app.stats[0].correct, app.stats[0].total), (2, 3));
        assert_eq!(app.accuracy, vec![("Ders".to_string(), 2, 3)]);
        assert_eq!(app.weakest, vec![("Kayıtlı sorular".to_string(), 2, 3)]);
        assert_eq!(app.daily.len(), 1);
        assert_eq!((app.daily[0].1, app.daily[0].2), (2, 3));
        assert_eq!(app.wrong_count(), 1);
        // "w" replays only the wrong answer; answering it finishes the one-question retry.
        key(&mut app, KeyCode::Char('2'));
        key(&mut app, KeyCode::Char('w'));
        assert_eq!(app.session.as_ref().unwrap().questions.len(), 1);
        assert_eq!(app.session.as_ref().unwrap().topic, "Yanlışlar");
        key(&mut app, KeyCode::Char('b'));
        assert_eq!(
            app.feedback.as_deref().map(|f| f.contains("✅ Doğru")),
            Some(true)
        );
        key(&mut app, KeyCode::Enter);
        assert!(app.session.is_none());
        key(&mut app, KeyCode::Char('4'));
        assert_eq!(app.tab, 3);
    }

    #[test]
    fn pending_fake_ai_spins_keeps_navigation_live_and_rejects_second_job() {
        let (_root, mut app) = setup();
        let (release, wait) = mpsc::channel();
        app.begin_ai_with(
            JobKind::Answer {
                question: "Soru?".into(),
            },
            move |prompt, schema| {
                assert!(prompt.contains("### ders.md"));
                assert_eq!(schema, ai::ANSWER_SCHEMA);
                wait.recv().unwrap();
                Ok(r#"{"answer":"Kaynak: ders.md"}"#.into())
            },
        )
        .unwrap();
        assert!(app.spinner().unwrap().contains("Yanıt hazırlanıyor"));
        app.poll().unwrap();
        assert!(app.pending.is_some());
        app.begin_ai_with(
            JobKind::Answer {
                question: "İkinci?".into(),
            },
            |_, _| panic!("second AI job must not start"),
        )
        .unwrap();
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Down);
        assert_eq!(app.tab, 1);
        assert_eq!(app.selected, 1);
        release.send(()).unwrap();
        complete(&mut app);
        assert!(app.spinner().is_none());
        assert!(app.answer.contains("Kaynak: ders.md"));
        assert_eq!(app.answer_question.as_deref(), Some("Soru?"));
        assert!(app.status.contains("Sor sekmesinde"));
        assert_eq!(app.store.history("Ders").unwrap().len(), 2);
        assert!(app.store.history("Diğer").unwrap().is_empty());
    }

    #[test]
    fn generated_quiz_and_due_reviews_use_the_same_answer_flow() {
        let (_root, mut app) = setup();
        app.begin_ai_with(
            JobKind::Quiz {
                topic: "Hücre".into(),
                count: 3,
            },
            |prompt, schema| {
                assert!(prompt.contains("'Hücre' konusu için tam 3"));
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&schema).unwrap()["properties"]
                        ["questions"]["minItems"],
                    3
                );
                Ok(serde_json::to_string(&numbered_questions(3)).unwrap())
            },
        )
        .unwrap();
        complete(&mut app);
        assert_eq!(app.tab, 1);
        assert_eq!(app.session.as_ref().unwrap().questions.len(), 3);
        key(&mut app, KeyCode::Esc);
        assert!(app.session.is_none());
        let questions = app.store.unanswered("Ders").unwrap();
        let old = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let id = app
            .store
            .start_session("Ders", "Eski", &old.to_rfc3339())
            .unwrap();
        app.store
            .record_answer_at(id, questions[0].id, 0, old)
            .unwrap();
        app.store.end_session(id).unwrap();
        key(&mut app, KeyCode::Char('3'));
        key(&mut app, KeyCode::Enter);
        assert_eq!(
            app.session.as_ref().unwrap().questions[0].id,
            questions[0].id
        );
        key(&mut app, KeyCode::Char('b'));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.store.review_counts("Ders", Utc::now()).unwrap(), (0, 1));
        key(&mut app, KeyCode::Enter);
        assert!(app.status.contains("Sonraki tekrar"));
    }

    #[test]
    fn exact_review_skips_finished_deneme_questions() {
        let (_root, mut app) = setup();
        let yesterday = Utc::now() - chrono::Duration::days(1);
        let deneme = app
            .store
            .add_exam_questions("Ders", "Academic Mock 1 Deneme", numbered_questions(1))
            .unwrap()[0]
            .id;
        let normal = app
            .store
            .add_questions("Ders", "Konu", vec![question()])
            .unwrap()[0]
            .id;
        for (topic, kind, id) in [
            ("Academic Mock 1 Deneme", "exam", deneme),
            ("Konu", "quiz", normal),
        ] {
            let session = app
                .store
                .start_session_with_kind("Ders", topic, kind, &yesterday.to_rfc3339())
                .unwrap();
            app.store
                .record_answer_at(session, id, 0, yesterday)
                .unwrap();
            app.store.end_session(session).unwrap();
        }
        app.review(10, true).unwrap();
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.topic, "tekrar (orijinal)");
        assert_eq!(
            session.questions.iter().map(|q| q.id).collect::<Vec<_>>(),
            vec![normal]
        );
    }

    #[test]
    fn plan_lesson_shows_scrollable_lesson_before_its_quiz_on_the_plan_tab() {
        let (_root, mut app) = setup();
        key(&mut app, KeyCode::Char('5'));
        assert_eq!(app.tab, 4);
        app.begin_ai_with(JobKind::Lesson { topic: "G00 Sentence Architecture".into() }, |prompt, schema| {
            assert!(prompt.contains("G00 Sentence Architecture"));
            assert_eq!(schema, ai::lesson_schema());
            Ok(serde_json::json!({"lesson":"Anlatım. Kaynak: ders.md", "questions":numbered_questions(5)}).to_string())
        }).unwrap();
        complete(&mut app);
        assert_eq!(app.session_tab, 4);
        assert!(app.lesson.as_ref().unwrap().contains("Anlatım"));
        key(&mut app, KeyCode::PageDown);
        assert_eq!(app.scroll, 10);
        key(&mut app, KeyCode::Char('b'));
        assert_eq!(app.session.as_ref().unwrap().position, 0);
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Enter);
        assert!(app.lesson.is_none());
        assert_eq!(app.scroll, 0);
        let id = app.session.as_ref().unwrap().id;
        for _ in 0..5 {
            key(&mut app, KeyCode::Char('b'));
            key(&mut app, KeyCode::Enter);
        }
        assert!(app.session.is_none());
        assert!(app.summary.contains("5/5"));
        assert_eq!(app.summary_session, Some(id));
        assert_eq!(app.store.question_count("Ders").unwrap(), 5);
    }

    #[test]
    fn plan_tab_runs_review_and_launches_vocabulary_job() {
        let (_root, mut app) = setup();
        let days = write_test_course(&app.root, "Ders");
        plan::set_start(
            &app.store,
            "Ders",
            Local::now().date_naive() - chrono::Days::new(1),
        )
        .unwrap();
        app.refresh().unwrap();
        key(&mut app, KeyCode::Char('5'));
        // Day 2 has four tasks: review, G00 lesson, vocabulary, reading.
        assert_eq!(app.plan_task_count(), 4);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert!(app.pending.is_some());
        let _ = app.pending.take();
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.tab, 2);
        match plan::status(&app.store, "Ders", &days, Local::now().date_naive()).unwrap() {
            plan::Status::Active { done, .. } => assert_eq!(done, vec![true, false, false, false]),
            _ => panic!("gün 2 aktif olmalı"),
        }
    }

    #[test]
    fn escape_discards_pending_lesson_without_creating_quiz() {
        let (_root, mut app) = setup();
        let (release, wait) = mpsc::channel();
        app.begin_ai_with(
            JobKind::Lesson {
                topic: "Konu".into(),
            },
            move |_, _| {
                wait.recv().unwrap();
                Ok(
                    serde_json::json!({"lesson":"Ders", "questions":numbered_questions(5)})
                        .to_string(),
                )
            },
        )
        .unwrap();
        key(&mut app, KeyCode::Esc);
        assert!(app.pending.is_none());
        assert!(app.status.contains("İptal"));
        release.send(()).unwrap();
        complete(&mut app);
        assert!(app.session.is_none());
        assert!(app.lesson.is_none());
        assert_eq!(app.store.question_count("Ders").unwrap(), 0);
    }

    #[test]
    fn ai_failure_clears_pending_without_exposing_details() {
        let (_root, mut app) = setup();
        app.begin_ai_with(
            JobKind::Answer {
                question: "Soru?".into(),
            },
            |_, _| Err(invalid("PRIVATE_TOKEN_SENTINEL")),
        )
        .unwrap();
        let started = Instant::now();
        while app.pending.is_some() {
            if app.poll().is_err() {
                app.status = "İşlem tamamlanamadı.".into();
            }
            assert!(started.elapsed() < Duration::from_secs(5));
            thread::yield_now();
        }
        assert!(!app.status.contains("PRIVATE_TOKEN_SENTINEL"));
        assert!(app.store.history("Ders").unwrap().is_empty());
    }

    /// Plan bugün başlamış olsun: gün 1 (tekrar, deneme, iki elle görev).
    fn start_plan_today(app: &mut App) {
        write_test_course(&app.root, "Ders");
        plan::set_start(&app.store, "Ders", Local::now().date_naive()).unwrap();
        app.refresh().unwrap();
    }

    #[test]
    fn coach_decision_picks_a_pending_task_of_today_from_the_ai_answer() {
        let (_root, mut app) = setup();
        start_plan_today(&mut app);
        let reply = serde_json::json!({"index": 1, "reason": "Deneme sıradaki görev."}).to_string();
        app.begin_coach_with(move |prompt, schema| {
            assert!(prompt.contains("Teşhis denemesi"));
            assert!(prompt.contains("[1] bekliyor"));
            assert_eq!(schema, coach::SCHEMA);
            Ok(reply)
        })
        .unwrap();
        complete(&mut app);
        assert!(app.pending.is_none());
        let step = app.coach_step.clone().unwrap();
        assert_eq!((step.day, step.task), (1, 1));
        assert_eq!(app.coach_ready, Some(step));
    }

    #[test]
    fn unusable_coach_answer_falls_back_to_the_first_task_of_the_plan_day() {
        let (_root, mut app) = setup();
        start_plan_today(&mut app);
        app.begin_coach_with(|_, _| Ok("bozuk".into())).unwrap();
        complete(&mut app);
        let step = app.coach_step.clone().unwrap();
        assert_eq!(step.task, 0);
        assert!(step.reason.contains("yerel kurallarla"));
    }

    #[test]
    fn coach_review_step_runs_due_reviews_and_marks_the_plan_task_done() {
        let (_root, mut app) = setup();
        start_plan_today(&mut app);
        app.store
            .add_questions("Ders", "Konu", vec![question()])
            .unwrap();
        let questions = app.store.unanswered("Ders").unwrap();
        let old = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let id = app
            .store
            .start_session("Ders", "Eski", &old.to_rfc3339())
            .unwrap();
        app.store
            .record_answer_at(id, questions[0].id, 0, old)
            .unwrap();
        app.store.end_session(id).unwrap();
        app.refresh().unwrap();
        app.coach_ready = Some(coach::Step {
            day: 1,
            task: 0,
            command: "/tekrar".into(),
            label: "Tekrar".into(),
            reason: "Tekrar zamanı.".into(),
        });
        app.coach_tick().unwrap();
        assert!(app.pending.is_none());
        assert_eq!(app.session_tab, 2);
        assert_eq!(app.session.as_ref().unwrap().questions.len(), 1);
        match plan::status(
            &app.store,
            "Ders",
            &app.course.plan,
            Local::now().date_naive(),
        )
        .unwrap()
        {
            plan::Status::Active { done, .. } => assert!(done[0]),
            _ => panic!("gün 1 aktif olmalı"),
        }
    }

    #[test]
    fn cutting_a_session_pauses_the_coach_and_f3_resumes_it() {
        let (_root, mut app) = setup();
        app.store
            .add_questions("Ders", "Konu", vec![question(); 2])
            .unwrap();
        key(&mut app, KeyCode::Char('2'));
        key(&mut app, KeyCode::Enter);
        assert!(app.session.is_some());
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.coach_mode, CoachMode::Paused);
        key(&mut app, KeyCode::F(3));
        assert_eq!(app.coach_mode, CoachMode::On);
        assert!(app.coach_due);
        assert_eq!(
            app.store.state("coach_mode").unwrap().as_deref(),
            Some("on")
        );
    }

    #[test]
    fn finishing_a_quiz_queues_the_next_coach_decision() {
        let (_root, mut app) = setup();
        app.store
            .add_questions("Ders", "Konu", vec![question()])
            .unwrap();
        app.coach_due = false;
        key(&mut app, KeyCode::Char('2'));
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('b'));
        key(&mut app, KeyCode::Enter);
        assert!(app.session.is_none());
        assert!(app.coach_due);
        assert!(app.coach_note.contains("1/1"));
    }

    #[test]
    fn escape_on_a_coach_decision_pauses_the_coach() {
        let (_root, mut app) = setup();
        start_plan_today(&mut app);
        let (release, wait) = mpsc::channel();
        app.begin_coach_with(move |_, _| {
            wait.recv().unwrap();
            Ok(String::new())
        })
        .unwrap();
        key(&mut app, KeyCode::Esc);
        assert!(app.pending.is_none());
        assert_eq!(app.coach_mode, CoachMode::Paused);
        release.send(()).unwrap();
    }

    #[test]
    fn plan_deneme_uses_the_next_past_exam_without_calling_ai() {
        let (_root, mut app) = setup();
        start_plan_today(&mut app);
        let bank = exam::Bank {
            title: "Mock Exam 2023".into(),
            questions: numbered_questions(3),
        };
        let second = exam::Bank {
            title: "Mock Exam 2024".into(),
            questions: numbered_questions(3),
        };
        exam::save(&app.root, "Ders", "bank_2023", &bank).unwrap();
        exam::save(&app.root, "Ders", "bank_2024", &second).unwrap();
        app.refresh().unwrap();
        // Gün 1, görev 1: "/deneme". Bekleyen AI işi olmamalı.
        key(&mut app, KeyCode::Char('5'));
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert!(app.pending.is_none());
        let session = app
            .session
            .as_ref()
            .expect("sınav bankası oturumu açılmalı");
        assert_eq!(session.questions.len(), 3);
        assert!(session.is_exam);
        assert_eq!(app.tab, 4);
        assert_eq!(
            app.next_bank("Ders").unwrap().map(|(meta, _)| meta.slug),
            Some("bank_2024".into())
        );
    }

    #[test]
    fn exam_bank_is_discovered_from_kaynaklar_and_marked_used_on_start() {
        let (_root, mut app) = setup();
        start_plan_today(&mut app);
        let bank_dir = app.root.join("kaynaklar/Ders/exams");
        fs::create_dir_all(&bank_dir).unwrap();
        let raw_items = exam::fake_items(&(1..=3));
        let questions = exam::import_with("Midterm Exam", "text", &[0, 1, 2], 5, |range, _, _| {
            assert_eq!(range, 1..=3);
            Ok(raw_items.clone())
        })
        .unwrap();
        let bank = exam::Bank {
            title: "Midterm Exam".into(),
            questions,
        };
        fs::write(
            bank_dir.join("midterm.json"),
            serde_json::to_string_pretty(&bank).unwrap(),
        )
        .unwrap();
        app.deneme(app.course.exam_question_count()).unwrap();
        assert!(app.pending.is_none());
        let session = app.session.as_ref().expect("sınav oturumu açılmalı");
        assert_eq!(session.questions.len(), 3);
        assert!(session.is_exam);
        assert!(exam::load(&app.root, "Ders", "midterm").unwrap().is_some());
        assert!(
            app.store
                .state(&exam::used_key("Ders", "midterm"))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn help_overlay_swallows_keys_until_closed() {
        let (_root, mut app) = setup();
        key(&mut app, KeyCode::F(1));
        assert!(app.help);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.tab, 0);
        key(&mut app, KeyCode::Esc);
        assert!(!app.help);
    }

    #[test]
    fn ctrl_q_needs_an_answer_and_never_quits() {
        let (_root, mut app) = setup();
        app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(!app.quit);
        assert!(app.status.contains("Önce Sor sekmesinde"));
        assert!(app.pending.is_none());
    }

    #[test]
    fn long_waits_warn_and_ai_jobs_cancel_at_once() {
        let (_root, mut app) = setup();
        let (release, wait) = mpsc::channel();
        app.begin_ai_with(
            JobKind::Quiz {
                topic: "Hücre".into(),
                count: 3,
            },
            move |_, _| {
                wait.recv().unwrap();
                Ok(String::new())
            },
        )
        .unwrap();
        app.pending.as_mut().unwrap().started = Instant::now() - Duration::from_secs(400);
        let spinner = app.spinner().unwrap();
        assert!(spinner.contains("Quiz hazırlanıyor"));
        assert!(spinner.contains("Uzun sürüyor"));
        assert!(spinner.contains("Esc iptal"));
        key(&mut app, KeyCode::Esc);
        assert!(app.spinner().is_none());
        assert!(app.status.contains("İptal"));
        release.send(()).unwrap();
    }

    #[test]
    fn success_notices_expire_while_errors_wait_for_the_next_key() {
        let (_root, mut app) = setup();
        app.status = "✓ Tamam.".into();
        app.tick();
        app.status_since = Instant::now() - NOTICE_TTL - Duration::from_secs(1);
        app.tick();
        assert!(app.status.is_empty());
        app.status = "Hata.".into();
        app.tick();
        app.status_since = Instant::now() - NOTICE_TTL - Duration::from_secs(1);
        app.tick();
        assert_eq!(app.status, "Hata.");
        key(&mut app, KeyCode::Tab);
        assert!(app.status.is_empty());
    }

    #[test]
    fn b_key_starts_an_unstarted_plan_from_today() {
        let (root, mut app) = setup();
        write_test_course(root.path(), "Ders");
        app.refresh().unwrap();
        key(&mut app, KeyCode::Char('5'));
        assert!(matches!(app.plan, plan::Status::NotStarted));
        key(&mut app, KeyCode::Char('b'));
        assert!(matches!(app.plan, plan::Status::Active { day: 1, .. }));
        assert!(app.status.contains("bugünden"));
    }

    #[cfg(windows)]
    #[test]
    fn listener_status_checks_held_lock_rather_than_file_existence() {
        let (root, mut app) = setup();
        let path = root.path().join("data/listen.lock");
        let held = telegram::open_listener_lock(&path).unwrap();
        app.refresh().unwrap();
        assert!(app.listener);
        drop(held);
        app.refresh().unwrap();
        assert!(!app.listener);
    }

    #[test]
    fn lesson_is_cached_across_restarts_scrolls_with_arrows_and_persists_chat_turns() {
        let (root, mut app) = setup();
        write_test_course(root.path(), "Ders");
        // Gün 2'de görev 1: "/anlat G00 Sentence Architecture"
        plan::set_start(
            &app.store,
            "Ders",
            Local::now().date_naive() - chrono::Days::new(1),
        )
        .unwrap();
        app.refresh().unwrap();
        key(&mut app, KeyCode::Char('5'));
        key(&mut app, KeyCode::Down);
        assert_eq!(app.task_selected, 1);

        // İlk kez ders anlatımı üretilir ve kaydedilir.
        app.begin_ai_with(
            JobKind::Lesson {
                topic: "G00 Sentence Architecture".into(),
            },
            |_, _| {
                Ok(serde_json::json!({
                    "lesson": "### **Konu Özeti**\nSatır 1\nSatır 2\nKaynak: ders.md",
                    "questions": numbered_questions(5)
                })
                .to_string())
            },
        )
        .unwrap();
        complete(&mut app);
        assert_eq!(app.editing, Some(Input::LessonChat));
        assert_eq!(app.lesson_topic, "G00 Sentence Architecture");

        // ↑ / ↓ tuşları arkadaki görev seçimini değiştirmeden ders metnini kaydırır.
        app.lesson_max_scroll.set(25);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        assert_eq!(app.scroll, 2);
        assert_eq!(app.task_selected, 1);
        key(&mut app, KeyCode::Up);
        assert_eq!(app.scroll, 1);

        // Öğrenci ders ekranında soru sorar; yanıt hem ekrana hem veritabanına kaydedilir.
        for ch in "Özne nasıl bulunur?".chars() {
            key(&mut app, KeyCode::Char(ch));
        }
        assert_eq!(app.input, "Özne nasıl bulunur?");
        app.send_lesson_chat(|prompt, _| {
            assert!(prompt.contains("G00 Sentence Architecture"));
            assert!(prompt.contains("Özne nasıl bulunur?"));
            Ok(answer_reply("Fiilden önceki ana isim öbeğine bakın."))
        })
        .unwrap();
        assert!(app.input.is_empty());
        assert!(app.lesson_follow);
        complete(&mut app);
        assert_eq!(app.lesson_turns.len(), 1);
        assert_eq!(
            app.lesson_turns[0].reply,
            CoachReply::Answer("Fiilden önceki ana isim öbeğine bakın.".into())
        );

        // Uygulama kapatılıp yeniden açıldığında aynı göreve tıklanınca AI beklenmeden anında açılır.
        drop(app);
        let mut reopened = App::open(root.path()).unwrap();
        key(&mut reopened, KeyCode::Char('5'));
        key(&mut reopened, KeyCode::Down);
        assert_eq!(reopened.task_selected, 1);
        key(&mut reopened, KeyCode::Enter);
        assert!(reopened.pending.is_none());
        assert!(reopened.lesson.as_deref().unwrap().contains("Konu Özeti"));
        assert_eq!(reopened.lesson_turns.len(), 1);
        assert_eq!(reopened.lesson_turns[0].question, "Özne nasıl bulunur?");
        assert_eq!(
            reopened.lesson_turns[0].reply,
            CoachReply::Answer("Fiilden önceki ana isim öbeğine bakın.".into())
        );
        assert_eq!(reopened.session.as_ref().unwrap().questions.len(), 5);
        assert!(reopened.status.contains("Kayıtlı ders anlatımı açıldı"));

        // Boş Enter ile 5 soruluk Kazanım Testine geçilir.
        key(&mut reopened, KeyCode::Enter);
        assert!(reopened.lesson.is_none());
        assert!(reopened.editing.is_none());
        assert!(reopened.session.is_some());
    }

    #[test]
    fn manual_plan_task_caches_its_explanation_and_reopens_without_waiting_for_ai() {
        let (root, mut app) = setup();
        start_plan_today(&mut app);
        let label = plan::task(&app.course.plan, 1, 2).unwrap().label.clone();
        let question = bot::manual_task_question(&label);
        app.begin_ai_with(
            JobKind::Answer {
                question: question.clone(),
            },
            |_, _| Ok(answer_reply("Bu görevi şöyle uygulayın. Kaynak: ders.md")),
        )
        .unwrap();
        complete(&mut app);

        drop(app);
        let mut reopened = App::open(root.path()).unwrap();
        key(&mut reopened, KeyCode::Char('5'));
        key(&mut reopened, KeyCode::Down);
        key(&mut reopened, KeyCode::Down);
        assert_eq!(reopened.task_selected, 2);
        key(&mut reopened, KeyCode::Enter);
        assert!(reopened.pending.is_none());
        assert_eq!(reopened.tab, 0);
        assert!(reopened.answer.contains("Bu görevi şöyle uygulayın."));
    }

    #[test]
    fn plan_navigation_with_arrow_keys_and_shortcuts_walks_days_and_returns_to_today() {
        let (root, mut app) = setup();
        let days = write_test_course(root.path(), "Ders");
        // Plan starts 1 day ago -> active day is Day 2.
        plan::set_start(
            &app.store,
            "Ders",
            Local::now().date_naive() - chrono::Days::new(1),
        )
        .unwrap();
        app.refresh().unwrap();
        key(&mut app, KeyCode::Char('5'));
        assert_eq!(app.tab, 4);
        assert_eq!(app.plan_day(), 2);

        // Sol ok ile önceki güne (Gün 1) gitme.
        key(&mut app, KeyCode::Left);
        assert_eq!(app.plan_day(), 1);
        assert!(app.status.contains("Gün 1/4"));

        // Gün 1'deyken sola basınca 1'in altına düşmez.
        key(&mut app, KeyCode::Left);
        assert_eq!(app.plan_day(), 1);

        // 'l' veya ']' ile ileri gitme.
        key(&mut app, KeyCode::Char('l'));
        assert_eq!(app.plan_day(), 2);
        key(&mut app, KeyCode::Char(']'));
        assert_eq!(app.plan_day(), 3);

        // 'h' veya '[' ile geri gitme.
        key(&mut app, KeyCode::Char('h'));
        assert_eq!(app.plan_day(), 2);
        key(&mut app, KeyCode::Char('['));
        assert_eq!(app.plan_day(), 1);

        // Home veya 'b' tuşuyla bugüne (Gün 2) anında dönme.
        key(&mut app, KeyCode::Home);
        assert_eq!(app.plan_day(), 2);
        assert!(app.status.contains("Bugüne dönüldü"));

        // Gün 1'e gidip oradaki görevi çalıştırma (geçmiş gün görevi tamamlama).
        key(&mut app, KeyCode::Left);
        assert_eq!(app.plan_day(), 1);
        // Gün 1 görev 0: /tekrar
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.tab, 2);
        // Gün 1'in 0. görevi tamamlandı olarak işaretlenmeli.
        let first = Local::now().date_naive() - chrono::Days::new(1);
        assert!(plan::day_done(&app.store, "Ders", &days, first, 1).unwrap()[0]);
    }

    #[test]
    fn spec_tui_1_f4_toggles_lang_live_and_persists() {
        let (root, mut app) = setup();
        key(&mut app, KeyCode::F(4));
        assert_eq!(app.store.state("lang").unwrap().as_deref(), Some("en"));
        assert!(
            app.status.contains("English") || app.status.contains("EN"),
            "{}",
            app.status
        );
        let reopened = App::open(root.path()).unwrap();
        assert_eq!(reopened.store.state("lang").unwrap().as_deref(), Some("en"));
        key(&mut app, KeyCode::F(4));
        assert_eq!(app.store.state("lang").unwrap().as_deref(), Some("tr"));
    }

    #[test]
    fn spec_tui_2_subject_switch_refreshes_immediately_and_resets_plan_view() {
        let (root, mut app) = setup();
        fs::create_dir_all(root.path().join("kaynaklar/A_Plan")).unwrap();
        fs::write(
            root.path().join("kaynaklar/A_Plan/notes.md"),
            "Planlı ders.",
        )
        .unwrap();
        write_test_course(root.path(), "A_Plan");
        fs::create_dir_all(root.path().join("kaynaklar/B_Empty")).unwrap();
        fs::write(root.path().join("kaynaklar/B_Empty/notes.md"), "Boş ders.").unwrap();
        sources::index(root.path(), false).unwrap();
        app.refresh().unwrap();

        app.command("/course", "A_Plan").unwrap();
        assert_eq!(app.subjects[app.selected].name, "A_Plan");
        assert!(!app.course.plan.is_empty());
        app.plan_view_day = Some(3);

        app.command("/course", "B_Empty").unwrap();
        assert_eq!(app.subjects[app.selected].name, "B_Empty");
        assert!(app.course.plan.is_empty());
        assert!(app.plan_view_day.is_none());
    }

    #[test]
    fn spec_tui_3_cards_command_in_today_tab_starts_vocab_job() {
        let (root, mut app) = setup();
        write_test_course(root.path(), "Ders");
        plan::set_start(
            &app.store,
            "Ders",
            Local::now().date_naive() - chrono::Days::new(1),
        )
        .unwrap();
        app.refresh().unwrap();
        key(&mut app, KeyCode::Char('5'));
        // Day 2 Task 2 is `/kelime ...`
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        let _ = app.pending.take();
        assert!(
            !app.status.contains("Telegram"),
            "Today tab refused /kelime instead of launching Vocab job: {}",
            app.status
        );
    }

    #[test]
    fn tui_lesson_practice_plan_jump_and_question_mark_help() {
        let (root, mut app) = setup();
        write_test_course(root.path(), "Ders");
        app.refresh().unwrap();

        key(&mut app, KeyCode::Char('?'));
        assert!(app.help);
        key(&mut app, KeyCode::Char('?'));
        assert!(!app.help);

        key(&mut app, KeyCode::Enter);
        assert_eq!(app.editing, Some(Input::Answer));
        key(&mut app, KeyCode::Char('?'));
        assert!(!app.help);
        assert_eq!(app.input, "?");
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.editing, None);

        app.lang = Lang::En;
        app.command("/lesson", "").unwrap();
        assert_eq!(app.awaiting, Some("anlat"));
        app.awaiting = None;

        app.command("/lesson", "Clauses").unwrap();
        assert!(matches!(
            app.pending.as_ref().map(|p| &p.work),
            Some(Work::Ai { kind: JobKind::Lesson { topic }, .. }) if topic == "Clauses"
        ));
        let _ = app.pending.take();

        app.command("/practice", "Grammar 2").unwrap();
        assert!(matches!(
            app.pending.as_ref().map(|p| &p.work),
            Some(Work::Ai { kind: JobKind::Quiz { topic, count: 2 }, .. }) if topic == "Grammar (Quick Check)"
        ));
        let _ = app.pending.take();

        app.command("/plan", "start").unwrap();
        app.command("/plan", "jump 2").unwrap();
        let today = Local::now().date_naive();
        assert_eq!(
            plan::start_date(&app.store, "Ders").unwrap(),
            Some(today - chrono::Days::new(1))
        );
    }
}
