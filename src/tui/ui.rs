use super::app::{App, CoachMode, CoachReply, DenemeChat, Input, tabs};
use crate::{bot, i18n::Lang, plan};
use chrono::{Days, Utc};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState,
        Tabs, Wrap,
    },
};

const ACCENT: Color = Color::Rgb(0x94, 0xe2, 0xd5);
const GOOD: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
const WARN: Color = Color::Rgb(0xfa, 0xb3, 0x87);
const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
const MUTED: Color = Color::Rgb(0x6c, 0x70, 0x86);
const SURFACE: Color = Color::Rgb(0x31, 0x32, 0x44);
const BASE: Color = Color::Rgb(0x1e, 0x1e, 0x2e);
/// Bu genişliğin altında ders listesi gizlenir; içerik için yer açılır.
const NARROW: u16 = 80;

const ONBOARDING: &str = "Başlamak için:\n\
1. kaynaklar/ içinde bir ders klasörü açın, örn. kaynaklar/Matematik\n\
2. Belgelerinizi (.md, .txt, .pdf, .docx) bu klasöre koyun\n\
3. r tuşuyla dizinleyin\n\
4. 2 ile Quiz'e geçip n ile ilk quizi üretin";

const ONBOARDING_EN: &str = "Getting started:\n\
1. Create a course folder under kaynaklar/, e.g. kaynaklar/Calculus\n\
2. Add your study files (.md, .txt, .pdf, .docx) there\n\
3. Press r to index sources\n\
4. Press 2 for Quiz and n to generate your first quiz";

const HELP: &str = "\
Genel    Tab / Shift+Tab, 1–6 sekme · ↑/↓ ders · r dizinle
         F2 Telegram dinleyici · F3 Koç · F4 Dil (TR/EN) · q çık
Sor      Enter soru yaz · Ctrl+Q bu konudan 5 soruluk quiz
Komut    / her sekmeden komut yazar (Telegram ile aynı):
         /quiz [konu n] · /tekrar [n] [orijinal] · /kazanim <konu> [n]
         /kelime <konu> [n] · /anlat <konu> · /deneme [n] · /denemeler
         /plan [baslat tarih | gun N | gec N] · /bugun · /dersler
         /yanlislar · /ders <ad> · /lang <tr|en> · /iptal · /test · /yardim
Quiz     n yeni quiz · Enter kayıtlı sorular · w yanlışları tekrar
         a–e veya 1–5 cevap · Enter sonraki · Esc bitir
Tekrar   t veya Enter bugünkü tekrarları başlat
Deneme   ↑/↓ seç · Enter koçla tartış · d sil (y onay)
         sohbette Tab/Shift+Tab yanlışları gez · Enter gönder
Bugün    ←/→ gün değiştir · ↑/↓ görev · Enter çalıştır · b başlat
         F3 koçu aç/kapat: AI bir sonraki adımı seçip kendisi başlatır
Her yerde PgUp / PgDn kaydır · Esc AI işini iptal eder · F1 / ? yardım";

const HELP_EN: &str = "\
General  Tab / Shift+Tab, 1–6 tabs · ↑/↓ course · r index
         F2 Telegram listener · F3 Coach · F4 Lang (TR/EN) · q quit
Ask      Enter ask a question · Ctrl+Q 5-question quiz on topic
Command  / runs slash commands from any tab (shared with Telegram):
         /quiz [topic n] · /review [n] [exact] · /cards <topic> [n]
         /explain <topic> · /exam [n] · /exams · /today · /courses
         /plan [start date | day N | goto N] · /mistakes · /course <name>
         /practice <t> [n] · /lang <tr|en> · /cancel · /test · /help
Quiz     n new quiz · Enter saved questions · w retry mistakes
         a–e or 1–5 answer · Enter next · Esc finish
Review   t or Enter start due reviews
Exams    ↑/↓ select · Enter discuss with coach · d delete (y confirm)
         in chat Tab/Shift+Tab step mistakes · Enter send
Today    ←/→ change day · ↑/↓ task · Enter run · b start plan
         F3 toggle coach: AI chooses and launches next step
Everywhere PgUp / PgDn scroll · Esc cancel AI · F1 / ? help";

fn panel(title: &str) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Line::styled(
            format!(" {title} "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(MUTED))
}

fn paragraph(frame: &mut Frame, area: Rect, title: &str, text: &str, scroll: u16) {
    panel_text(frame, area, title, markdown(text), scroll);
}

fn find_single_star(after: &str) -> Option<usize> {
    let bytes = after.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'*' {
            if index + 1 < bytes.len() && bytes[index + 1] == b'*' {
                index += 2;
                continue;
            }
            return Some(index);
        }
        index += 1;
    }
    None
}

fn find_inline(text: &str) -> Option<(usize, usize, usize, Modifier, Option<Color>, bool)> {
    for (index, _) in text.char_indices() {
        let rest = &text[index..];
        if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`')
            && end > 0
        {
            return Some((index, 1, end, Modifier::empty(), Some(ACCENT), true));
        }
        if let Some(after) = rest.strip_prefix("***")
            && let Some(end) = after.find("***")
            && end > 0
            && !after[..end].trim().is_empty()
        {
            return Some((
                index,
                3,
                end,
                Modifier::BOLD | Modifier::ITALIC,
                None,
                false,
            ));
        }
        if let Some(after) = rest.strip_prefix("**")
            && let Some(mut end) = after.find("**")
        {
            if after[end..].starts_with("***") {
                end += 1;
            }
            if end > 0 && !after[..end].trim().is_empty() {
                return Some((index, 2, end, Modifier::BOLD, None, false));
            }
        }
        if !rest.starts_with("**")
            && let Some(after) = rest.strip_prefix('*')
            && let Some(end) = find_single_star(after)
            && end > 0
        {
            let inner = &after[..end];
            if !inner.starts_with(char::is_whitespace) && !inner.ends_with(char::is_whitespace) {
                return Some((index, 1, end, Modifier::ITALIC, None, false));
            }
        }
    }
    None
}

fn inline_spans(text: &str, style: Style) -> Vec<Span<'static>> {
    if text.is_empty() {
        return vec![];
    }
    let mut spans = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if let Some((start, marker, len, modifier, color, literal)) = find_inline(remaining) {
            if start > 0 {
                let plain = remaining[..start].replace("**", "");
                if !plain.is_empty() {
                    spans.push(Span::styled(plain, style));
                }
            }
            let inner = &remaining[start + marker..start + marker + len];
            let mut inner_style = style.add_modifier(modifier);
            if let Some(fg) = color {
                inner_style = inner_style.fg(fg);
            }
            if literal {
                spans.push(Span::styled(inner.to_string(), inner_style));
            } else {
                spans.extend(inline_spans(inner, inner_style));
            }
            remaining = &remaining[start + marker * 2 + len..];
        } else {
            let plain = remaining.replace("**", "");
            if !plain.is_empty() {
                spans.push(Span::styled(plain, style));
            }
            break;
        }
    }
    spans
}

fn markdown_line(line: &str) -> Line<'static> {
    let trimmed = line.trim_start();
    if trimmed == "---" {
        return Line::default();
    }
    if let Some(rest) = trimmed.strip_prefix('#') {
        let heading = rest.trim_start_matches('#').trim();
        return Line::from(inline_spans(
            heading,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        let indent = &line[..line.len() - trimmed.len()];
        let mut spans = vec![Span::styled(
            format!("{indent}• "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )];
        spans.extend(inline_spans(rest.trim_start(), Style::default()));
        return Line::from(spans);
    }
    Line::from(inline_spans(line, Style::default()))
}

fn markdown(text: &str) -> Text<'static> {
    if text.is_empty() {
        return Text::default();
    }
    Text::from(text.lines().map(markdown_line).collect::<Vec<_>>())
}

fn panel_text(frame: &mut Frame, area: Rect, title: &str, text: Text<'static>, scroll: u16) {
    frame.render_widget(
        Paragraph::new(text)
            .block(panel(title))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

/// Doğru/toplam oranı yüzde olarak; toplam 0 ise None.
fn percent(correct: i64, total: i64) -> Option<f64> {
    (total > 0).then(|| correct as f64 * 100.0 / total as f64)
}

/// `width` karelik düz metin çubuk; `value` 0–100 arasındaki yüzde.
fn bar(value: f64, width: usize) -> String {
    let filled = ((value / 100.0) * width as f64)
        .round()
        .clamp(0.0, width as f64) as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

/// Renkli çubuk: dolu kısım vurgu rengiyle, kalan kısım soluk.
fn bar_spans(value: f64, width: usize) -> Vec<Span<'static>> {
    let filled = ((value / 100.0) * width as f64)
        .round()
        .clamp(0.0, width as f64) as usize;
    vec![
        Span::styled("█".repeat(filled), Style::default().fg(ACCENT)),
        Span::styled("░".repeat(width - filled), Style::default().fg(MUTED)),
    ]
}

/// "72% (58/80)" biçiminde; cevap yoksa bunu söyler.
fn accuracy_text(correct: i64, total: i64, lang: Lang) -> String {
    match percent(correct, total) {
        Some(value) => format!("{value:>3.0}% ({correct}/{total})"),
        None => lang.pick("henüz cevap yok", "no answers yet").into(),
    }
}

/// Giriş kutusunda genişliğe sığan son karakterler.
fn input_tail(text: &str, width: u16) -> String {
    let keep = width.saturating_sub(3) as usize;
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(keep)..].iter().collect()
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

/// Renkli etiket: kısa, kalın, arka planlı.
fn chip(text: &str, fg: Color, bg: Color) -> Span<'static> {
    Span::styled(
        format!(" {text} "),
        Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
    )
}

/// Tuş ipuçları: her tuş bir "chip", yanında kısa açıklama.
fn keycaps(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (key, label) in pairs {
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default()
                .fg(ACCENT)
                .bg(SURFACE)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {label}   "),
            Style::default().fg(MUTED),
        ));
    }
    Line::from(spans)
}

fn plan_short(app: &App) -> String {
    let total = app.course.plan.len();
    match (&app.plan, app.lang) {
        (plan::Status::Active { day, .. }, Lang::Tr) => format!("Gün {day}/{total}"),
        (plan::Status::Active { day, .. }, Lang::En) => format!("Day {day}/{total}"),
        (plan::Status::NotStarted, _) if total == 0 => {
            app.lang.pick("Esnek ders", "Flexible course").into()
        }
        (plan::Status::NotStarted, _) => app.lang.pick("Plan yok", "No plan").into(),
        (plan::Status::Upcoming { .. }, _) => app.lang.pick("Plan yakında", "Plan upcoming").into(),
        (plan::Status::Finished { .. }, _) => app.lang.pick("Plan bitti", "Plan complete").into(),
    }
}

/// En üst çubuk: uygulama adı, plan günü, bekleyen tekrar ve Telegram durumu.
fn header(app: &App) -> Line<'static> {
    let due: i64 = app.subjects.iter().map(|subject| subject.due).sum();
    let due_chip = if due > 0 {
        let label = match app.lang {
            Lang::Tr => format!("{due} tekrar bekliyor"),
            Lang::En => format!("{due} due"),
        };
        chip(&label, BASE, WARN)
    } else {
        chip(
            app.lang.pick("tekrar yok", "no reviews due"),
            MUTED,
            SURFACE,
        )
    };
    let (telegram, telegram_color) = if app.listener {
        ("● Telegram", GOOD)
    } else {
        ("○ Telegram", MUTED)
    };
    let coach = match app.coach_mode {
        CoachMode::On => chip(app.lang.pick("Koç ●", "Coach ●"), BASE, GOOD),
        CoachMode::Paused => chip(app.lang.pick("Koç beklemede", "Coach paused"), BASE, WARN),
        CoachMode::Off => chip(app.lang.pick("Koç kapalı", "Coach off"), MUTED, SURFACE),
    };
    let lang_chip = chip(app.lang.code().to_ascii_uppercase().as_str(), BASE, TEXT);
    Line::from(vec![
        chip("studia", BASE, ACCENT),
        Span::raw(" "),
        chip(&plan_short(app), TEXT, SURFACE),
        Span::raw(" "),
        due_chip,
        Span::raw(" "),
        coach,
        Span::raw(" "),
        lang_chip,
        Span::raw("  "),
        Span::styled(
            telegram,
            Style::default()
                .fg(telegram_color)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::new().style(Style::default().fg(TEXT).bg(BASE)), area);
    let [top, body, status, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(header(app)), top);
    let right = if body.width < NARROW {
        body
    } else {
        let [subjects, right] =
            Layout::horizontal([Constraint::Percentage(26), Constraint::Percentage(74)])
                .areas(body);
        draw_subjects(frame, subjects, app);
        right
    };
    let [tabs_area, content] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(right);
    frame.render_widget(
        Tabs::new(tabs(app.lang))
            .select(app.tab)
            .style(Style::default().fg(MUTED))
            .highlight_style(
                Style::default()
                    .fg(BASE)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            )
            .divider(" ")
            .padding(" ", " "),
        tabs_area,
    );
    match app.tab {
        0 => draw_ask(frame, content, app),
        1 | 2 | 4 => draw_practice(frame, content, app),
        3 => draw_stats(frame, content, app),
        5 => draw_denemeler(frame, content, app),
        _ => unreachable!(),
    }
    let activity = app.spinner();
    let (line, color) = match &activity {
        Some(text) => (text.clone(), ACCENT),
        None if app.status.starts_with('✓') => (app.status.clone(), GOOD),
        None => (app.status.clone(), WARN),
    };
    frame.render_widget(
        Paragraph::new(Line::styled(format!(" {line}"), Style::default().fg(color))),
        status,
    );
    frame.render_widget(
        Paragraph::new(vec![
            keycaps(&context_keys(app)),
            keycaps(&global_keys(app)),
        ]),
        footer,
    );
    if app.help {
        draw_help(frame, area, app.lang);
    }
}

fn draw_subjects(frame: &mut Frame, area: Rect, app: &App) {
    let heading = app.lang.pick("Dersler", "Courses");
    if app.subjects.is_empty() {
        paragraph(
            frame,
            area,
            heading,
            app.lang.pick(
                "Henüz ders yok.\nkaynaklar/<ders>/ klasörü ekleyin, sonra r ile dizinleyin.",
                "No courses yet.\nAdd a kaynaklar/<course>/ folder, then press r to index.",
            ),
            0,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .subjects
        .iter()
        .map(|subject| {
            let counts = match app.lang {
                Lang::Tr => format!("  {} soru · {} tekrar", subject.questions, subject.due),
                Lang::En => format!("  {} questions · {} due", subject.questions, subject.due),
            };
            ListItem::new(Text::from(vec![
                Line::styled(
                    subject.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Line::styled(counts, Style::default().fg(MUTED)),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(panel(heading))
            .highlight_symbol("▍")
            .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        area,
        &mut state,
    );
}

/// Sor sekmesinin boş durumu: genel durum ve nasıl başlanacağı.
fn dashboard(app: &App) -> Text<'static> {
    let due: i64 = app.subjects.iter().map(|subject| subject.due).sum();
    let questions: i64 = app.subjects.iter().map(|subject| subject.questions).sum();
    let (correct, total) = app
        .daily
        .iter()
        .fold((0i64, 0i64), |(c, t), (_, dc, dt)| (c + dc, t + dt));
    let accuracy = match (percent(correct, total), app.lang) {
        (Some(value), Lang::Tr) => format!("{value:.0}%  (son 7 gün)"),
        (Some(value), Lang::En) => format!("{value:.0}%  (last 7 days)"),
        (None, _) => app.lang.pick("henüz cevap yok", "no answers yet").into(),
    };
    let heading = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
    let item = |label: &str, value: String| {
        Line::from(vec![
            Span::styled(format!("  {label:<14}"), Style::default().fg(MUTED)),
            Span::styled(
                value,
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ])
    };
    match app.lang {
        Lang::Tr => Text::from(vec![
            Line::styled("Bugün", heading),
            item("Tekrar", format!("{due} soru bekliyor")),
            item("Doğruluk", accuracy),
            item("Plan", plan_text(app)),
            item(
                "Soru bankası",
                format!("{questions} soru · {} ders", app.subjects.len()),
            ),
            Line::default(),
            Line::styled("Nasıl başlarım?", heading),
            Line::from("  Enter    bir soru yazın; yanıt seçili dersin kaynaklarından gelir"),
            Line::from("  Ctrl+Q   yanıtın konusundan 5 soruluk quiz üretir"),
            Line::from("  2 · 3    Quiz ve Tekrar sekmeleri · n yeni quiz · t tekrarı başlat"),
            Line::from("  Koç      açıksa sistem konu seçer, anlatır, test eder (F3 ile kapat)"),
            Line::from("  F1 / ?   tüm kısayollar"),
        ]),
        Lang::En => Text::from(vec![
            Line::styled("Today", heading),
            item("Reviews", format!("{due} due")),
            item("Accuracy", accuracy),
            item("Plan", plan_text(app)),
            item(
                "Question bank",
                format!("{questions} questions · {} courses", app.subjects.len()),
            ),
            Line::default(),
            Line::styled("How to start", heading),
            Line::from("  Enter    ask a question grounded in your active course sources"),
            Line::from("  Ctrl+Q   generate a 5-question quiz on the last answer topic"),
            Line::from("  2 · 3    Quiz & Review tabs · n new quiz · t start reviews"),
            Line::from("  Coach    automatically chooses, teaches, and tests (F3 toggle)"),
            Line::from("  F1 / ?   all shortcuts"),
        ]),
    }
}

fn plan_text(app: &App) -> String {
    let total = app.course.plan.len();
    match (&app.plan, app.lang) {
        (plan::Status::Active { day, .. }, Lang::Tr) => format!("Gün {day}/{total}"),
        (plan::Status::Active { day, .. }, Lang::En) => format!("Day {day}/{total}"),
        (plan::Status::NotStarted, _) if total == 0 => app
            .lang
            .pick(
                "esnek ders (plan dosyası yok)",
                "flexible course (no plan file)",
            )
            .into(),
        (plan::Status::NotStarted, _) => app
            .lang
            .pick(
                "başlatılmadı (Bugün sekmesinde b)",
                "not started (press b in Today tab)",
            )
            .into(),
        (plan::Status::Upcoming { start }, Lang::Tr) => {
            format!("{} tarihinde başlayacak", start.format("%d.%m.%Y"))
        }
        (plan::Status::Upcoming { start }, Lang::En) => {
            format!("starts on {}", start.format("%Y-%m-%d"))
        }
        (plan::Status::Finished { .. }, _) => app.lang.pick("tamamlandı", "completed").into(),
    }
}

fn draw_ask(frame: &mut Frame, area: Rect, app: &App) {
    let [answer, input] = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(area);
    if app.answer.is_empty() {
        if app.subjects.is_empty() {
            let body = match app.lang {
                Lang::Tr => ONBOARDING,
                Lang::En => ONBOARDING_EN,
            };
            paragraph(
                frame,
                answer,
                app.lang.pick("Başlarken", "Getting Started"),
                body,
                0,
            );
        } else {
            panel_text(
                frame,
                answer,
                app.lang.pick("Özet", "Summary"),
                dashboard(app),
                0,
            );
        }
    } else {
        paragraph(
            frame,
            answer,
            app.lang.pick("Yanıt · PgUp/PgDn", "Answer · PgUp/PgDn"),
            &app.answer,
            app.scroll,
        );
    }
    let title = if app.editing.is_some() {
        app.lang.pick(
            "Soru · Enter gönder · Esc temizle",
            "Question · Enter send · Esc clear",
        )
    } else {
        app.lang.pick("Soru · Enter yaz", "Question · Enter type")
    };
    paragraph(frame, input, title, &input_tail(&app.input, input.width), 0);
}

fn draw_practice(frame: &mut Frame, content: Rect, app: &App) {
    if app.tab == 4
        && let Some(lesson) = &app.lesson
    {
        draw_lesson(frame, content, app, lesson);
    } else if let Some(input) = &app.editing {
        let prompt = match input {
            Input::Topic => app.lang.pick("Konu (boş: Genel)", "Topic (blank: General)"),
            Input::Count => app.lang.pick(
                "Soru sayısı (1–30, boş: 10)",
                "Question count (1–30, blank: 10)",
            ),
            Input::Answer => app.lang.pick("Soru veya /komut", "Question or /command"),
            Input::DenemeChat => app.lang.pick("Koça soru", "Question for coach"),
            Input::LessonChat => app.lang.pick("Ders hakkında soru", "Question on lesson"),
        };
        let hint = app
            .lang
            .pick("Enter devam · Esc iptal", "Enter continue · Esc cancel");
        paragraph(
            frame,
            content,
            prompt,
            &format!("{}\n\n{hint}", app.input),
            0,
        );
    } else if let Some(session) = &app.session
        && app.session_tab == app.tab
    {
        draw_session(frame, content, app, session);
    } else if app.tab == 4 {
        draw_plan(frame, content, app);
    } else {
        draw_idle(frame, content, app);
    }
}

/// Ders anlatımı ve altındaki takip sohbeti: üstte ders + soru/cevaplar, altta soru kutusu.
fn draw_lesson(frame: &mut Frame, content: Rect, app: &App, lesson: &str) {
    let [panel_area, input] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(content);
    let mut text = lesson.to_string();
    if !app.lesson_turns.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("\n━━━━━━━━━━━━━━━━━━━━━━━━\n");
        let you = app.lang.pick("Sen", "You");
        let waiting = app
            .lang
            .pick("*Yanıt hazırlanıyor…*\n", "*Preparing answer…*\n");
        for turn in &app.lesson_turns {
            text.push('\n');
            text.push_str(&format!("**{you}:** {}\n\n", turn.question));
            match &turn.reply {
                CoachReply::Waiting => text.push_str(waiting),
                CoachReply::Answer(answer) => text.push_str(&format!("{answer}\n")),
                CoachReply::Failed(message) => text.push_str(&format!("⚠ {message}\n")),
            }
        }
    }
    let inner_width = panel_area.width.saturating_sub(2);
    let inner_height = usize::from(panel_area.height.saturating_sub(2));
    let rendered = markdown(&text);
    let total = Paragraph::new(rendered.clone())
        .wrap(Wrap { trim: false })
        .line_count(inner_width);
    let max = u16::try_from(total.saturating_sub(inner_height)).unwrap_or(u16::MAX);
    app.lesson_max_scroll.set(max);
    let offset = if app.lesson_follow {
        max
    } else {
        app.scroll.min(max)
    };
    let label = app.lang.pick("Ders anlatımı", "Lesson");
    let nav = app.lang.pick(
        "↑/↓ PgUp/PgDn kaydır · Boş Enter: quiz",
        "↑/↓ PgUp/PgDn scroll · Blank Enter: quiz",
    );
    let title = if app.lesson_topic.is_empty() {
        format!("{label} · {nav}")
    } else {
        format!("{label} ({}) · {nav}", app.lesson_topic)
    };
    panel_text(frame, panel_area, &title, rendered, offset);
    let input_title = if app.input.is_empty() {
        app.lang.pick(
            "Kafana takılanı yaz ve Enter'a bas · Boş Enter: quize geç · Esc: kapat",
            "Ask a question & Enter · Blank Enter: start quiz · Esc: close",
        )
    } else {
        app.lang.pick(
            "Soru · Enter gönder · Esc temizle",
            "Question · Enter send · Esc clear",
        )
    };
    paragraph(
        frame,
        input,
        input_title,
        &input_tail(&app.input, input.width),
        0,
    );
}

/// Aktif quiz: renkli ilerleme çubuğu, soru ya da geri bildirim.
fn draw_session(frame: &mut Frame, area: Rect, app: &App, session: &bot::Session) {
    let total = session.questions.len();
    let answered = session.position.min(total);
    let ratio = percent(answered as i64, total as i64).unwrap_or(0.0);
    let mut progress = vec![Span::styled(
        app.lang.pick("İlerleme  ", "Progress  "),
        Style::default().fg(MUTED),
    )];
    progress.extend(bar_spans(ratio, 16));
    progress.push(Span::styled(
        match app.lang {
            Lang::Tr => format!("  {answered}/{total} · doğru {}", session.correct()),
            Lang::En => format!("  {answered}/{total} · correct {}", session.correct()),
        },
        Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
    ));
    let body = match &app.feedback {
        Some(feedback) => format!(
            "{feedback}\n\n{}",
            app.lang.pick(
                "Enter: sonraki soru / sonucu göster",
                "Enter: next question / show summary",
            )
        ),
        None => match session.questions.get(answered) {
            Some(question) => match app.lang {
                Lang::Tr => format!(
                    "Soru {}/{total}\n\n{}\n\na–e / 1–5 cevap · Esc bitir",
                    answered + 1,
                    bot::question_text(question)
                ),
                Lang::En => format!(
                    "Question {}/{total}\n\n{}\n\na–e / 1–5 answer · Esc finish",
                    answered + 1,
                    bot::question_text(question)
                ),
            },
            None => String::new(),
        },
    };
    let mut lines = vec![Line::from(progress), Line::default()];
    lines.extend(markdown(&body).lines);
    let tab_names = tabs(app.lang);
    panel_text(
        frame,
        area,
        &format!("{} · {}", tab_names[app.tab], app.session_subject),
        Text::from(lines),
        app.scroll,
    );
}

/// Quiz ve tekrar sekmelerinin boştaki hali: özet ve tuş ipuçları.
fn draw_idle(frame: &mut Frame, content: Rect, app: &App) {
    let hints = if app.tab == 1 {
        let mut text = String::from(app.lang.pick(
            "n: yeni quiz (konu ve soru sayısı)\nEnter: kayıtlı sorularla başla",
            "n: new quiz (topic and count)\nEnter: start saved questions",
        ));
        if app.wrong_count() > 0 {
            text.push_str(&match app.lang {
                Lang::Tr => format!("\nw: son quizdeki {} yanlışı tekrar et", app.wrong_count()),
                Lang::En => format!("\nw: retry {} mistake(s) from last quiz", app.wrong_count()),
            });
        }
        text
    } else {
        let due = app
            .subjects
            .get(app.selected)
            .map_or(0, |subject| subject.due);
        match app.lang {
            Lang::Tr => {
                format!(
                    "Bugün {due} soru tekrar bekliyor.\nt veya Enter: en fazla 10 soruyla başla"
                )
            }
            Lang::En => {
                format!("{due} question(s) due today.\nt or Enter: start with up to 10 questions")
            }
        }
    };
    let text = if app.summary.is_empty() || app.session_tab != app.tab {
        hints
    } else {
        format!("{}\n\n{hints}", app.summary)
    };
    let tab_names = tabs(app.lang);
    paragraph(frame, content, tab_names[app.tab], &text, app.scroll);
}

/// Bugün sekmesinin üstünde koçun durumu, sıradaki adımı, nedeni ve son özeti.
fn draw_coach(frame: &mut Frame, area: Rect, app: &App) {
    let mode = match app.coach_mode {
        CoachMode::On => app.lang.pick("açık", "on"),
        CoachMode::Paused => app
            .lang
            .pick("beklemede · F3 ile sürdür", "paused · F3 to resume"),
        CoachMode::Off => app.lang.pick("kapalı · F3 ile aç", "off · F3 to enable"),
    };
    let (step, reason) = match &app.coach_step {
        Some(step) => (step.label.clone(), step.reason.clone()),
        None => (
            app.lang.pick("henüz seçilmedi", "not chosen yet").into(),
            String::new(),
        ),
    };
    let note = if app.coach_note.is_empty() {
        "—".to_string()
    } else {
        app.coach_note.clone()
    };
    let label = |text: &str| Span::styled(format!("{text:<9}"), Style::default().fg(MUTED));
    let row = |title: &str, value: &str| {
        let mut spans = vec![label(title)];
        spans.extend(inline_spans(value, Style::default()));
        Line::from(spans)
    };
    let text = Text::from(vec![
        Line::from(vec![
            label(app.lang.pick("Durum", "Status")),
            Span::styled(mode, Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
        ]),
        row(app.lang.pick("Sıradaki", "Next"), &step),
        row(app.lang.pick("Neden", "Why"), &reason),
        row(app.lang.pick("Son", "Last"), &note),
    ]);
    panel_text(frame, area, app.lang.pick("Koç", "Coach"), text, 0);
}

fn draw_plan(frame: &mut Frame, content: Rect, app: &App) {
    let [coach_area, content] =
        Layout::vertical([Constraint::Length(6), Constraint::Min(1)]).areas(content);
    draw_coach(frame, coach_area, app);
    let summary = if app.session_tab == app.tab {
        app.summary.as_str()
    } else {
        ""
    };
    let [summary_area, tasks] = Layout::vertical([
        Constraint::Length(if summary.is_empty() { 0 } else { 4 }),
        Constraint::Min(1),
    ])
    .areas(content);
    paragraph(
        frame,
        summary_area,
        app.lang.pick("Sonuç", "Result"),
        summary,
        0,
    );

    let total_days = app.course.plan.len();
    if total_days == 0 {
        paragraph(
            frame,
            tasks,
            app.lang.pick(
                "Çalışma Planı · Esnek Ders",
                "Study Plan · Flexible Course",
            ),
            app.lang.pick(
                "Bu ders için çalışma planı tanımlı değil (isteğe bağlı: kaynaklar/<ders>/course.json).\n\nDoğrudan Soru/Özet (1), Quiz (2), Tekrar (3) veya Deneme (5) sekmelerini kullanabilirsiniz.",
                "No study plan defined for this course (optional: kaynaklar/<course>/course.json).\n\nYou can use Ask (1), Quiz (2), Review (3), or Exams (5) directly.",
            ),
            0,
        );
        return;
    }

    let view_day = app.plan_day();
    let info = plan::day(&app.course.plan, view_day);
    let subject_name = app
        .subjects
        .get(app.selected)
        .map(|s| s.name.as_str())
        .unwrap_or("");
    let start_opt = if subject_name.is_empty() {
        None
    } else {
        plan::start_date(&app.store, subject_name).ok().flatten()
    };
    let done = match start_opt {
        Some(start) if !subject_name.is_empty() => {
            plan::day_done(&app.store, subject_name, &app.course.plan, start, view_day)
                .unwrap_or_default()
        }
        _ => Vec::new(),
    };

    let tag = match (&app.plan, app.lang) {
        (
            plan::Status::Active {
                day: active_day,
                start,
                ..
            },
            Lang::Tr,
        ) => {
            let date = plan::date_of(*start, view_day).format("%d.%m.%Y");
            if view_day == *active_day {
                format!("{date} · Bugün")
            } else if view_day < *active_day {
                format!("{date} · Geçmiş (Bugün: Gün {active_day})")
            } else {
                format!("{date} · İleriki (Bugün: Gün {active_day})")
            }
        }
        (
            plan::Status::Active {
                day: active_day,
                start,
                ..
            },
            Lang::En,
        ) => {
            let date = plan::date_of(*start, view_day).format("%Y-%m-%d");
            if view_day == *active_day {
                format!("{date} · Today")
            } else if view_day < *active_day {
                format!("{date} · Past (Today: Day {active_day})")
            } else {
                format!("{date} · Upcoming (Today: Day {active_day})")
            }
        }
        (plan::Status::NotStarted, _) => app
            .lang
            .pick(
                "Plan başlatılmadı (b: bugünden başlat)",
                "Not started (press b to start today)",
            )
            .into(),
        (plan::Status::Upcoming { start }, Lang::Tr) => {
            let date = plan::date_of(*start, view_day).format("%d.%m.%Y");
            format!("{date} · Plan yakında")
        }
        (plan::Status::Upcoming { start }, Lang::En) => {
            let date = plan::date_of(*start, view_day).format("%Y-%m-%d");
            format!("{date} · Upcoming")
        }
        (plan::Status::Finished { start }, Lang::Tr) => {
            let date = plan::date_of(*start, view_day).format("%d.%m.%Y");
            format!("{date} · Plan tamamlandı")
        }
        (plan::Status::Finished { start }, Lang::En) => {
            let date = plan::date_of(*start, view_day).format("%Y-%m-%d");
            format!("{date} · Completed")
        }
    };

    let title_name = info.map_or("", |i| i.title.as_str());
    let title = match app.lang {
        Lang::Tr => format!(
            "Gün {view_day}/{total_days} · {tag} · {title_name} · ←/→ gün · ↑/↓ görev · Enter çalıştır"
        ),
        Lang::En => format!(
            "Day {view_day}/{total_days} · {tag} · {title_name} · ←/→ day · ↑/↓ task · Enter run"
        ),
    };

    let items: Vec<ListItem> = info
        .map(|info| {
            info.tasks
                .iter()
                .enumerate()
                .map(|(index, task)| {
                    let mark = if done.get(index).copied().unwrap_or(false) {
                        "✅"
                    } else {
                        "⬜"
                    };
                    ListItem::new(format!("{mark} {}", task.label))
                })
                .collect()
        })
        .unwrap_or_default();

    if items.is_empty() {
        paragraph(
            frame,
            tasks,
            app.lang.pick("Bugün", "Today"),
            app.lang.pick(
                "Bugün dinlenme / sınav günü.",
                "Exam / rest day — no scheduled tasks today.",
            ),
            0,
        );
    } else {
        let mut state = ListState::default().with_selected(Some(app.task_selected));
        frame.render_stateful_widget(
            List::new(items)
                .block(panel(&title))
                .highlight_symbol("▍")
                .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            tasks,
            &mut state,
        );
    }
}

/// Denemeler: seçilebilir geçmiş listesi; Enter ile açılan koç sohbeti aynı yerde görünür.
fn draw_denemeler(frame: &mut Frame, content: Rect, app: &App) {
    match &app.deneme_chat {
        Some(chat) => draw_deneme_chat(frame, content, app, chat),
        None => draw_deneme_list(frame, content, app),
    }
}

fn draw_deneme_list(frame: &mut Frame, content: Rect, app: &App) {
    if app.deneme_items.is_empty() {
        paragraph(
            frame,
            content,
            app.lang.pick("Geçmiş denemeler", "Past exams"),
            app.lang.pick(
                "Henüz tamamlanmış bir deneme yok.\n\nSor sekmesinde /deneme yazarak yeni bir deneme başlatabilirsiniz.",
                "No completed mock exams yet.\n\nRun /exam in the Ask tab to start one.",
            ),
            0,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .deneme_items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            ListItem::new(markdown_line(
                bot::deneme_line_in(index + 1, item, app.lang).trim_end(),
            ))
        })
        .collect();
    let title = match app.lang {
        Lang::Tr => format!(
            "Geçmiş denemeler · seçili {}/{}",
            app.deneme_selected + 1,
            app.deneme_items.len()
        ),
        Lang::En => format!(
            "Past exams · selected {}/{}",
            app.deneme_selected + 1,
            app.deneme_items.len()
        ),
    };
    let mut state = ListState::default().with_selected(Some(app.deneme_selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(panel(&title))
            .highlight_symbol("▍")
            .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        content,
        &mut state,
    );
}

/// Koç sohbeti: üstte sayısal döküm, altta eskiden yeniye turlar; takip açıkken en alta sabitlenir.
fn draw_deneme_chat(frame: &mut Frame, content: Rect, app: &App, chat: &DenemeChat) {
    let [panel_area, input] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(content);
    let mut text = chat.breakdown.clone();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("━━━━━━━━━━━━━━━━━━━━━━━━\n");
    let you = app.lang.pick("Sen", "You");
    let coach = app.lang.pick("Koç", "Coach");
    for turn in &chat.turns {
        text.push('\n');
        if turn.question.is_empty() {
            text.push_str(match app.lang {
                Lang::Tr => "Sen: denemeyi değerlendirmesini istedin.\n",
                Lang::En => "You: requested exam evaluation.\n",
            });
        } else {
            text.push_str(&format!("{you}: {}\n", turn.question));
        }
        match &turn.reply {
            CoachReply::Waiting => text.push_str(&format!("{coach}: …\n")),
            CoachReply::Answer(answer) => text.push_str(&format!("{coach}: {answer}\n")),
            CoachReply::Failed(message) => text.push_str(&format!("⚠ {message}\n")),
        }
    }
    // Sarılmış toplam satır sayısı; panelin iç yüksekliğiyle farkı en büyük ofsettir.
    let inner_width = panel_area.width.saturating_sub(2);
    let inner_height = usize::from(panel_area.height.saturating_sub(2));
    let rendered = markdown(&text);
    let total = Paragraph::new(rendered.clone())
        .wrap(Wrap { trim: false })
        .line_count(inner_width);
    let max = u16::try_from(total.saturating_sub(inner_height)).unwrap_or(u16::MAX);
    chat.max_scroll.set(max);
    let offset = if chat.follow {
        max
    } else {
        chat.scroll.min(max)
    };
    let title = format!(
        "{} · {} · {coach}",
        chat.label,
        wrong_position(chat, app.lang)
    );
    panel_text(frame, panel_area, &title, rendered, offset);
    let title = if app.input.is_empty() {
        app.lang.pick(
            "Soru · Enter gönder · Esc sohbetten çık",
            "Question · Enter send · Esc exit chat",
        )
    } else {
        app.lang.pick(
            "Soru · Enter gönder · Esc temizle",
            "Question · Enter send · Esc clear",
        )
    };
    paragraph(frame, input, title, &input_tail(&app.input, input.width), 0);
}

/// Sohbet başlığındaki yanlış konumu: odaktaki soru ya da yanlış sayısı.
fn wrong_position(chat: &DenemeChat, lang: Lang) -> String {
    match chat
        .focus
        .and_then(|index| chat.wrongs.get(index).map(|wrong| (index, wrong)))
    {
        Some((index, wrong)) => match lang {
            Lang::Tr => format!(
                "Yanlış {}/{} (Soru {} · {})",
                index + 1,
                chat.wrongs.len(),
                wrong.number,
                wrong.category
            ),
            Lang::En => format!(
                "Mistake {}/{} (Q{} · {})",
                index + 1,
                chat.wrongs.len(),
                wrong.number,
                wrong.category
            ),
        },
        None if chat.wrongs.is_empty() => lang.pick("Yanlış yok", "No mistakes").into(),
        None => match lang {
            Lang::Tr => format!("{} yanlış · Tab ile incele", chat.wrongs.len()),
            Lang::En => format!("{} mistake(s) · Tab to inspect", chat.wrongs.len()),
        },
    }
}

fn draw_stats(frame: &mut Frame, content: Rect, app: &App) {
    // The daily panel needs seven lines plus its border; the other two share the rest.
    let [sessions, middle, weakest] = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(9),
        Constraint::Min(4),
    ])
    .areas(content);
    let rows = app.stats.iter().map(|session| {
        Row::new([
            session.started.format("%Y-%m-%d %H:%M").to_string(),
            session.subject.clone(),
            session.topic.clone(),
            format!("{}/{}", session.correct, session.total),
            format!("{:.1}", session.minutes),
        ])
    });
    let mut selected = TableState::default().with_selected(
        (!app.stats.is_empty())
            .then_some((app.scroll as usize).min(app.stats.len().saturating_sub(1))),
    );
    let headers = match app.lang {
        Lang::Tr => ["Tarih (UTC)", "Ders", "Konu", "Skor", "Dakika"],
        Lang::En => ["Date (UTC)", "Course", "Topic", "Score", "Minutes"],
    };
    frame.render_stateful_widget(
        Table::new(
            rows,
            [
                Constraint::Length(16),
                Constraint::Percentage(23),
                Constraint::Percentage(30),
                Constraint::Length(7),
                Constraint::Length(7),
            ],
        )
        .header(Row::new(headers).style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)))
        .block(panel(
            app.lang
                .pick("Son 10 oturum · PgUp/PgDn", "Last 10 sessions · PgUp/PgDn"),
        )),
        sessions,
        &mut selected,
    );
    let [subjects, daily] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(middle);
    paragraph(
        frame,
        subjects,
        app.lang.pick("Dersler · doğruluk", "Courses · accuracy"),
        &subject_lines(app),
        0,
    );
    paragraph(
        frame,
        daily,
        app.lang.pick("Son 7 gün", "Last 7 days"),
        &daily_lines(app),
        0,
    );
    let title = match app.subjects.get(app.selected) {
        Some(subject) => format!(
            "{} · {}",
            app.lang.pick("En zayıf konular", "Weakest topics"),
            subject.name
        ),
        None => app.lang.pick("En zayıf konular", "Weakest topics").into(),
    };
    paragraph(frame, weakest, &title, &weakest_lines(app), 0);
}

fn subject_lines(app: &App) -> String {
    if app.subjects.is_empty() {
        return app.lang.pick("Henüz ders yok.", "No courses yet.").into();
    }
    app.subjects
        .iter()
        .map(|subject| {
            let (correct, total) = app
                .accuracy
                .iter()
                .find(|(name, _, _)| name == &subject.name)
                .map_or((0, 0), |(_, correct, total)| (*correct, *total));
            match app.lang {
                Lang::Tr => format!(
                    "{:<10} {} {}\n    bugün {} tekrar · toplam {} tekrar",
                    subject.name,
                    bar(percent(correct, total).unwrap_or(0.0), 10),
                    accuracy_text(correct, total, app.lang),
                    subject.due,
                    subject.reviews
                ),
                Lang::En => format!(
                    "{:<10} {} {}\n    due today {} · total {} reviews",
                    subject.name,
                    bar(percent(correct, total).unwrap_or(0.0), 10),
                    accuracy_text(correct, total, app.lang),
                    subject.due,
                    subject.reviews
                ),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn daily_lines(app: &App) -> String {
    let today = Utc::now().date_naive();
    (0..7)
        .rev()
        .map(|back| {
            let day = today.checked_sub_days(Days::new(back)).unwrap_or(today);
            let key = day.format("%Y-%m-%d").to_string();
            let label = day.format("%d.%m").to_string();
            match app.daily.iter().find(|(date, _, _)| *date == key) {
                Some((_, correct, total)) => format!(
                    "{label}  {} {}",
                    bar(percent(*correct, *total).unwrap_or(0.0), 10),
                    accuracy_text(*correct, *total, app.lang)
                ),
                None => format!("{label}  —"),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn weakest_lines(app: &App) -> String {
    if app.weakest.is_empty() {
        return app
            .lang
            .pick(
                "Yeterli veri yok: konu başına en az 3 cevap gerekir.",
                "Not enough data: at least 3 answers per topic required.",
            )
            .into();
    }
    app.weakest
        .iter()
        .map(|(topic, correct, total)| {
            format!(
                "{topic}\n  {} {}",
                bar(percent(*correct, *total).unwrap_or(0.0), 12),
                accuracy_text(*correct, *total, app.lang)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Aktif bağlama göre alt satırdaki tuş ipuçları.
fn context_keys(app: &App) -> Vec<(&'static str, &'static str)> {
    let lang = app.lang;
    if let Some(input) = app.editing {
        return match input {
            Input::Answer => vec![
                ("Enter", lang.pick("gönder", "send")),
                ("Esc", lang.pick("vazgeç", "cancel")),
                ("Ctrl+U", lang.pick("temizle", "clear")),
                ("Backspace", lang.pick("sil", "del")),
            ],
            Input::Topic | Input::Count => vec![
                ("Enter", lang.pick("devam", "next")),
                ("Esc", lang.pick("vazgeç", "cancel")),
                ("Backspace", lang.pick("sil", "del")),
            ],
            Input::DenemeChat => vec![
                ("Enter", lang.pick("gönder", "send")),
                ("Tab", lang.pick("sonraki yanlış", "next mistake")),
                ("Shift+Tab", lang.pick("önceki", "prev")),
                ("↑/↓", lang.pick("kaydır", "scroll")),
                ("PgUp/PgDn", lang.pick("hızlı kaydır", "fast scroll")),
                ("Esc", lang.pick("çık", "exit")),
            ],
            Input::LessonChat => vec![
                (
                    lang.pick("Yaz + Enter", "Type + Enter"),
                    lang.pick("soru sor", "ask"),
                ),
                (
                    lang.pick("Boş Enter", "Blank Enter"),
                    lang.pick("quiz'e geç", "quiz"),
                ),
                ("↑/↓ PgUp/PgDn", lang.pick("kaydır", "scroll")),
                ("Esc", lang.pick("temizle / kapat", "clear / close")),
            ],
        };
    }
    if app.tab == 4 && app.lesson.is_some() {
        return vec![
            ("↑/↓ PgUp/PgDn", lang.pick("kaydır", "scroll")),
            ("Enter", lang.pick("quiz'e geç", "quiz")),
            ("Esc", lang.pick("kapat", "close")),
        ];
    }
    if app.session.is_some() && app.session_tab == app.tab {
        return if app.feedback.is_some() {
            vec![
                ("Enter", lang.pick("sonraki soru", "next question")),
                ("Esc", lang.pick("quizi bitir", "finish quiz")),
            ]
        } else {
            vec![
                ("a–e / 1–5", lang.pick("cevap", "answer")),
                ("Esc", lang.pick("quizi bitir", "finish quiz")),
                ("PgUp/PgDn", lang.pick("kaydır", "scroll")),
            ]
        };
    }
    match app.tab {
        0 => vec![
            ("Enter", lang.pick("yaz", "ask")),
            ("Ctrl+Q", lang.pick("bu konudan quiz", "quiz topic")),
            ("PgUp/PgDn", lang.pick("kaydır", "scroll")),
        ],
        1 if app.wrong_count() > 0 => vec![
            ("n", lang.pick("yeni quiz", "new quiz")),
            ("Enter", lang.pick("kayıtlı sorular", "saved questions")),
            ("w", lang.pick("yanlışları tekrar", "retry mistakes")),
        ],
        1 => vec![
            ("n", lang.pick("yeni quiz", "new quiz")),
            ("Enter", lang.pick("kayıtlı sorular", "saved questions")),
        ],
        2 => vec![
            ("t / Enter", lang.pick("tekrarı başlat", "start review")),
            ("↑/↓", lang.pick("ders", "course")),
        ],
        // Denemeler: liste ve koç sohbeti aynı sekmede; sohbet açıkken editing dalı yukarıda.
        3 => vec![
            ("PgUp/PgDn", lang.pick("kaydır", "scroll")),
            ("↑/↓", lang.pick("ders", "course")),
        ],
        5 if app.deneme_delete.is_some() => vec![
            ("y", lang.pick("sil", "delete")),
            (
                lang.pick("başka tuş", "any key"),
                lang.pick("vazgeç", "cancel"),
            ),
        ],
        5 => vec![
            ("↑/↓", lang.pick("deneme", "exam")),
            ("Enter", lang.pick("koçla tartış", "coach chat")),
            ("d", lang.pick("sil", "delete")),
        ],
        _ => vec![
            ("←/→", lang.pick("gün", "day")),
            ("↑/↓", lang.pick("görev", "task")),
            ("Enter", lang.pick("çalıştır", "run")),
            ("b", lang.pick("bugün / başlat", "today / start")),
        ],
    }
}

fn global_keys(app: &App) -> Vec<(&'static str, &'static str)> {
    let lang = app.lang;
    vec![
        ("Tab / 1–6", lang.pick("sekme", "tabs")),
        ("r", lang.pick("dizinle", "index")),
        (
            "F2",
            if app.listener {
                lang.pick("Telegram açık", "Telegram on")
            } else {
                lang.pick("Telegram kapalı", "Telegram off")
            },
        ),
        (
            "F3",
            match app.coach_mode {
                CoachMode::On => lang.pick("Koç açık", "Coach on"),
                CoachMode::Paused => lang.pick("Koç sürdür", "Coach resume"),
                CoachMode::Off => lang.pick("Koç kapalı", "Coach off"),
            },
        ),
        ("F4", lang.pick("EN", "TR")),
        ("F1 / ?", lang.pick("yardım", "help")),
        ("q", lang.pick("çık", "quit")),
    ]
}

fn draw_help(frame: &mut Frame, area: Rect, lang: Lang) {
    let popup = centered(area, 72, 19);
    frame.render_widget(Clear, popup);
    let body = match lang {
        Lang::Tr => HELP,
        Lang::En => HELP_EN,
    };
    paragraph(
        frame,
        popup,
        lang.pick(
            "Yardım · F1 / ? veya Esc kapatır",
            "Help · F1 / ? or Esc to close",
        ),
        body,
        0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Question;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn render(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn app_with_subject() -> (tempfile::TempDir, App) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("kaynaklar/Ders")).unwrap();
        std::fs::write(root.path().join("kaynaklar/Ders/ders.md"), "Kaynak.").unwrap();
        let app = App::open(root.path()).unwrap();
        (root, app)
    }

    #[test]
    fn every_tab_and_the_help_overlay_render_at_normal_and_small_sizes() {
        let (_root, mut app) = app_with_subject();
        for (width, height) in [(100, 30), (80, 24), (60, 16)] {
            for tab in 0..crate::tui::app::tabs(app.lang).len() {
                app.tab = tab;
                assert!(!render(&app, width, height).is_empty());
            }
            app.tab = 3;
            app.help = true;
            assert!(render(&app, width, height).contains("Yardım"));
            app.help = false;
        }
    }

    #[test]
    fn empty_workspace_shows_the_first_run_guide() {
        let root = tempfile::tempdir().unwrap();
        let app = App::open(root.path()).unwrap();
        assert!(render(&app, 100, 30).contains("Başlamak için"));
    }

    #[test]
    fn subject_workspace_opens_on_the_summary_dashboard() {
        let (_root, app) = app_with_subject();
        let screen = render(&app, 100, 30);
        assert!(screen.contains("Özet"));
        assert!(screen.contains("Nasıl başlarım?"));
    }

    #[test]
    fn coach_chat_shows_breakdown_above_the_reply_at_small_sizes() {
        let (_root, mut app) = app_with_subject();
        app.tab = super::super::app::DENEME_TAB;
        app.deneme_chat = Some(DenemeChat {
            session: 1,
            label: "Deneme 1: 1/80 doğru".into(),
            breakdown: "Cevaplanan: 1/80\n".into(),
            turns: vec![super::super::app::DenemeTurn {
                question: "Neden?".into(),
                reply: CoachReply::Answer("Gramer zayıf.".into()),
            }],
            wrongs: vec![],
            focus: None,
            follow: true,
            scroll: 0,
            max_scroll: std::cell::Cell::new(0),
        });
        app.editing = Some(Input::DenemeChat);
        let screen = render(&app, 100, 30);
        let breakdown_at = screen.find("Cevaplanan").unwrap();
        let reply_at = screen.find("Gramer zayıf.").unwrap();
        assert!(breakdown_at < reply_at, "{screen}");
        render(&app, 60, 16);
    }

    #[test]
    fn coach_chat_follows_the_newest_reply_until_scrolled_to_the_top() {
        let (_root, mut app) = app_with_subject();
        app.tab = super::super::app::DENEME_TAB;
        let breakdown: String = (1..=60).map(|i| format!("Döküm satırı {i:02}\n")).collect();
        app.deneme_chat = Some(DenemeChat {
            session: 1,
            label: "Deneme 1: 1/80 doğru".into(),
            breakdown,
            turns: vec![super::super::app::DenemeTurn {
                question: "Neden?".into(),
                reply: CoachReply::Answer("Gramer zayıf.".into()),
            }],
            wrongs: vec![],
            focus: None,
            follow: true,
            scroll: 0,
            max_scroll: std::cell::Cell::new(0),
        });
        app.editing = Some(Input::DenemeChat);
        let screen = render(&app, 60, 16);
        assert!(screen.contains("Gramer zayıf."), "{screen}");
        assert!(!screen.contains("Döküm satırı 01"), "{screen}");

        let chat = app.deneme_chat.as_mut().unwrap();
        chat.follow = false;
        let screen = render(&app, 60, 16);
        assert!(screen.contains("Döküm satırı 01"), "{screen}");
    }

    #[test]
    fn quiz_session_shows_progress_then_feedback_with_the_next_hint() {
        let (_root, mut app) = app_with_subject();
        let question = Question {
            question: "Soru?".into(),
            choices: ["a", "b", "c", "d"].map(String::from).into(),
            answer_index: 1,
            explanation: "Doğru cevap: 'b'. Açıklama".into(),
            source: "ders.md".into(),
            topic: None,
        };
        app.store
            .add_questions("Ders", "Konu", vec![question])
            .unwrap();
        app.key(press(KeyCode::Char('2'))).unwrap();
        app.key(press(KeyCode::Enter)).unwrap();
        assert!(render(&app, 100, 30).contains("İlerleme"));
        app.key(press(KeyCode::Char('a'))).unwrap();
        assert!(render(&app, 100, 30).contains("Enter: sonraki soru"));
    }

    #[test]
    fn lesson_and_markdown_render_bold_italic_headings_and_bullets_without_raw_markers() {
        let (_root, mut app) = app_with_subject();
        app.tab = 4;
        app.lesson = Some(
            "### **1) Konunun Özü**\n\
             - **Zıtlık:** *Although* ve `despite` bağlaçları.\n\
             *Örnek (**Example**):* ***Çok önemli*** kural."
                .into(),
        );
        let screen = render(&app, 100, 30);
        assert!(screen.contains("1) Konunun Özü"));
        assert!(screen.contains("• Zıtlık: Although ve despite bağlaçları."));
        assert!(!screen.contains("**"));
        assert!(!screen.contains("###"));

        let parsed = markdown(app.lesson.as_deref().unwrap());
        assert!(
            parsed.lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(parsed.lines[0].spans[0].style.fg, Some(ACCENT));
        assert!(
            parsed.lines[1]
                .spans
                .iter()
                .any(|s| s.content == "Zıtlık:" && s.style.add_modifier.contains(Modifier::BOLD))
        );
        assert!(
            parsed.lines[1].spans.iter().any(
                |s| s.content == "Although" && s.style.add_modifier.contains(Modifier::ITALIC)
            )
        );
        assert!(
            parsed.lines[1]
                .spans
                .iter()
                .any(|s| s.content == "despite" && s.style.fg == Some(ACCENT))
        );
    }

    #[test]
    fn spec_tui_1_ui_renders_english_when_f4_toggled() {
        let (_root, mut app) = app_with_subject();
        assert!(render(&app, 100, 30).contains("Özet"));
        app.key(press(KeyCode::F(4))).unwrap();
        let screen = render(&app, 100, 30);
        assert!(screen.contains("Summary"), "{screen}");
        assert!(screen.contains("Today"), "{screen}");
        assert!(screen.contains("Courses"), "{screen}");
        app.help = true;
        let help_screen = render(&app, 100, 30);
        assert!(help_screen.contains("Help"), "{help_screen}");
    }
}
