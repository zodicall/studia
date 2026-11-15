use super::app::{
    ANKI_TAB, AnkiSession, AnkiView, App, CoachMode, CoachReply, DenemeChat, Input, PLAN_TAB,
    PlanChat, tabs,
};
use super::modals::{AiSettingsModal, Modal, PlanEditModal, PlanEditPrompt, TASK_CMD_KINDS};
use crate::{bot, config::AiProvider, fsrs_engine, i18n::Lang, plan};
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
1. resources/ içinde bir ders klasörü açın, örn. resources/Matematik\n\
2. Belgelerinizi (.md, .txt, .pdf, .docx) bu klasöre koyun\n\
3. r tuşuyla dizinleyin\n\
4. 2 ile Quiz'e geçip n ile ilk quizi üretin";

const ONBOARDING_EN: &str = "Getting started:\n\
1. Create a course folder under resources/, e.g. resources/Calculus\n\
2. Add your study files (.md, .txt, .pdf, .docx) there\n\
3. Press r to index sources\n\
4. Press 2 for Quiz and n to generate your first quiz";

const HELP: &str = "\
Genel    Tab / Shift+Tab, 1–8 sekme · ↑/↓ ders · r dizinle
         F2 Telegram · F3 Koç · F4 Dil · F5 AI/Model · q çık
Sor      Enter soru yaz · ↑/↓ PgUp/PgDn kaydır · Ctrl+Q bu konudan 5 soruluk quiz
Komut    / her sekmeden komut yazar (Telegram ile aynı):
         /quiz [konu n] · /tekrar [n] [orijinal] · /kazanim <konu> [n]
         /kartlar · /kart <konu> [n] · /kulucka · /strateji · /kelime <konu> [n]
         /anlat <konu> · /deneme [n] · /denemeler · /bugun · /dersler
         /plan [baslat tarih | gun N | gec N | olustur | duzenle]
         /ai · /model <ad> · /effort <seviye> · /yanlislar · /ders <ad> · /lang <tr|en> · /iptal · /test · /yardim
Quiz     n yeni quiz · Enter kayıtlı sorular · w yanlışları tekrar
         a–e veya 1–5 cevap · yanlışsa c Cuotiben analizi · Enter sonraki · Esc bitir
Tekrar   t veya Enter bugünkü tekrarları başlat
Deneme   ↑/↓ seç · Enter koçla tartış · d sil (y onay)
         sohbette Tab/Shift+Tab yanlışları gez · Ctrl+F Cuotiben · Enter gönder
Bugün    ←/→ gün değiştir · ↑/↓ görev · Enter çalıştır · b başlat
         g Planla sekmesini aç · e planı elle düzenle
Planla   Enter / n AI ile plan tartış · ←/→ taslak gün · Ctrl+B bugünden başlat
         Ctrl+S taslağı kaydet · Ctrl+N sıfırla · Esc çık
Kartlar  Enter FSRS çalış · n AI kart üret · t/←/→ etiket · h High-Yield
         k Kuluçka · s Strateji Defteri · x Anki dışa aktar
         tekrarda Space/Enter cevabı aç · 1 Hatırlamadım · 2 Hatırladım · r Sülük AI onar
         F3 koçu aç/kapat · F5 AI ve model ayarları
Her yerde PgUp / PgDn kaydır · Esc AI işini iptal eder · F1 / ? yardım";

const HELP_EN: &str = "\
General  Tab / Shift+Tab, 1–8 tabs · ↑/↓ course · r index
         F2 Telegram listener · F3 Coach · F4 Lang (TR/EN) · F5 AI/Model · q quit
Ask      Enter ask a question · ↑/↓ PgUp/PgDn scroll · Ctrl+Q 5-question quiz on topic
Command  / runs slash commands from any tab (shared with Telegram):
         /quiz [topic n] · /review [n] [exact] · /practice <t> [n]
         /anki · /flashcards <topic> [n] · /incubation · /strategy · /cards <topic> [n]
         /explain <topic> · /exam [n] · /exams · /today · /courses
         /plan [start date | day N | goto N | create | edit] · /ai · /model <name> · /effort <lvl>
         /mistakes · /course <name> · /lang <tr|en> · /cancel · /test · /help
Quiz     n new quiz · Enter saved questions · w retry mistakes
         a–e or 1–5 answer · on wrong c Cuotiben · Enter next · Esc finish
Review   t or Enter start due reviews
Exams    ↑/↓ select · Enter discuss with coach · d delete (y confirm)
         in chat Tab/Shift+Tab step mistakes · Ctrl+F Cuotiben · Enter send
Today    ←/→ change day · ↑/↓ task · Enter run · b start plan
         g open Plan tab · e edit plan manually
Plan     Enter / n discuss plan with AI · ←/→ draft day · Ctrl+B toggle start today
         Ctrl+S save draft · Ctrl+N reset · Esc exit
Cards    Enter FSRS study · n AI generate · t/←/→ tag filter · h High-Yield
         k Incubation · s Strategy Sheet · x export Anki .txt
         in review Space/Enter reveal · 1 Again · 2 Good · r Leech AI repair
         F3 toggle coach · F5 AI & model settings
Everywhere PgUp / PgDn scroll · Esc cancel AI · F1 / ? help";

/// Terminal kontrol karakterlerini ve ANSI CSI/OSC kaçış dizilerini temizler.
pub(crate) fn sanitize_tui_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(ch) = rest.chars().next() {
        if let Some(after_csi) = rest.strip_prefix("\x1b[") {
            let mut end = 0;
            for (idx, b) in after_csi.bytes().enumerate() {
                if (0x40..=0x7E).contains(&b) {
                    end = idx + 1;
                    break;
                }
                if !(0x20..=0x3F).contains(&b) {
                    end = idx;
                    break;
                }
                end = idx + 1;
            }
            rest = &after_csi[end..];
            continue;
        }
        if let Some(after_osc) = rest.strip_prefix("\x1b]") {
            let mut end = after_osc.len();
            let bytes = after_osc.as_bytes();
            let mut idx = 0;
            while idx < bytes.len() {
                if bytes[idx] == 0x07 {
                    end = idx + 1;
                    break;
                }
                if bytes[idx] == 0x1B && idx + 1 < bytes.len() && bytes[idx + 1] == b'\\' {
                    end = idx + 2;
                    break;
                }
                idx += 1;
            }
            rest = &after_osc[end..];
            continue;
        }
        if let Some(after_crlf) = rest.strip_prefix("\r\n") {
            out.push('\n');
            rest = after_crlf;
            continue;
        }
        rest = &rest[ch.len_utf8()..];
        match ch {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            '\u{0000}'..='\u{001F}' | '\u{007F}'..='\u{009F}' => {}
            _ => out.push(ch),
        }
    }
    out
}

fn panel(title: &str) -> Block<'static> {
    let title = sanitize_tui_text(title);
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
    let clean = sanitize_tui_text(text);
    inline_spans_clean(&clean, style)
}

fn inline_spans_clean(text: &str, style: Style) -> Vec<Span<'static>> {
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
                spans.extend(inline_spans_clean(inner, inner_style));
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
    let clean = sanitize_tui_text(line);
    markdown_line_clean(&clean)
}

fn markdown_line_clean(line: &str) -> Line<'static> {
    let trimmed = line.trim_start();
    if trimmed == "---" {
        return Line::default();
    }
    if let Some(rest) = trimmed.strip_prefix('#') {
        let heading = rest.trim_start_matches('#').trim();
        return Line::from(inline_spans_clean(
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
        spans.extend(inline_spans_clean(rest.trim_start(), Style::default()));
        return Line::from(spans);
    }
    Line::from(inline_spans_clean(line, Style::default()))
}

fn markdown(text: &str) -> Text<'static> {
    if text.is_empty() {
        return Text::default();
    }
    let clean = sanitize_tui_text(text);
    Text::from(clean.lines().map(markdown_line_clean).collect::<Vec<_>>())
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
    let ai_label = sanitize_tui_text(&app.ai_badge);
    let ai_chip = chip(&format!("AI {ai_label}"), TEXT, SURFACE);
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
        Span::raw(" "),
        ai_chip,
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
        PLAN_TAB => draw_plan_studio(frame, content, app),
        ANKI_TAB => draw_anki(frame, content, app),
        _ => unreachable!(),
    }
    let activity = app.spinner();
    let (line, color) = match &activity {
        Some(text) => (sanitize_tui_text(text), ACCENT),
        None => {
            let clean = sanitize_tui_text(&app.status);
            let color = if clean.starts_with('✓') { GOOD } else { WARN };
            (clean, color)
        }
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
    if let Some(modal) = &app.modal {
        draw_modal(frame, area, modal, app.lang);
    }
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
                "Henüz ders yok.\nresources/<ders>/ klasörü ekleyin, sonra r ile dizinleyin.",
                "No courses yet.\nAdd a resources/<course>/ folder, then press r to index.",
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
                    sanitize_tui_text(&subject.name),
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
    let (title_base, rendered) = if app.answer.is_empty() {
        if app.subjects.is_empty() {
            let body = match app.lang {
                Lang::Tr => ONBOARDING,
                Lang::En => ONBOARDING_EN,
            };
            (
                app.lang.pick("Başlarken", "Getting Started"),
                markdown(body),
            )
        } else {
            (app.lang.pick("Özet", "Summary"), dashboard(app))
        }
    } else {
        (app.lang.pick("Yanıt", "Answer"), markdown(&app.answer))
    };
    let inner_width = answer.width.saturating_sub(2);
    let inner_height = usize::from(answer.height.saturating_sub(2));
    let total = Paragraph::new(rendered.clone())
        .wrap(Wrap { trim: false })
        .line_count(inner_width);
    let max = u16::try_from(total.saturating_sub(inner_height)).unwrap_or(u16::MAX);
    app.ask_max_scroll.set(max);
    let offset = app.scroll.min(max);
    let panel_title = if !app.answer.is_empty() || max > 0 {
        format!("{title_base} · ↑/↓ PgUp/PgDn")
    } else {
        title_base.to_string()
    };
    panel_text(frame, answer, &panel_title, rendered, offset);
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
            Input::PlanChat => app.lang.pick("Plan tartışması", "Plan discussion"),
            Input::FlashcardTopic => app.lang.pick(
                "FSRS kart konusu [adet] (ör. Phrasal Verbs 10)",
                "FSRS card topic [count] (e.g. Conditionals 10)",
            ),
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
        Some(feedback) => {
            let cuotiben = if app.last_wrong_choice.is_some() {
                app.lang.pick(
                    "\n[c] Cuotiben hata defterine analiz et (FSRS / Sınav Tekniği)",
                    "\n[c] Analyze mistake with Cuotiben (FSRS / Strategy Sheet)",
                )
            } else {
                ""
            };
            format!(
                "{feedback}{cuotiben}\n\n{}",
                app.lang.pick(
                    "Enter: sonraki soru / sonucu göster",
                    "Enter: next question / show summary",
                )
            )
        }
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
                "Çalışma Planı · Henüz Plan Yok",
                "Study Plan · No Plan Yet",
            ),
            app.lang.pick(
                "Bu ders için henüz çalışma planı tanımlı değil.\n\n• g  Planla sekmesinde AI ile tartışarak ders programı oluştur (/plan olustur)\n• e  Elle gün ve görev ekleyerek plan oluştur (/plan duzenle)\n\nİsterseniz doğrudan Soru/Özet (1), Quiz (2), Tekrar (3) veya Deneme (6) sekmelerini de kullanabilirsiniz.",
                "No study plan is defined for this course yet.\n\n• g  Discuss and generate an AI schedule in the Plan tab (/plan create)\n• e  Create or edit days & tasks manually (/plan edit)\n\nYou can also use Ask (1), Quiz (2), Review (3), or Exams (6) directly.",
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
        Lang::Tr => {
            format!("Gün {view_day}/{total_days} · {tag} · {title_name} · g Planla · e düzenle")
        }
        Lang::En => {
            format!("Day {view_day}/{total_days} · {tag} · {title_name} · g Plan tab · e edit")
        }
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
                    ListItem::new(format!("{mark} {}", sanitize_tui_text(&task.label)))
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
    let title = if !app.input.is_empty() {
        app.lang.pick(
            "Soru · Enter gönder · Esc temizle",
            "Question · Enter send · Esc clear",
        )
    } else if chat.focus.is_some() {
        app.lang.pick(
            "Soru · Enter gönder · Ctrl+F Cuotiben · Esc çık",
            "Question · Enter send · Ctrl+F Cuotiben · Esc exit",
        )
    } else {
        app.lang.pick(
            "Soru · Enter gönder · Esc sohbetten çık",
            "Question · Enter send · Esc exit chat",
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

fn draw_plan_studio(frame: &mut Frame, content: Rect, app: &App) {
    let Some(chat) = &app.plan_chat else {
        let subject = app
            .subjects
            .get(app.selected)
            .map(|s| app.course.display_title(&s.name))
            .unwrap_or("");
        let existing = if app.course.plan.is_empty() {
            app.lang
                .pick("Henüz kayıtlı plan yok.", "No saved plan yet.")
                .to_string()
        } else {
            match app.lang {
                Lang::Tr => format!("Mevcut kayıtlı plan: {} gün.", app.course.plan.len()),
                Lang::En => format!("Current saved plan: {} day(s).", app.course.plan.len()),
            }
        };
        let body = match app.lang {
            Lang::Tr => format!(
                "Ders: {subject}\n{existing}\n\n\
                 Burada AI ile çalışma planınızı adım adım tartışabilirsiniz:\n\
                 • AI ders materyallerini ve zayıf konularınızı inceleyip önce netleştirici sorular sorar.\n\
                 • Verdiğiniz cevaplara göre gün gün taslak program oluşturur.\n\
                 • İstediğiniz değişiklikleri sohbet ederek revize edebilir, hazır olduğunda Ctrl+S ile kaydedebilirsiniz.\n\n\
                 • Enter / n  AI ile plan tartışmasını başlat\n\
                 • e          Planı elle düzenle"
            ),
            Lang::En => format!(
                "Course: {subject}\n{existing}\n\n\
                 Co-design your study schedule with AI here:\n\
                 • AI reviews your course materials and weak topics, then asks clarifying questions.\n\
                 • Builds a day-by-day draft schedule tailored to your answers.\n\
                 • Discuss revisions anytime and press Ctrl+S when ready to save.\n\n\
                 • Enter / n  Start AI planning discussion\n\
                 • e          Edit plan manually"
            ),
        };
        paragraph(
            frame,
            content,
            app.lang.pick("Çalışma Planı Stüdyosu", "Study Plan Studio"),
            &body,
            0,
        );
        return;
    };

    if let Some(days) = chat.draft.as_deref().filter(|d| !d.is_empty()) {
        if content.width >= 100 {
            let [left, right] =
                Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .areas(content);
            let [chat_area, input_area] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(left);
            draw_plan_chat(frame, chat_area, input_area, app, chat);
            draw_plan_draft(frame, right, app.lang, chat, days);
        } else {
            let draft_height = (content.height / 3).clamp(5, 11);
            let [chat_area, draft_area, input_area] = Layout::vertical([
                Constraint::Min(4),
                Constraint::Length(draft_height),
                Constraint::Length(3),
            ])
            .areas(content);
            draw_plan_chat(frame, chat_area, input_area, app, chat);
            draw_plan_draft(frame, draft_area, app.lang, chat, days);
        }
    } else {
        let [chat_area, input_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(content);
        draw_plan_chat(frame, chat_area, input_area, app, chat);
    }
}

fn draw_plan_chat(
    frame: &mut Frame,
    panel_area: Rect,
    input_area: Rect,
    app: &App,
    chat: &PlanChat,
) {
    let you = app.lang.pick("Sen", "You");
    let planner = app.lang.pick("Plan Danışmanı", "Planner");
    let mut text = String::new();
    for turn in &chat.turns {
        if !text.is_empty() {
            text.push('\n');
        }
        if turn.question.is_empty() {
            text.push_str(match app.lang {
                Lang::Tr => "Sen: yeni çalışma planı tartışmasını başlattın.\n",
                Lang::En => "You: started a new study plan discussion.\n",
            });
        } else {
            text.push_str(&format!("{you}: {}\n", turn.question));
        }
        match &turn.reply {
            CoachReply::Waiting => text.push_str(&format!("{planner}: …\n")),
            CoachReply::Answer(answer) => text.push_str(&format!("{planner}: {answer}\n")),
            CoachReply::Failed(message) => text.push_str(&format!("⚠ {message}\n")),
        }
    }
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
    let subject_title = app.course.display_title(&chat.subject);
    let title = format!("{planner} · {}", sanitize_tui_text(subject_title));
    panel_text(frame, panel_area, &title, rendered, offset);

    let input_title = match (app.input.is_empty(), chat.draft.is_some()) {
        (true, true) => app.lang.pick(
            "Mesaj / Revize · Enter gönder · Ctrl+S planı kaydet · Esc çık",
            "Message / Revise · Enter send · Ctrl+S save plan · Esc exit",
        ),
        (true, false) => app.lang.pick(
            "Cevap / İstek · Enter gönder · Esc çık",
            "Reply / Request · Enter send · Esc exit",
        ),
        (false, _) => app.lang.pick(
            "Mesaj · Enter gönder · Esc temizle",
            "Message · Enter send · Esc clear",
        ),
    };
    paragraph(
        frame,
        input_area,
        input_title,
        &input_tail(&app.input, input_area.width),
        0,
    );
}

fn draw_plan_draft(
    frame: &mut Frame,
    area: Rect,
    lang: Lang,
    chat: &PlanChat,
    days: &[crate::course::DayConfig],
) {
    let idx = chat.draft_day.min(days.len().saturating_sub(1));
    let day = &days[idx];
    let start_tag = if chat.auto_start {
        lang.pick("Bugünden başlat: Açık", "Start today: On")
    } else {
        lang.pick("Bugünden başlat: Kapalı", "Start today: Off")
    };
    let title = match lang {
        Lang::Tr => format!(
            "Taslak Plan · Gün {}/{} · ←/→ gün · Ctrl+S kaydet",
            idx + 1,
            days.len()
        ),
        Lang::En => format!(
            "Draft Plan · Day {}/{} · ←/→ day · Ctrl+S save",
            idx + 1,
            days.len()
        ),
    };
    let mut lines = vec![
        Line::from(Span::styled(
            sanitize_tui_text(&day.title),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];
    for (i, task) in day.tasks.iter().enumerate() {
        let cmd = sanitize_tui_text(&task.command);
        let label = sanitize_tui_text(&task.label);
        let row = if cmd.is_empty() {
            format!("{}. {label}", i + 1)
        } else {
            format!("{}. {label}  [{cmd}]", i + 1)
        };
        lines.push(Line::from(row));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!("{start_tag} (Ctrl+B)"),
        Style::default().fg(MUTED),
    )));
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(&title))
            .wrap(Wrap { trim: false }),
        area,
    );
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
            sanitize_tui_text(&session.subject),
            sanitize_tui_text(&session.topic),
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

fn draw_anki(frame: &mut Frame, content: Rect, app: &App) {
    if app.editing == Some(Input::FlashcardTopic) {
        let prompt = app.lang.pick(
            "FSRS kart konusu [adet] (ör. Phrasal Verbs 10)",
            "FSRS card topic [count] (e.g. Conditionals 10)",
        );
        let hint = app
            .lang
            .pick("Enter kart üret · Esc iptal", "Enter generate · Esc cancel");
        paragraph(
            frame,
            content,
            prompt,
            &format!("{}\n\n{hint}", app.input),
            0,
        );
        return;
    }
    if let Some(session) = &app.anki_session {
        draw_anki_review(frame, content, app, session);
        return;
    }
    match app.anki_view {
        AnkiView::Overview => draw_anki_overview(frame, content, app),
        AnkiView::Incubation => draw_anki_incubation(frame, content, app),
        AnkiView::Strategy => draw_anki_strategy(frame, content, app),
    }
}

fn draw_anki_overview(frame: &mut Frame, content: Rect, app: &App) {
    let subject = app
        .subjects
        .get(app.selected)
        .map(|s| s.name.as_str())
        .unwrap_or("—");
    let st = &app.anki_stats;
    let selected_tag = if app.anki_tag_idx == 0 {
        app.lang.pick("Tümü (Tek Havuz)", "All (Single Pool)")
    } else {
        app.anki_tags
            .get(app.anki_tag_idx - 1)
            .map_or("—", |t| t.tag.as_str())
    };
    let hy_badge = if app.anki_high_yield_only {
        app.lang.pick("AÇIK ★", "ON ★")
    } else {
        app.lang.pick("KAPALI", "OFF")
    };

    let banner_height = if st.backlog_paused { 3 } else { 0 };
    let [stats_area, banner_area, tags_area] = Layout::vertical([
        Constraint::Length(7),
        Constraint::Length(banner_height),
        Constraint::Min(4),
    ])
    .areas(content);

    let stats_lines = vec![
        Line::from(vec![
            Span::styled(
                format!(
                    "{}: {}   ",
                    app.lang.pick("Aktif Havuz", "Active Pool"),
                    st.total_active
                ),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "{}: {}   ",
                    app.lang.pick("Öğrenme (10m/30m)", "Learning (10m/30m)"),
                    st.due_learning
                ),
                Style::default().fg(ACCENT),
            ),
            Span::styled(
                format!(
                    "{}: {}   ",
                    app.lang.pick("Bekleyen Review", "Due Reviews"),
                    st.due_reviews
                ),
                Style::default().fg(if st.due_reviews > 0 { GOOD } else { MUTED }),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!(
                    "{}: {}/{} ({}: {}, {}: {})   ",
                    app.lang.pick("Yeni Kota", "New Quota"),
                    st.new_today,
                    crate::fsrs_engine::DAILY_NEW_CARD_LIMIT,
                    app.lang.pick("kalan", "left"),
                    st.new_remaining_today,
                    app.lang.pick("yeni havuz", "new pool"),
                    st.new_total
                ),
                Style::default().fg(TEXT),
            ),
            Span::styled(
                format!(
                    "{}: {:.1}d   ",
                    app.lang.pick("Ort. S", "Avg S"),
                    st.avg_stability
                ),
                Style::default().fg(MUTED),
            ),
            Span::styled(
                format!("Leech: {}   ", st.leech_count),
                Style::default().fg(if st.leech_count > 0 { WARN } else { MUTED }),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!(
                    "{}: {}   {}: {}   ",
                    app.lang.pick("Kuluçka", "Incubation"),
                    st.incubation_count,
                    app.lang.pick("Strateji Notu", "Strategy Notes"),
                    st.strategy_notes_count
                ),
                Style::default().fg(ACCENT),
            ),
            Span::styled(
                format!(
                    "R*={:.0}%",
                    crate::fsrs_engine::DEFAULT_RETENTION * 100.0
                ),
                Style::default().fg(MUTED),
            ),
        ]),
        Line::from(vec![Span::styled(
            format!(
                "{}: {selected_tag}   |   High-Yield [h]: {hy_badge}",
                app.lang.pick("Etiket Filtresi", "Tag Filter")
            ),
            Style::default().fg(TEXT),
        )]),
        Line::from(Span::styled(
            app.lang.pick(
                "Enter: çalış · n: AI kart üret · k: Kuluçka · s: Strateji · x: Anki .txt aktar",
                "Enter: study · n: generate AI cards · k: Incubation · s: Strategy · x: export Anki .txt",
            ),
            Style::default().fg(GOOD),
        )),
    ];
    let title = format!("{} · {subject}", app.lang.pick("FSRS Kart Havuzu", "FSRS Card Pool"));
    panel_text(frame, stats_area, &title, Text::from(stats_lines), 0);

    if st.backlog_paused {
        let warn_text = match app.lang {
            Lang::Tr => format!(
                "⚠ YIĞILMA KORUMASI AKTİF: {} vadesi gelmiş tekrar var (≥ {}). Yeni kart açılımı otomatik durduruldu.",
                st.due_reviews,
                crate::fsrs_engine::BACKLOG_PROTECTION_THRESHOLD
            ),
            Lang::En => format!(
                "⚠ BACKLOG PROTECTION ACTIVE: {} due reviews (≥ {}). Daily new cards automatically paused.",
                st.due_reviews,
                crate::fsrs_engine::BACKLOG_PROTECTION_THRESHOLD
            ),
        };
        panel_text(
            frame,
            banner_area,
            app.lang.pick("Koruma", "Protection"),
            Text::from(Line::styled(
                warn_text,
                Style::default().fg(WARN).add_modifier(Modifier::BOLD),
            )),
            0,
        );
    }

    if app.anki_tags.is_empty() {
        paragraph(
            frame,
            tags_area,
            app.lang
                .pick("Hiyerarşik Etiket Ağacı", "Hierarchical Tag Tree"),
            app.lang.pick(
                "Henüz FSRS kartı yok. n tuşuna basıp konu yazarak AI ile atomik FSRS kartları üretin veya quiz yanlışlarında [c] ile Cuotiben kullanın.",
                "No FSRS flashcards yet. Press n to generate atomic FSRS cards on any topic, or press [c] on a quiz mistake for Cuotiben.",
            ),
            app.scroll,
        );
    } else {
        let items: Vec<ListItem> = std::iter::once(ListItem::new(Line::styled(
            app.lang
                .pick("*  Tüm Etiketler (Tek Havuz)", "*  All Tags (Single Pool)"),
            Style::default().fg(TEXT),
        )))
        .chain(app.anki_tags.iter().map(|t| {
            let leech_part = if t.leech_count > 0 {
                format!(" · ⚠ {} leech", t.leech_count)
            } else {
                String::new()
            };
            ListItem::new(Line::styled(
                format!(
                    "{}   ({} {} · {} due · {} {}{})",
                    t.tag,
                    t.total,
                    app.lang.pick("kart", "cards"),
                    t.due,
                    t.new_count,
                    app.lang.pick("yeni", "new"),
                    leech_part
                ),
                Style::default().fg(if t.due > 0 { ACCENT } else { MUTED }),
            ))
        }))
        .collect();
        let mut list_state = ListState::default().with_selected(Some(app.anki_tag_idx));
        frame.render_stateful_widget(
            List::new(items)
                .block(panel(app.lang.pick(
                    "Hiyerarşik Etiket Ağacı (t / ←→ seç)",
                    "Hierarchical Tag Tree (t / ←→ select)",
                )))
                .highlight_style(
                    Style::default()
                        .fg(BASE)
                        .bg(ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
            tags_area,
            &mut list_state,
        );
    }
}

fn draw_anki_review(frame: &mut Frame, content: Rect, app: &App, session: &AnkiSession) {
    let Some(card) = session.current() else {
        return;
    };
    let now = Utc::now();
    let elapsed = session.elapsed_secs();
    let limit = card.card_type.speed_focus_limit_secs();
    let speed_over = session.speed_focus_exceeded();
    let retrievability = fsrs_engine::retrievability(card, now, None);

    let [header_area, card_area, action_area] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(6),
        Constraint::Length(4),
    ])
    .areas(content);

    let mut line1_spans = vec![
        Span::styled(
            format!(
                " {}/{}  {}  ",
                session.position + 1,
                session.cards.len(),
                card.tag
            ),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "[{} · {}]  ",
                card.state.label(app.lang),
                card.card_type.as_str()
            ),
            Style::default().fg(ACCENT),
        ),
    ];
    if card.is_high_yield {
        line1_spans.push(Span::styled("★  ", Style::default().fg(GOOD)));
    }
    if card.is_leech {
        line1_spans.push(Span::styled(
            format!("⚠ LEECH ({}x)  ", card.lapses),
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        ));
    }
    let timer_label = if speed_over {
        match app.lang {
            Lang::Tr => format!(" ⏰ {elapsed}s/{limit}s HIZ ODAĞI!"),
            Lang::En => format!(" ⏰ {elapsed}s/{limit}s SPEED FOCUS!"),
        }
    } else {
        format!(" ⏱ {elapsed}s/{limit}s")
    };
    let line2_spans = vec![
        Span::styled(
            timer_label,
            Style::default()
                .fg(if speed_over { WARN } else { GOOD })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "   |   R={:.0}%  S={:.1}d  D={:.1}",
                retrievability * 100.0,
                card.stability,
                card.difficulty
            ),
            Style::default().fg(MUTED),
        ),
    ];
    let filter_label = match (session.tag_filter.as_deref(), session.high_yield_only) {
        (Some(tag), true) => format!(" · {tag} · ★"),
        (Some(tag), false) => format!(" · {tag}"),
        (None, true) => " · ★ High-Yield".into(),
        (None, false) => String::new(),
    };
    panel_text(
        frame,
        header_area,
        &format!("FSRS · {}{}", session.subject, filter_label),
        Text::from(vec![Line::from(line1_spans), Line::from(line2_spans)]),
        0,
    );

    let rendered_front = if card.card_type == fsrs_engine::CardKind::Cloze {
        fsrs_engine::format_cloze_front(&card.front)
    } else {
        card.front.clone()
    };
    let mut body = format!("**{}**\n\n{}", card.context_prefix, rendered_front);
    if session.revealed {
        let clean_back = if card.card_type == fsrs_engine::CardKind::Cloze {
            fsrs_engine::format_cloze_back(&card.back)
        } else {
            card.back.clone()
        };
        body.push_str(&format!("\n\n━━━━━━━━━━━━━━━━━━━━━━━━\n\n{clean_back}"));
        if !card.extra_context.trim().is_empty() {
            body.push_str(&format!("\n\n> {}", card.extra_context.trim()));
        }
        if let Some(hint) = &card.mnemonic_hint
            && !hint.trim().is_empty()
        {
            body.push_str(&format!(
                "\n\n💡 **{}:** {}",
                app.lang.pick("İpucu", "Mnemonic"),
                hint.trim()
            ));
        }
    } else {
        body.push_str(app.lang.pick(
            "\n\n---\n*Hatırlamaya çalış ve Space / Enter ile cevabı aç.*",
            "\n\n---\n*Recall first, then press Space / Enter to reveal.*",
        ));
    }
    panel_text(
        frame,
        card_area,
        app.lang.pick("Kart", "Card"),
        markdown(&body),
        app.scroll,
    );

    let action_lines = if session.revealed {
        let (again_label, good_label) = fsrs_engine::preview_ratings(card, now, app.lang);
        let mut spans = vec![
            Span::styled(
                format!(
                    " [1] {} ({again_label}) ",
                    app.lang.pick("Hatırlamadım", "Again")
                ),
                Style::default()
                    .fg(BASE)
                    .bg(WARN)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!(
                    " [2] {} ({good_label}) ",
                    app.lang.pick("Hatırladım", "Good")
                ),
                Style::default()
                    .fg(BASE)
                    .bg(GOOD)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        if card.is_leech {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                app.lang.pick(" [r] AI Onar ", " [r] AI Repair "),
                Style::default()
                    .fg(BASE)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        vec![Line::from(spans)]
    } else {
        vec![Line::from(Span::styled(
            app.lang.pick(
                " [Space / Enter] Cevabı Göster     [Esc] Oturumdan Çık",
                " [Space / Enter] Reveal Answer     [Esc] Exit Session",
            ),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))]
    };
    panel_text(
        frame,
        action_area,
        app.lang
            .pick("2-Buton Değerlendirme", "2-Button Rating"),
        Text::from(action_lines),
        0,
    );
}

fn draw_anki_incubation(frame: &mut Frame, content: Rect, app: &App) {
    if app.anki_incubation.is_empty() {
        paragraph(
            frame,
            content,
            app.lang.pick("Kuluçka Havuzu (Cuotiben)", "Incubation Queue (Cuotiben)"),
            app.lang.pick(
                "Onay bekleyen kuluçka kartı yok.\nQuiz veya Deneme yanlışlarında [c] / Ctrl+F ile Cuotiben analizi çalıştırabilirsiniz.\n\nEsc: geri dön",
                "No pending incubation cards.\nPress [c] on a quiz mistake or Ctrl+F in Exam Coach to run Cuotiben analysis.\n\nEsc: go back",
            ),
            0,
        );
        return;
    }
    let idx = app.anki_inc_idx.min(app.anki_incubation.len() - 1);
    let card = &app.anki_incubation[idx];
    let mut text = format!(
        "{} {}/{} · {} {}\n\n**{}** {}\n\n━━━━━━━━━━━━━━━━━━━━━━━━\n\n{}",
        app.lang.pick("Kart", "Card"),
        idx + 1,
        app.anki_incubation.len(),
        card.tag,
        if card.is_high_yield { "★ High-Yield" } else { "" },
        card.context_prefix,
        card.front,
        card.back
    );
    if !card.extra_context.trim().is_empty() {
        text.push_str(&format!("\n\n> {}", card.extra_context.trim()));
    }
    if let Some(hint) = &card.mnemonic_hint
        && !hint.trim().is_empty()
    {
        text.push_str(&format!(
            "\n\n💡 **{}:** {}",
            app.lang.pick("İpucu", "Mnemonic"),
            hint.trim()
        ));
    }
    text.push_str(app.lang.pick(
        "\n\n[a / Enter] Onayla ve FSRS Havuzuna Ekle   ·   [x] Sil   ·   [←/→] Gez   ·   [Esc] Geri",
        "\n\n[a / Enter] Approve to FSRS Pool   ·   [x] Delete   ·   [←/→] Browse   ·   [Esc] Back",
    ));
    panel_text(
        frame,
        content,
        app.lang.pick("Kuluçka Havuzu (Cuotiben)", "Incubation Queue (Cuotiben)"),
        markdown(&text),
        app.scroll,
    );
}

fn draw_anki_strategy(frame: &mut Frame, content: Rect, app: &App) {
    if app.anki_notes.is_empty() {
        paragraph(
            frame,
            content,
            app.lang
                .pick("Sınav Tekniği Defteri (Cuotiben)", "Strategy Sheet (Cuotiben)"),
            app.lang.pick(
                "Henüz sınav tekniği / dikkat hatası notu yok.\nBilgi eksikliği dışındaki hatalar FSRS kart kuyruğunu kirletmeden buraya kaydedilir.\n\nEsc: geri dön",
                "No exam strategy / careless mistake notes yet.\nNon-knowledge errors are logged here without cluttering your FSRS queue.\n\nEsc: go back",
            ),
            0,
        );
        return;
    }
    let body = app
        .anki_notes
        .iter()
        .enumerate()
        .map(|(i, n)| format!("### {}. {}\n{}\n*{}*", i + 1, n.tag, n.note, n.created_at))
        .collect::<Vec<_>>()
        .join("\n\n");
    panel_text(
        frame,
        content,
        app.lang
            .pick("Sınav Tekniği Defteri (Esc: geri)", "Strategy Sheet (Esc: back)"),
        markdown(&body),
        app.scroll,
    );
}

/// Aktif bağlama göre alt satırdaki tuş ipuçları.
fn context_keys(app: &App) -> Vec<(&'static str, &'static str)> {
    let lang = app.lang;
    if let Some(modal) = &app.modal {
        return match modal {
            Modal::PlanEdit(m) => {
                if m.prompt.is_some() {
                    vec![
                        ("Tab/↑↓", lang.pick("alan", "field")),
                        ("←/→", lang.pick("tür", "kind")),
                        ("Enter", lang.pick("uygula", "apply")),
                        ("Esc", lang.pick("geri", "back")),
                    ]
                } else {
                    vec![
                        ("←/→", lang.pick("gün", "day")),
                        ("+/t/d", lang.pick("gün ekle/ad/sil", "add/ren/del day")),
                        (
                            "a/Enter/x",
                            lang.pick("görev ekle/düz/sil", "task add/ed/del"),
                        ),
                        ("s", lang.pick("kaydet", "save")),
                        ("Esc", lang.pick("kapat", "close")),
                    ]
                }
            }
            Modal::AiSettings(_) => vec![
                ("Tab/↑↓", lang.pick("alan", "field")),
                (
                    "←/→",
                    lang.pick("sağlayıcı/model/efor", "provider/model/effort"),
                ),
                ("Ctrl+T", lang.pick("test", "test")),
                ("Ctrl+S", lang.pick("kaydet", "save")),
                ("Esc", lang.pick("kapat", "close")),
            ],
        };
    }
    if let Some(input) = app.editing {
        return match input {
            Input::Answer => vec![
                ("Enter", lang.pick("gönder", "send")),
                ("↑/↓ PgUp/PgDn", lang.pick("kaydır", "scroll")),
                ("Esc", lang.pick("vazgeç", "cancel")),
                ("Ctrl+U", lang.pick("temizle", "clear")),
            ],
            Input::Topic | Input::Count => vec![
                ("Enter", lang.pick("devam", "next")),
                ("Esc", lang.pick("vazgeç", "cancel")),
                ("Backspace", lang.pick("sil", "del")),
            ],
            Input::DenemeChat => {
                let mut keys = vec![
                    ("Enter", lang.pick("gönder", "send")),
                    ("Tab", lang.pick("sonraki yanlış", "next mistake")),
                    ("Shift+Tab", lang.pick("önceki", "prev")),
                    ("↑/↓", lang.pick("kaydır", "scroll")),
                    ("PgUp/PgDn", lang.pick("hızlı kaydır", "fast scroll")),
                    ("Esc", lang.pick("çık", "exit")),
                ];
                if app.deneme_chat.as_ref().is_some_and(|c| c.focus.is_some()) {
                    keys.insert(1, ("Ctrl+F", "Cuotiben"));
                }
                keys
            }
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
            Input::PlanChat => {
                let mut keys = vec![
                    ("Enter", lang.pick("gönder", "send")),
                    ("↑/↓ PgUp/PgDn", lang.pick("kaydır", "scroll")),
                    ("Ctrl+N", lang.pick("sıfırla", "reset")),
                    ("Esc", lang.pick("çık", "exit")),
                ];
                if app.plan_chat.as_ref().is_some_and(|c| c.draft.is_some()) {
                    keys.insert(1, ("←/→", lang.pick("taslak gün", "draft day")));
                    keys.insert(2, ("Ctrl+B", lang.pick("bugün/kaydet", "start/save")));
                    keys.insert(3, ("Ctrl+S", lang.pick("planı kaydet", "save plan")));
                }
                keys
            }
            Input::FlashcardTopic => vec![
                ("Enter", lang.pick("kart üret", "generate cards")),
                ("Esc", lang.pick("vazgeç", "cancel")),
                ("Backspace", lang.pick("sil", "del")),
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
            let mut keys = vec![
                ("Enter", lang.pick("sonraki soru", "next question")),
                ("Esc", lang.pick("quizi bitir", "finish quiz")),
            ];
            if app.last_wrong_choice.is_some() {
                keys.insert(1, ("c", "Cuotiben"));
            }
            keys
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
            ("↑/↓ PgUp/PgDn", lang.pick("kaydır", "scroll")),
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
        PLAN_TAB => vec![
            (
                "Enter / n",
                lang.pick("planlamayı başlat", "start planning"),
            ),
            ("e", lang.pick("elle düzenle", "edit manually")),
        ],
        ANKI_TAB => {
            if let Some(session) = &app.anki_session {
                if session.revealed {
                    let mut keys = vec![
                        ("1", lang.pick("Hatırlamadım", "Again")),
                        ("2", lang.pick("Hatırladım", "Good")),
                        ("Esc", lang.pick("çık", "exit")),
                    ];
                    if session.current().is_some_and(|c| c.is_leech) {
                        keys.insert(2, ("r", lang.pick("AI onar", "AI repair")));
                    }
                    keys
                } else {
                    vec![
                        ("Space / Enter", lang.pick("cevabı aç", "reveal")),
                        ("Esc", lang.pick("çık", "exit")),
                    ]
                }
            } else {
                match app.anki_view {
                    AnkiView::Overview => vec![
                        ("Enter", lang.pick("çalış", "study")),
                        ("n", lang.pick("kart üret", "generate")),
                        ("t / ←→", lang.pick("etiket", "tag")),
                        ("h", lang.pick("yüksek getiri", "high-yield")),
                        ("k", lang.pick("kuluçka", "incubation")),
                        ("s", lang.pick("strateji", "strategy")),
                        ("x", lang.pick("dışa aktar", "export")),
                    ],
                    AnkiView::Incubation => vec![
                        ("a / Enter", lang.pick("onayla", "approve")),
                        ("x", lang.pick("sil", "delete")),
                        ("←/→", lang.pick("gez", "browse")),
                        ("Esc", lang.pick("geri", "back")),
                    ],
                    AnkiView::Strategy => vec![
                        ("PgUp/PgDn", lang.pick("kaydır", "scroll")),
                        ("Esc / b", lang.pick("geri", "back")),
                    ],
                }
            }
        }
        _ => vec![
            ("←/→", lang.pick("gün", "day")),
            ("↑/↓", lang.pick("görev", "task")),
            ("Enter", lang.pick("çalıştır", "run")),
            ("b", lang.pick("başlat", "start")),
            ("g", lang.pick("Planla", "Plan tab")),
            ("e", lang.pick("düzenle", "edit")),
        ],
    }
}

fn global_keys(app: &App) -> Vec<(&'static str, &'static str)> {
    let lang = app.lang;
    vec![
        ("Tab / 1–8", lang.pick("sekme", "tabs")),
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
        ("F5", lang.pick("AI/Model", "AI/Model")),
        ("F1 / ?", lang.pick("yardım", "help")),
        ("q", lang.pick("çık", "quit")),
    ]
}

fn draw_help(frame: &mut Frame, area: Rect, lang: Lang) {
    let popup = centered(area, 76, 21);
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

fn draw_modal(frame: &mut Frame, area: Rect, modal: &Modal, lang: Lang) {
    match modal {
        Modal::PlanEdit(m) => draw_plan_editor(frame, area, m, lang),
        Modal::AiSettings(m) => draw_ai_settings(frame, area, m, lang),
    }
}

fn field_row(label: &str, value: &str, focused: bool, hint: Option<&str>) -> Line<'static> {
    let marker = if focused { "▸ " } else { "  " };
    let label_style = if focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(MUTED)
    };
    let val_style = if focused {
        Style::default()
            .fg(TEXT)
            .bg(SURFACE)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT)
    };
    let mut spans = vec![
        Span::styled(format!("{marker}{label:<18} "), label_style),
        Span::styled(format!(" {} ", sanitize_tui_text(value)), val_style),
    ];
    if let Some(hint) = hint {
        spans.push(Span::styled(
            format!("  {}", sanitize_tui_text(hint)),
            Style::default().fg(MUTED),
        ));
    }
    Line::from(spans)
}

fn draw_plan_editor(frame: &mut Frame, area: Rect, m: &PlanEditModal, lang: Lang) {
    let popup = centered(area, 82, 23);
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title(lang.pick(
            " Çalışma Planı Düzenleyici · s kaydet · Esc kapat ",
            " Study Plan Editor · s save · Esc close ",
        ))
        .style(Style::default().fg(TEXT).bg(BASE))
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [head_area, body_area, foot_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(if m.prompt.is_some() { 7 } else { 2 }),
    ])
    .areas(inner);

    let total_days = m.days.len();
    let cur_day = m.days.get(m.day_idx);
    let day_num = cur_day.map_or(1, |d| d.day);
    let day_title = cur_day.map_or("", |d| d.title.as_str());
    let head_lines = vec![
        Line::from(vec![
            Span::styled(
                format!(" ◀ Gün/Day {day_num}/{total_days}: "),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                sanitize_tui_text(day_title),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ▶", Style::default().fg(ACCENT)),
        ]),
        Line::styled(
            lang.pick(
                " ←/→ gün · + yeni gün · t başlık · d gün sil · a görev ekle · Enter düzenle · x sil · s kaydet",
                " ←/→ day · + add day · t rename · d del day · a add task · Enter edit · x del · s save",
            ),
            Style::default().fg(MUTED),
        ),
    ];
    frame.render_widget(
        Paragraph::new(head_lines).wrap(Wrap { trim: false }),
        head_area,
    );

    let tasks = cur_day.map(|d| d.tasks.as_slice()).unwrap_or(&[]);
    let task_items: Vec<ListItem> = if tasks.is_empty() {
        vec![ListItem::new(lang.pick(
            "  (Bu günde görev yok — eklemek için 'a' tuşuna basın)",
            "  (No tasks on this day — press 'a' to add one)",
        ))]
    } else {
        tasks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let cmd = if t.command.is_empty() {
                    lang.pick("[okuma]", "[manual]")
                } else {
                    t.command.as_str()
                };
                let line = format!(
                    "  {}. {}  ({})",
                    i + 1,
                    sanitize_tui_text(&t.label),
                    sanitize_tui_text(cmd)
                );
                ListItem::new(line)
            })
            .collect()
    };
    let mut state = ListState::default();
    if !tasks.is_empty() {
        state.select(Some(m.task_idx.min(tasks.len() - 1)));
    }
    frame.render_stateful_widget(
        List::new(task_items)
            .highlight_symbol("▸ ")
            .highlight_style(
                Style::default()
                    .fg(BASE)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
        body_area,
        &mut state,
    );

    if let Some(prompt) = &m.prompt {
        let mut p_lines = Vec::new();
        match prompt {
            PlanEditPrompt::DayTitle { is_new, text } => {
                p_lines.push(Line::styled(
                    if *is_new {
                        lang.pick(
                            "Yeni Gün Başlığı (Enter ekle, Esc iptal):",
                            "New Day Title (Enter add, Esc cancel):",
                        )
                    } else {
                        lang.pick(
                            "Gün Başlığını Düzenle (Enter uygula, Esc iptal):",
                            "Rename Day (Enter apply, Esc cancel):",
                        )
                    },
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ));
                p_lines.push(field_row(lang.pick("Başlık", "Title"), text, true, None));
            }
            PlanEditPrompt::TaskForm {
                is_new,
                field,
                cmd_kind,
                arg,
                label,
            } => {
                let kind_label = match (*cmd_kind, lang) {
                    (0, Lang::Tr) => "◀ Elle okuma / çalışma ▶",
                    (0, Lang::En) => "◀ Manual reading / study ▶",
                    (i, _) => TASK_CMD_KINDS.get(i).copied().unwrap_or(""),
                };
                p_lines.push(Line::styled(
                    if *is_new {
                        lang.pick(
                            "Yeni Görev (Enter ekle, Esc iptal):",
                            "New Task (Enter add, Esc cancel):",
                        )
                    } else {
                        lang.pick(
                            "Görevi Düzenle (Enter uygula, Esc iptal):",
                            "Edit Task (Enter apply, Esc cancel):",
                        )
                    },
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ));
                p_lines.push(field_row(
                    lang.pick("Komut türü", "Command kind"),
                    kind_label,
                    *field == 0,
                    Some("←/→"),
                ));
                p_lines.push(field_row(
                    lang.pick("Konu / argüman", "Topic / argument"),
                    arg,
                    *field == 1,
                    None,
                ));
                p_lines.push(field_row(
                    lang.pick("Etiket (isteğe b.)", "Label (optional)"),
                    label,
                    *field == 2,
                    None,
                ));
            }
        }
        if let Some(err) = &m.error {
            p_lines.push(Line::styled(
                format!("! {}", sanitize_tui_text(err)),
                Style::default().fg(WARN),
            ));
        }
        frame.render_widget(
            Paragraph::new(p_lines)
                .block(Block::bordered().border_style(Style::default().fg(MUTED)))
                .wrap(Wrap { trim: false }),
            foot_area,
        );
    } else if let Some(err) = &m.error {
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("! {}", sanitize_tui_text(err)),
                Style::default().fg(WARN),
            )),
            foot_area,
        );
    }
}

fn mask_secret_display(
    dirty: bool,
    input: &str,
    existing_masked: Option<&str>,
    lang: Lang,
) -> String {
    if dirty {
        if input.is_empty() {
            lang.pick("(silinecek)", "(will clear)").into()
        } else {
            let bullets = "•".repeat(input.chars().count().min(12));
            format!("{bullets} ({})", lang.pick("yeni", "new"))
        }
    } else if let Some(mask) = existing_masked {
        mask.to_string()
    } else {
        lang.pick("(ayarlı değil)", "(not set)").into()
    }
}

fn draw_ai_settings(frame: &mut Frame, area: Rect, m: &AiSettingsModal, lang: Lang) {
    let popup = centered(area, 84, 25);
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title(lang.pick(
            " AI Sağlayıcı ve Model Ayarları (.env) · Esc kapat ",
            " AI Provider & Model Settings (.env) · Esc close ",
        ))
        .style(Style::default().fg(TEXT).bg(BASE))
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let selected_provider = m.provider();
    let provider_val = format!("◀ {} ▶", selected_provider.display_name(lang));
    let model_val = if m.model.is_empty() {
        lang.pick("(varsayılan)", "(default)")
    } else {
        m.model.as_str()
    };
    let effort_val = match m.effort() {
        Some(eff) => format!("◀ {} ▶", eff.display_name(lang)),
        None => format!("◀ {} ▶", lang.pick("(varsayılan)", "(default)")),
    };
    let base_val = if m.base_url.is_empty() {
        lang.pick("(varsayılan)", "(default)")
    } else {
        m.base_url.as_str()
    };
    let custom_val = if m.custom_cmd.is_empty() {
        lang.pick("(yok)", "(none)")
    } else {
        m.custom_cmd.as_str()
    };
    let gemini_disp = mask_secret_display(
        m.gemini_key_dirty,
        &m.gemini_key_input,
        m.gemini_key_masked.as_deref(),
        lang,
    );
    let openai_disp = mask_secret_display(
        m.openai_key_dirty,
        &m.openai_key_input,
        m.openai_key_masked.as_deref(),
        lang,
    );

    let mut status_spans = Vec::new();
    for st in &m.statuses {
        let mark = if st.ready { "✓" } else { "·" };
        let color = if st.provider == selected_provider {
            ACCENT
        } else if st.ready {
            GOOD
        } else {
            MUTED
        };
        status_spans.push(Span::styled(
            format!("{mark}{} ", st.provider.as_env_str()),
            Style::default().fg(color),
        ));
    }

    let test_style = if m.field == 8 {
        Style::default()
            .fg(BASE)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(ACCENT).bg(SURFACE)
    };
    let save_style = if m.field == 9 {
        Style::default()
            .fg(BASE)
            .bg(GOOD)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(GOOD).bg(SURFACE)
    };

    let mut lines = vec![
        Line::from(status_spans),
        Line::raw(""),
        field_row(
            lang.pick("Sağlayıcı", "Provider"),
            &provider_val,
            m.field == 0,
            Some("←/→"),
        ),
        field_row(
            lang.pick("Model", "Model"),
            model_val,
            m.field == 1,
            Some(lang.pick("←/→ hazır · yaz: özel", "←/→ preset · type custom")),
        ),
        field_row(
            lang.pick("Düşünme Eforu", "Effort"),
            &effort_val,
            m.field == 2,
            Some("←/→"),
        ),
        field_row(
            lang.pick("Base URL", "Base URL"),
            base_val,
            m.field == 3,
            Some("OpenAI / Gemini URL"),
        ),
        field_row(
            "GEMINI_API_KEY",
            &gemini_disp,
            m.field == 4,
            Some(lang.pick("yaz: değiştir · Del/Bksp: sil", "type: set · Bksp: clear")),
        ),
        field_row(
            "OPENAI_API_KEY",
            &openai_disp,
            m.field == 5,
            Some(lang.pick("yaz: değiştir · Del/Bksp: sil", "type: set · Bksp: clear")),
        ),
        field_row(
            "STUDIA_AI_CMD",
            custom_val,
            m.field == 6,
            Some("{prompt} {schema}"),
        ),
        field_row(
            lang.pick("Zaman aşımı (sn)", "Timeout (sec)"),
            &m.timeout_secs,
            m.field == 7,
            Some("0 = limitsiz"),
        ),
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                lang.pick(
                    " Bağlantıyı Test Et (Ctrl+T) ",
                    " Test Connection (Ctrl+T) ",
                ),
                test_style,
            ),
            Span::raw("   "),
            Span::styled(
                lang.pick(" .env'ye Kaydet (Ctrl+S) ", " Save to .env (Ctrl+S) "),
                save_style,
            ),
        ]),
    ];

    if let Some((ok, msg)) = &m.test_result {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            sanitize_tui_text(msg),
            Style::default().fg(if *ok { GOOD } else { WARN }),
        ));
    }
    if let Some(err) = &m.error {
        lines.push(Line::styled(
            format!("! {}", sanitize_tui_text(err)),
            Style::default().fg(WARN),
        ));
    }
    if !m.env_overrides.is_empty() {
        lines.push(Line::styled(
            format!(
                "{} {}",
                lang.pick(
                    "Not — ortam değişkeni aktif:",
                    "Note — env overrides active:"
                ),
                m.env_overrides.join(", ")
            ),
            Style::default().fg(WARN),
        ));
    }
    if selected_provider == AiProvider::Auto {
        lines.push(Line::styled(
            lang.pick(
                "Auto: önce agy CLI, yoksa GEMINI_API_KEY / OPENAI_API_KEY / diğer CLI'lar denenir.",
                "Auto: tries agy CLI first, then GEMINI_API_KEY / OPENAI_API_KEY / other CLIs.",
            ),
            Style::default().fg(MUTED),
        ));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
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
        std::fs::create_dir_all(root.path().join("resources/Ders")).unwrap();
        std::fs::write(root.path().join("resources/Ders/ders.md"), "Kaynak.").unwrap();
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

    #[test]
    fn spec_tui_control_char_and_escape_sanitization() {
        // CSI renk ve imleç kaçışları temizlenir.
        assert_eq!(
            sanitize_tui_text("\x1b[31mKırmızı\x1b[0m \x1b[2J\x1b[10;20H\x1b[?25hMetin"),
            "Kırmızı Metin"
        );

        // OSC başlık (BEL ile) ve köprü (ST ile) kaçışları temizlenir.
        assert_eq!(
            sanitize_tui_text(
                "\x1b]0;Sahte Başlık\x07Normal \x1b]8;;https://example.com\x1b\\Bağlantı\x1b]8;;\x1b\\"
            ),
            "Normal Bağlantı"
        );

        // BEL, NUL, BS, C1 (\u{009b}) ve \r üzerine yazma denemeleri temizlenir; \r\n -> \n, \t -> 4 boşluk.
        assert_eq!(
            sanitize_tui_text("ilk\r\nüzerine\ryaz\t\x07\x00\x08\u{007f}\u{009b}31mson"),
            "ilk\nüzerineyaz    31mson"
        );

        // Türkçe UTF-8 karakterler korunur.
        let turkish = "ğüşiöçİĞÜŞÖÇ";
        assert_eq!(sanitize_tui_text(turkish), turkish);

        // Markdown sözdizimi korunur ve kaçış dizileri sızmaz.
        let md = markdown("### \x1b[31m**Başlık**\x1b[0m\n- *Öğe\x07* `kod\r`\tğüşiöçİĞÜŞÖÇ");
        let rendered_lines: Vec<String> = md
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(rendered_lines[0], "Başlık");
        assert_eq!(rendered_lines[1], "• Öğe kod    ğüşiöçİĞÜŞÖÇ");

        // Tam TUI render yolunda durum çubuğu, yanıt ve ders adları temizlenir.
        let (_root, mut app) = app_with_subject();
        app.status = "\x1b[32m✓ Hazır\x07\r\x1b]0;pwn\x07".into();
        app.answer = "Yanıt\x1b[2J **kalın**\u{009b} ğüşiöç".into();
        let screen = render(&app, 100, 30);
        assert!(screen.contains("✓ Hazır"));
        assert!(screen.contains("Yanıt kalın ğüşiöç"));
        assert!(!screen.contains('\x1b'));
        assert!(!screen.contains('\x07'));
        assert!(!screen.contains('\u{009b}'));
    }

    #[test]
    fn plan_studio_tab_renders_idle_chat_and_draft_preview() {
        let (_root, mut app) = app_with_subject();
        app.key(press(KeyCode::Char('7'))).unwrap();
        assert_eq!(app.tab, PLAN_TAB);
        let idle = render(&app, 100, 30);
        assert!(idle.contains("Çalışma Planı Stüdyosu"), "{idle}");

        app.plan_chat = Some(PlanChat {
            subject: "Ders".into(),
            turns: vec![super::super::app::DenemeTurn {
                question: String::new(),
                reply: CoachReply::Answer("Hedef tarihiniz nedir?".into()),
            }],
            draft: Some(vec![crate::course::DayConfig {
                day: 1,
                title: "Temel Kavramlar".into(),
                tasks: vec![crate::course::TaskConfig {
                    command: "/anlat Temel".into(),
                    label: "Konu anlatımı".into(),
                }],
            }]),
            draft_day: 0,
            auto_start: true,
            follow: true,
            scroll: 0,
            max_scroll: std::cell::Cell::new(0),
        });
        app.editing = Some(Input::PlanChat);

        for (w, h) in [(140, 34), (100, 30), (60, 16)] {
            let screen = render(&app, w, h);
            assert!(
                screen.contains("Hedef tarihiniz nedir?"),
                "{w}x{h}:\n{screen}"
            );
            assert!(screen.contains("Taslak Plan"), "{w}x{h}:\n{screen}");
            assert!(screen.contains("Temel Kavramlar"), "{w}x{h}:\n{screen}");
        }
    }

    #[test]
    fn all_new_modals_render_cleanly_and_mask_api_secrets() {
        let (_root, mut app) = app_with_subject();

        // 1. PlanEditModal (list + inline TaskForm) from Today tab.
        app.key(press(KeyCode::Char('5'))).unwrap();
        app.key(press(KeyCode::Char('e'))).unwrap();
        let editor_screen = render(&app, 100, 30);
        assert!(
            editor_screen.contains("Çalışma Planı Düzenleyici"),
            "{editor_screen}"
        );
        app.key(press(KeyCode::Char('a'))).unwrap();
        let prompt_screen = render(&app, 100, 30);
        assert!(prompt_screen.contains("Yeni Görev"), "{prompt_screen}");
        app.key(press(KeyCode::Esc)).unwrap();
        app.key(press(KeyCode::Esc)).unwrap();

        // 2. AiSettingsModal: verify raw typed secret never appears in buffer.
        app.key(press(KeyCode::F(5))).unwrap();
        app.key(press(KeyCode::Tab)).unwrap(); // 1: model
        app.key(press(KeyCode::Tab)).unwrap(); // 2: effort
        app.key(press(KeyCode::Tab)).unwrap(); // 3: base_url
        app.key(press(KeyCode::Tab)).unwrap(); // 4: gemini_key
        for ch in "sk-super-secret-xyz999".chars() {
            app.key(press(KeyCode::Char(ch))).unwrap();
        }
        for (w, h) in [(100, 30), (60, 16)] {
            let ai_screen = render(&app, w, h);
            assert!(!ai_screen.contains("sk-super-secret-xyz999"));
            assert!(ai_screen.contains("AI Sağlayıcı") || ai_screen.contains("AI"));
        }
        app.key(press(KeyCode::Esc)).unwrap();
        app.key(press(KeyCode::F(4))).unwrap();
        app.key(press(KeyCode::F(5))).unwrap();
        let en_screen = render(&app, 100, 30);
        assert!(
            en_screen.contains("AI Provider & Model Settings"),
            "{en_screen}"
        );
    }

    #[test]
    fn anki_tab_renders_overview_2button_speed_focus_incubation_and_strategy() {
        let (_root, mut app) = app_with_subject();
        let cards = app
            .store
            .add_flashcards(
                vec![crate::store::NewFlashcard {
                    subject: "Ders".into(),
                    tag: "#Ders::Baglac::Zitlik".into(),
                    status: crate::fsrs_engine::CardStatus::Active,
                    card_type: crate::fsrs_engine::CardKind::Cloze,
                    context_prefix: "[YDS - Bağlaç]".into(),
                    front: "{{c1::Although}} tam cümle alır.".into(),
                    back: "**Although** + **tam cümle**".into(),
                    extra_context: "Despite isim öbeği alır.".into(),
                    mnemonic_hint: Some("ALL-Sentence".into()),
                    is_high_yield: true,
                    source_question_id: None,
                }],
                Utc::now(),
            )
            .unwrap();
        let mut t = Utc::now();
        app.store
            .record_flashcard_review(cards[0].id, crate::fsrs_engine::BinaryRating::Good, 1500, t)
            .unwrap();
        t += chrono::Duration::minutes(35);
        app.store
            .record_flashcard_review(cards[0].id, crate::fsrs_engine::BinaryRating::Good, 1500, t)
            .unwrap();
        for _ in 0..3 {
            t += chrono::Duration::days(2);
            app.store
                .record_flashcard_review(
                    cards[0].id,
                    crate::fsrs_engine::BinaryRating::Again,
                    2000,
                    t,
                )
                .unwrap();
            if app.store.flashcard(cards[0].id).unwrap().unwrap().state
                == crate::fsrs_engine::CardState::Relearning
            {
                t += chrono::Duration::minutes(15);
                app.store
                    .record_flashcard_review(
                        cards[0].id,
                        crate::fsrs_engine::BinaryRating::Good,
                        2000,
                        t,
                    )
                    .unwrap();
            }
        }
        app.store
            .add_flashcards(
                vec![crate::store::NewFlashcard {
                    subject: "Ders".into(),
                    tag: "#Ders::Tense::Perfect".into(),
                    status: crate::fsrs_engine::CardStatus::Incubation,
                    card_type: crate::fsrs_engine::CardKind::Qa,
                    context_prefix: "[YDS - Tense]".into(),
                    front: "Since + Past Simple sonrasında ana cümle ne olur?".into(),
                    back: "**Present Perfect** (**have/has V3**)".into(),
                    extra_context: String::new(),
                    mnemonic_hint: None,
                    is_high_yield: false,
                    source_question_id: None,
                }],
                Utc::now(),
            )
            .unwrap();
        app.store
            .add_exam_technique_note(
                "Ders",
                None,
                "#Ders::Teknik",
                "Olumsuz soru kökünü çiz.",
                Utc::now(),
            )
            .unwrap();
        app.refresh().unwrap();

        // 1. Overview renders TR & EN at normal & small terminal sizes.
        app.key(press(KeyCode::Char('8'))).unwrap();
        assert_eq!(app.tab, ANKI_TAB);
        for (w, h) in [(100, 30), (80, 24), (60, 16)] {
            let overview = render(&app, w, h);
            assert!(overview.contains("FSRS Kart Havuzu"), "{w}x{h}:\n{overview}");
            if w >= 80 {
                assert!(
                    overview.contains("#Ders::Baglac::Zitlik"),
                    "{w}x{h}:\n{overview}"
                );
            } else {
                assert!(overview.contains("#Ders"), "{w}x{h}:\n{overview}");
            }
        }

        // 2. Active 2-Button Review + Speed Focus warning (>6s for Cloze) + Leech [r] button.
        let reloaded = app.store.flashcard(cards[0].id).unwrap().unwrap();
        app.anki_session = Some(AnkiSession {
            subject: "Ders".into(),
            cards: vec![reloaded],
            position: 0,
            revealed: true,
            shown_at: std::time::Instant::now() - std::time::Duration::from_secs(9),
            again_count: 0,
            good_count: 0,
            tag_filter: None,
            high_yield_only: false,
        });
        let review_tr = render(&app, 100, 30);
        assert!(review_tr.contains("HIZ ODAĞI!"), "{review_tr}");
        assert!(review_tr.contains("Hatırlamadım"), "{review_tr}");
        assert!(review_tr.contains("Hatırladım"), "{review_tr}");
        assert!(review_tr.contains("[r] AI Onar"), "{review_tr}");
        assert!(review_tr.contains("ALL-Sentence"), "{review_tr}");
        // Strictly 2 rating buttons (no Easy / Hard).
        assert!(!review_tr.contains("[3]"));
        assert!(!review_tr.contains("[4]"));

        // Switch to English (F4) and verify EN labels.
        app.key(press(KeyCode::F(4))).unwrap();
        let review_en = render(&app, 100, 30);
        assert!(review_en.contains("SPEED FOCUS!"), "{review_en}");
        assert!(review_en.contains("[r] AI Repair"), "{review_en}");

        // 3. Incubation & Strategy sub-views render cleanly.
        app.anki_session = None;
        app.anki_view = AnkiView::Incubation;
        let inc_en = render(&app, 100, 30);
        assert!(inc_en.contains("Incubation Queue"), "{inc_en}");
        app.anki_view = AnkiView::Strategy;
        let strat_en = render(&app, 100, 30);
        assert!(strat_en.contains("Strategy Sheet"), "{strat_en}");
    }
}
