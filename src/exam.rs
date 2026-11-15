//! Mock exam bank discovery, safe persistence, and answer-key-backed import.
//! Discovers JSON exam banks under `resources/<subject>/exams/*.json` and
//! `data/exams/<subject>/*.json`, sorted deterministically by slug.

use crate::{Result, ai, course, invalid, store::Store};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use std::ops::RangeInclusive;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Questions are requested from AI in chunks of up to 40 items.
#[cfg(test)]
pub const CHUNK_SIZE: usize = 40;
/// Upper bound on mock exam questions imported in one bank.
pub const MAX_BANK_SIZE: usize = 200;

/// Metadata for a discovered mock exam bank.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredBank {
    pub slug: String,
    pub path: PathBuf,
    pub title: String,
}

/// Saved mock exam bank: display title and ordered questions.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Bank {
    pub title: String,
    pub questions: Vec<ai::Question>,
}

/// State key tracking whether a bank has already been completed/started for a subject.
pub fn used_key(subject: &str, slug: &str) -> String {
    format!("deneme:used:{subject}:{slug}")
}

/// Cevap anahtarını okur. Anahtar sınavdan sınava farklı dizilir (`1B 2A`, `1. C 2. E`, bloklar halinde
/// ve bazı bloklar sınav metninin içinde); bu yüzden satır bazında taranır. Yalnızca `numara harf`
/// çiftlerinden oluşan satırlar anahtar sayılır; sayfa numaraları ve `A) şık` satırları eşleşmez.
#[cfg(test)]
pub fn parse_key(text: &str, count: usize) -> Result<Vec<usize>> {
    let mut answers: Vec<Option<usize>> = vec![None; count];
    for line in text.lines() {
        let Some(pairs) = key_pairs(line) else {
            continue;
        };
        for (number, letter) in pairs {
            if !(1..=count).contains(&number) {
                continue;
            }
            let index = (letter as u8 - b'A') as usize;
            match answers[number - 1] {
                None => answers[number - 1] = Some(index),
                Some(existing) if existing != index => {
                    return Err(invalid(format!(
                        "Cevap anahtarında {number}. soru için iki farklı cevap var."
                    )));
                }
                Some(_) => {}
            }
        }
    }
    answers
        .into_iter()
        .enumerate()
        .map(|(index, answer)| {
            answer
                .ok_or_else(|| invalid(format!("Cevap anahtarında {} numarası eksik.", index + 1)))
        })
        .collect()
}

/// Satır yalnızca `numara harf` çiftlerinden oluşuyorsa çiftleri döndürür; aksi halde None.
#[cfg(test)]
fn key_pairs(line: &str) -> Option<Vec<(usize, char)>> {
    let mut pairs = Vec::new();
    let mut tokens = line.split_whitespace();
    while let Some(token) = tokens.next() {
        let digits: String = token.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() || digits.len() > 3 {
            return None;
        }
        let number: usize = digits.parse().ok()?;
        let rest = token[digits.len()..].trim_end_matches('.');
        let letter_token = if rest.is_empty() {
            tokens.next()?
        } else {
            rest
        };
        let mut letters = letter_token.chars();
        let letter = letters.next()?;
        if letters.next().is_some() || !('A'..='E').contains(&letter) {
            return None;
        }
        pairs.push((number, letter));
    }
    (!pairs.is_empty()).then_some(pairs)
}

/// Prompt and answer key hints for importing a chunk of exam items.
#[cfg(test)]
fn prompt(
    text: &str,
    range: &RangeInclusive<usize>,
    answers: &[usize],
    choices_count: usize,
) -> String {
    let letters: Vec<String> = range
        .clone()
        .map(|number| format!("{number}-{}", (b'A' + answers[number - 1] as u8) as char))
        .collect();
    format!(
        "Aşağıdaki metin bir sınav belgesinin OCR çıktısıdır; bozuk kelimeler ve eksik harfler içerebilir. \
        Yalnızca {start}–{end} arası soruları çıkar. Her soru için:\n\
        - no: soru numarası\n\
        - question: soru metni. Paragraf, diyalog veya boşluk doldurma sorusuysa ilgili metnin tamamını da ekle; boşluk '----' ile gösterilsin.\n\
        - choices: tam {choices_count} şık, yalnızca metin (A), B) gibi harf veya numara olmadan).\n\
        - explanation: 1–2 kısa cümle; doğru cevabın neden doğru olduğunu söyle.\n\
        Doğru cevaplar kesindir, değiştirme: {letters}.\n\
        OCR bozukluklarını yalnızca okunabiliyorsa düzelt (birleşik veya bölünmüş kelimeler gibi). Bir kelime, sayı veya cümle parçası kaybolmuşsa \
        bunu TAHMİN EDEREK DOLDURMA: broken alanını true yap ve broken_reason alanında eksiği kısaca yaz (örn. boşluktaki sayı kayıp, cümle yarım). \
        Bozuk olmayan sorularda broken false ve broken_reason boş olsun.\n\n\
        Sınav metni:\n---\n{text}\n---",
        start = range.start(),
        end = range.end(),
        letters = letters.join(", "),
    )
}

#[cfg(test)]
fn schema(range: &RangeInclusive<usize>, choices_count: usize) -> String {
    let len = range.end() - range.start() + 1;
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "items": {
                "type": "array",
                "minItems": len,
                "maxItems": len,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "no": {"type": "integer", "minimum": range.start(), "maximum": range.end()},
                        "question": {"type": "string", "minLength": 1},
                        "choices": {"type": "array", "items": {"type": "string", "minLength": 1}, "minItems": choices_count, "maxItems": choices_count},
                        "explanation": {"type": "string", "minLength": 1},
                        "broken": {"type": "boolean"},
                        "broken_reason": {"type": "string"}
                    },
                    "required": ["no", "question", "choices", "explanation", "broken", "broken_reason"]
                }
            }
        },
        "required": ["items"]
    })
    .to_string()
}

#[cfg(test)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    no: usize,
    question: String,
    choices: Vec<String>,
    explanation: String,
    #[serde(default)]
    broken: bool,
    #[serde(default)]
    broken_reason: String,
}

/// Soru metninin başına konan OCR bozukluk işareti.
pub const BROKEN_MARK: &str = "⚠ OCR bozuk:";

/// Soru OCR bozuk olarak işaretlenmiş mi?
pub fn is_broken(question: &str) -> bool {
    question.starts_with(BROKEN_MARK)
}

#[cfg(test)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Items {
    items: Vec<Item>,
}

/// AI yanıtını doğrular ve cevap anahtarından gelen doğru şıkla birleştirir.
#[cfg(test)]
pub fn parse_items(
    raw: &str,
    range: &RangeInclusive<usize>,
    answers: &[usize],
    title: &str,
    choices_count: usize,
) -> Result<Vec<ai::Question>> {
    if !(4..=5).contains(&choices_count) {
        return Err(invalid(
            "Şık sayısı 4 veya 5 olmalıdır / Choices count must be 4 or 5.",
        ));
    }
    let parsed: Items = serde_json::from_str(ai::strip_code_fence(raw))?;
    let mut questions = Vec::new();
    for number in range.clone() {
        if number == 0 || number > answers.len() {
            return Err(invalid(format!(
                "{number}. soru için cevap anahtarı eksik / Missing answer key for question {number}."
            )));
        }
        let mut matches = parsed.items.iter().filter(|item| item.no == number);
        let item = matches.next().ok_or_else(|| {
            invalid(format!(
                "{number}. soru üretilmedi / Question {number} was not generated."
            ))
        })?;
        if matches.next().is_some() {
            return Err(invalid(format!(
                "{number}. soru birden fazla kez üretildi / Question {number} was generated multiple times."
            )));
        }
        let choices: Vec<String> = item
            .choices
            .iter()
            .map(|choice| {
                let text = ai::clean_choice(choice).trim();
                if text.is_empty() {
                    "(metin eksik)".to_string()
                } else {
                    text.to_string()
                }
            })
            .collect();
        if choices.len() != choices_count {
            return Err(invalid(format!(
                "{number}. sorunun şıkları geçersiz / Invalid choices for question {number}."
            )));
        }
        let answer_index = answers[number - 1];
        if answer_index >= choices.len() {
            return Err(invalid(format!(
                "{number}. soru için cevap indeksi ({answer_index}) şık sayısı ({}) dışında / Answer index ({answer_index}) out of bounds ({}) for question {number}.",
                choices.len(),
                choices.len()
            )));
        }
        // Aynı şıklar veya OCR'dan boş çıkan şık: soru bozuk işaretlenir, sınavın geri kalanı atılmaz.
        let mut distinct = choices.clone();
        distinct.sort();
        distinct.dedup();
        let mut broken = item.broken;
        let mut reason = item.broken_reason.trim().to_string();
        if distinct.len() != choices_count {
            broken = true;
            reason = format!("{reason} şıklar tekrar ediyor veya bozuk")
                .trim()
                .to_string();
        }
        let question = item.question.trim();
        let explanation = item.explanation.trim();
        if question.is_empty() || explanation.is_empty() {
            return Err(invalid(format!(
                "{number}. soru veya açıklama boş / Question {number} text or explanation is empty."
            )));
        }
        // Bozuk işaretli soru silinmez; metnin başına neyin eksik olduğu yazılır.
        let question = if broken {
            let reason = if reason.is_empty() {
                "metin eksik veya bozuk".to_string()
            } else {
                reason
            };
            format!("{BROKEN_MARK} {reason}\n\n{question}")
        } else {
            question.to_string()
        };
        questions.push(ai::Question {
            question,
            explanation: format!("Doğru cevap: '{}'. {explanation}", choices[answer_index]),
            choices,
            answer_index,
            source: format!("{title}, Soru {number}"),
            topic: None,
        });
    }
    Ok(questions)
}

/// Sınav metnini `CHUNK_SIZE` (40) soruluk parçalarda AI'ye çıkartır.
#[cfg(test)]
pub fn import_with(
    title: &str,
    text: &str,
    answers: &[usize],
    choices_count: usize,
    mut generate: impl FnMut(RangeInclusive<usize>, &str, &str) -> Result<String>,
) -> Result<Vec<ai::Question>> {
    if !(4..=5).contains(&choices_count) {
        return Err(invalid(
            "Şık sayısı 4 veya 5 olmalıdır / Choices count must be 4 or 5.",
        ));
    }
    if answers.is_empty() || answers.len() > MAX_BANK_SIZE {
        return Err(invalid(format!(
            "Soru sayısı 1–{MAX_BANK_SIZE} arasında olmalıdır / Question count must be between 1 and {MAX_BANK_SIZE}."
        )));
    }
    if let Some((i, &ans)) = answers
        .iter()
        .enumerate()
        .find(|(_, ans)| **ans >= choices_count)
    {
        return Err(invalid(format!(
            "{}. soru için cevap indeksi ({ans}) şık sayısı ({choices_count}) dışında / Answer index ({ans}) out of bounds ({choices_count}) for question {}.",
            i + 1,
            i + 1
        )));
    }
    let mut questions = Vec::with_capacity(answers.len());
    let mut start = 1usize;
    while start <= answers.len() {
        let end = (start + CHUNK_SIZE - 1).min(answers.len());
        let range = start..=end;
        let raw = generate(
            range.clone(),
            &prompt(text, &range, answers, choices_count),
            &schema(&range, choices_count),
        )?;
        questions.extend(parse_items(&raw, &range, answers, title, choices_count)?);
        start = end + 1;
    }
    Ok(questions)
}

#[cfg(test)]
fn saved_bank_path(root: &Path, subject: &str, slug: &str) -> Result<PathBuf> {
    let subject = course::validate_identifier(subject)?;
    let slug = course::validate_identifier(slug)?;
    Ok(root
        .join("data")
        .join("exams")
        .join(subject)
        .join(format!("{slug}.json")))
}

#[cfg(test)]
fn resource_bank_path(root: &Path, subject: &str, slug: &str) -> Result<PathBuf> {
    let subject = course::validate_identifier(subject)?;
    let slug = course::validate_identifier(slug)?;
    Ok(crate::sources::sources_dir(root)
        .join(subject)
        .join("exams")
        .join(format!("{slug}.json")))
}

fn validate_bank(bank: &Bank) -> Result<()> {
    if bank.title.trim().is_empty() {
        return Err(invalid(
            "Sınav bankası başlığı boş olamaz / Exam bank title cannot be empty.",
        ));
    }
    for (idx, q) in bank.questions.iter().enumerate() {
        if q.question.trim().is_empty() {
            return Err(invalid(format!(
                "{}. soru metni boş / Question {} text is empty.",
                idx + 1,
                idx + 1
            )));
        }
        if !(4..=5).contains(&q.choices.len()) {
            return Err(invalid(format!(
                "{}. sorunun şık sayısı 4 veya 5 olmalıdır / Question {} must have 4 or 5 choices.",
                idx + 1,
                idx + 1
            )));
        }
        if q.answer_index >= q.choices.len() {
            return Err(invalid(format!(
                "{}. soru için cevap indeksi şık sayısı dışında / Answer index out of bounds for question {}.",
                idx + 1,
                idx + 1
            )));
        }
    }
    Ok(())
}

fn load_file(path: &Path) -> Result<Option<Bank>> {
    if !path.is_file() {
        return Ok(None);
    }
    let len = fs::metadata(path)?.len();
    if len > course::MAX_COURSE_JSON_BYTES {
        return Err(invalid(format!(
            "Sınav bankası çok büyük ({len} bayt) / Exam bank too large ({len} bytes): {}",
            path.display()
        )));
    }
    let bank: Bank = serde_json::from_str(&fs::read_to_string(path)?)?;
    validate_bank(&bank)?;
    Ok(Some(bank))
}

#[cfg(test)]
pub fn save(root: &Path, subject: &str, slug: &str, bank: &Bank) -> Result<PathBuf> {
    let path = saved_bank_path(root, subject, slug)?;
    crate::config::atomic_write_private(&path, serde_json::to_string_pretty(bank)?.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
pub fn load(root: &Path, subject: &str, slug: &str) -> Result<Option<Bank>> {
    let saved = saved_bank_path(root, subject, slug)?;
    if let Some(bank) = load_file(&saved)? {
        return Ok(Some(bank));
    }
    let resource = resource_bank_path(root, subject, slug)?;
    load_file(&resource)
}

fn collect_banks_in_dir(
    base_root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, DiscoveredBank>,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let canon_base = fs::canonicalize(base_root)?;
    let canon_dir = fs::canonicalize(dir)?;
    if canon_dir == canon_base || !canon_dir.starts_with(&canon_base) {
        return Err(invalid(format!(
            "Sınav dizini izin verilen kök dışına taşıyor / Exam directory escapes allowed root: {}",
            dir.display()
        )));
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(canon_file) = fs::canonicalize(&path) else {
            continue;
        };
        if !canon_file.starts_with(&canon_dir) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(slug) = course::validate_identifier(stem) else {
            continue;
        };
        let Ok(Some(bank)) = load_file(&path) else {
            continue;
        };
        out.insert(
            slug.to_string(),
            DiscoveredBank {
                slug: slug.to_string(),
                path,
                title: bank.title,
            },
        );
    }
    Ok(())
}

/// Discovers valid mock exam banks under `<root>/resources/<subject>/exams/*.json`
/// and `<root>/data/exams/<subject>/*.json`, sorted deterministically by slug.
pub fn discover_banks(root: &Path, subject: &str) -> Result<Vec<DiscoveredBank>> {
    let subject = course::validate_identifier(subject)?;
    let mut found = BTreeMap::new();
    let resources_root = crate::sources::sources_dir(root);
    let exams_dir = resources_root.join(subject).join("exams");
    collect_banks_in_dir(&resources_root, &exams_dir, &mut found)?;
    let data_exams_root = root.join("data").join("exams");
    let data_exams_dir = data_exams_root.join(subject);
    collect_banks_in_dir(&data_exams_root, &data_exams_dir, &mut found)?;
    Ok(found.into_values().collect())
}

/// Returns the first unused exam bank for a subject in deterministic slug order.
pub fn next_unused_bank(
    store: &Store,
    root: &Path,
    subject: &str,
) -> Result<Option<(DiscoveredBank, Bank)>> {
    for meta in discover_banks(root, subject)? {
        if store.state(&used_key(subject, &meta.slug))?.is_some() {
            continue;
        }
        if let Some(bank) = load_file(&meta.path)? {
            if bank.questions.is_empty() {
                continue;
            }
            return Ok(Some((meta, bank)));
        }
    }
    Ok(None)
}

/// Test için sahte AI yanıtı: verilen aralığın her sorusu için şık ve açıklama üretir.
#[cfg(test)]
pub fn fake_items(range: &RangeInclusive<usize>) -> String {
    let items: Vec<serde_json::Value> = range
        .clone()
        .map(|no| {
            json!({
                "no": no,
                "question": format!("Soru {no} ----"),
                "choices": ["a", "b", "c", "d", "e"],
                "explanation": "Kural açıklaması."
            })
        })
        .collect();
    json!({ "items": items }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_key_reads_the_answer_key_layouts_of_the_three_exams() {
        let compact = "CEVAP ANAHTARI\n\n1B 2C 3B 4D 5C 6B 7E 8B 9D 10 B";
        assert_eq!(
            parse_key(compact, 10).unwrap(),
            vec![1, 2, 1, 3, 2, 1, 4, 1, 3, 1]
        );
        let grouped = "ANSWER KEY MOCK EXAM 1\n\n3 D 1 B 2 C";
        assert_eq!(parse_key(grouped, 3).unwrap(), vec![1, 2, 3]);
        let dotted = "TEST OF ENGLISH\n1. C 2. E 3. D 4. A 5. A";
        assert_eq!(parse_key(dotted, 5).unwrap(), vec![2, 4, 3, 0, 0]);
        assert!(parse_key("ANSWER KEY\n1 A 2 B", 3).is_err());
    }

    #[test]
    fn key_ignores_page_numbers_and_choice_lines_in_the_exam_body() {
        // Sayfa numarası (45), şık satırları (A) I) ve sınav başlığı anahtar sayılmamalı.
        let body = "45\n\nA) I\nB) II\n\n2026 SPRING EXAM\n\nA man walks 2 A ways\n\n1B 2A\n3D";
        assert_eq!(parse_key(body, 3).unwrap(), vec![1, 0, 3]);
    }

    #[test]
    fn parse_items_takes_the_answer_from_the_key_and_rejects_bad_items() {
        let range = 1..=2;
        let answers = [1, 0];
        let raw = fake_items(&range);
        let questions = parse_items(&raw, &range, &answers, "Academic Mock 2026", 5).unwrap();
        assert_eq!(questions[0].answer_index, 1);
        assert!(questions[0].explanation.starts_with("Doğru cevap: 'b'."));
        assert_eq!(questions[1].source, "Academic Mock 2026, Soru 2");
        let missing = json!({"items": [{"no": 1, "question": "q", "choices": ["a","b","c","d","e"], "explanation": "x"}]});
        assert!(parse_items(&missing.to_string(), &range, &answers, "t", 5).is_err());
        let four = json!({"items": [
            {"no": 1, "question": "q", "choices": ["a","b","c","d"], "explanation": "x"},
            {"no": 2, "question": "q", "choices": ["a","b","c","d","e"], "explanation": "x"}
        ]});
        assert!(parse_items(&four.to_string(), &range, &answers, "t", 5).is_err());
    }

    #[test]
    fn broken_items_are_marked_at_the_start_of_the_question_text() {
        let range = 1..=2;
        let answers = [1, 0];
        let raw = json!({"items": [
            {"no": 1, "question": "Complete: ----", "choices": ["a","b","c","d","e"], "explanation": "x", "broken": false, "broken_reason": ""},
            {"no": 2, "question": "and only ---- percent", "choices": ["a","b","c","d","e"], "explanation": "x", "broken": true, "broken_reason": "boşluktaki sayı kayıp"}
        ]});
        let questions = parse_items(&raw.to_string(), &range, &answers, "Exam", 5).unwrap();
        assert!(!is_broken(&questions[0].question));
        assert_eq!(questions[0].question, "Complete: ----");
        assert!(is_broken(&questions[1].question));
        assert!(questions[1].question.contains("boşluktaki sayı kayıp"));
        assert!(questions[1].question.ends_with("and only ---- percent"));
    }

    #[test]
    fn duplicate_choices_mark_the_item_broken_instead_of_failing_the_exam() {
        let range = 1..=2;
        let answers = [1, 0];
        let raw = json!({"items": [
            {"no": 1, "question": "Q1", "choices": ["a","a","c","d","e"], "explanation": "x", "broken": false, "broken_reason": ""},
            {"no": 2, "question": "Q2", "choices": ["a","b","c","d","e"], "explanation": "x", "broken": false, "broken_reason": ""}
        ]});
        let questions = parse_items(&raw.to_string(), &range, &answers, "Exam", 5).unwrap();
        assert!(is_broken(&questions[0].question));
        assert!(!is_broken(&questions[1].question));
    }

    #[test]
    fn import_builds_a_full_exam_from_two_requests() {
        let answers: Vec<usize> = (0..80).map(|i| i % 5).collect();
        let questions = import_with("Sample Test", "metin", &answers, 5, |range, _, _| {
            Ok(fake_items(&range))
        })
        .unwrap();
        assert_eq!(questions.len(), 80);
        assert_eq!(questions[79].answer_index, 79 % 5);
    }

    #[test]
    fn saved_and_missing_banks_load_and_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let bank = Bank {
            title: "Sample Exam".into(),
            questions: vec![],
        };
        save(dir.path(), "english", "sample_1", &bank).unwrap();
        assert_eq!(
            load(dir.path(), "english", "sample_1")
                .unwrap()
                .unwrap()
                .title,
            "Sample Exam"
        );
        assert!(load(dir.path(), "english", "missing").unwrap().is_none());
    }

    #[test]
    fn spec_exam_1_2_dynamic_discovery_and_traversal_guard() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let bio_exams = root.join("resources/Biology/exams");
        fs::create_dir_all(&bio_exams).unwrap();
        let midterm = Bank {
            title: "Biology Midterm".into(),
            questions: vec![ai::Question {
                question: "Cell powerhouse?".into(),
                choices: ["Mitochondria", "Nucleus", "Ribosome", "Golgi"]
                    .map(String::from)
                    .into(),
                answer_index: 0,
                explanation: "Correct answer: 'Mitochondria'.".into(),
                source: "midterm".into(),
                topic: Some("Cells".into()),
            }],
        };
        fs::write(
            bio_exams.join("midterm.json"),
            serde_json::to_string_pretty(&midterm).unwrap(),
        )
        .unwrap();

        let practice = Bank {
            title: "Biology Practice 1".into(),
            questions: vec![ai::Question {
                question: "DNA stands for?".into(),
                choices: [
                    "Deoxyribonucleic acid",
                    "Ribonucleic acid",
                    "Amino acid",
                    "Fatty acid",
                ]
                .map(String::from)
                .into(),
                answer_index: 0,
                explanation: "Correct answer: 'Deoxyribonucleic acid'.".into(),
                source: "practice".into(),
                topic: Some("Genetics".into()),
            }],
        };
        save(root, "Biology", "practice_1", &practice).unwrap();

        let discovered = discover_banks(root, "Biology").unwrap();
        let slugs: Vec<&str> = discovered.iter().map(|b| b.slug.as_str()).collect();
        assert_eq!(slugs, vec!["midterm", "practice_1"]);

        assert!(discover_banks(root, "../etc").is_err());
        assert!(load(root, "Biology", "../evil").is_err());
        assert!(save(root, "Biology", "../evil", &practice).is_err());

        let store = Store::open(&root.join("data/studia.db")).unwrap();
        store
            .set_state(&used_key("Biology", "midterm"), "1")
            .unwrap();
        let (next_meta, next_bank) = next_unused_bank(&store, root, "Biology").unwrap().unwrap();
        assert_eq!(next_meta.slug, "practice_1");
        assert_eq!(next_bank.title, "Biology Practice 1");
    }

    #[test]
    fn spec_exam_3_arbitrary_chunking_4_choices_and_bounds_check() {
        let answers15 = vec![0usize; 15];
        let mut ranges = Vec::new();
        let questions15 = import_with("Short Quiz", "text", &answers15, 4, |range, _, _| {
            ranges.push(range.clone());
            let items: Vec<serde_json::Value> = range
                .map(|no| {
                    json!({
                        "no": no,
                        "question": format!("Q{no}"),
                        "choices": ["a", "b", "c", "d"],
                        "explanation": "Rule."
                    })
                })
                .collect();
            Ok(json!({ "items": items }).to_string())
        })
        .unwrap();
        assert_eq!(ranges, vec![1..=15]);
        assert_eq!(questions15.len(), 15);
        assert_eq!(questions15[0].choices.len(), 4);

        let answers85 = vec![1usize; 85];
        let mut ranges85 = Vec::new();
        let questions85 = import_with("Big Exam", "text", &answers85, 5, |range, _, _| {
            ranges85.push(range.clone());
            Ok(fake_items(&range))
        })
        .unwrap();
        assert_eq!(ranges85, vec![1..=40, 41..=80, 81..=85]);
        assert_eq!(questions85.len(), 85);

        // Out-of-bounds answer_index (4 for 4-choice question) returns Err instead of panicking.
        let oob_raw = json!({"items": [{
            "no": 1,
            "question": "Q1",
            "choices": ["a", "b", "c", "d"],
            "explanation": "Rule."
        }]})
        .to_string();
        assert!(parse_items(&oob_raw, &(1..=1), &[4], "Quiz", 4).is_err());
    }

    #[test]
    fn spec_sec_bank_invariants_and_canonical_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let bad_title = Bank {
            title: "   ".into(),
            questions: vec![],
        };
        let bad_title_path = save(root, "Bio", "bad_title", &bad_title).unwrap();
        assert!(load_file(&bad_title_path).is_err());

        let bad_q = Bank {
            title: "Bad Q".into(),
            questions: vec![ai::Question {
                question: "  ".into(),
                choices: ["a", "b", "c", "d"].map(String::from).into(),
                answer_index: 0,
                explanation: "e".into(),
                source: "s".into(),
                topic: None,
            }],
        };
        let bad_q_path = save(root, "Bio", "bad_q", &bad_q).unwrap();
        assert!(load_file(&bad_q_path).is_err());

        let bad_choices = Bank {
            title: "Bad Choices".into(),
            questions: vec![ai::Question {
                question: "Q?".into(),
                choices: ["a", "b", "c"].map(String::from).into(),
                answer_index: 0,
                explanation: "e".into(),
                source: "s".into(),
                topic: None,
            }],
        };
        let bad_choices_path = save(root, "Bio", "bad_choices", &bad_choices).unwrap();
        assert!(load_file(&bad_choices_path).is_err());

        let bad_ans = Bank {
            title: "Bad Ans".into(),
            questions: vec![ai::Question {
                question: "Q?".into(),
                choices: ["a", "b", "c", "d"].map(String::from).into(),
                answer_index: 4,
                explanation: "e".into(),
                source: "s".into(),
                topic: None,
            }],
        };
        let bad_ans_path = save(root, "Bio", "bad_ans", &bad_ans).unwrap();
        assert!(load_file(&bad_ans_path).is_err());

        // Empty-questions bank is still discoverable (for legacy deletion) but skipped by next_unused_bank.
        let empty_bank = Bank {
            title: "Empty Bank".into(),
            questions: vec![],
        };
        save(root, "Bio", "empty_bank", &empty_bank).unwrap();
        let discovered = discover_banks(root, "Bio").unwrap();
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].slug, "empty_bank");

        let store = Store::open(&root.join("data/studia.db")).unwrap();
        assert!(next_unused_bank(&store, root, "Bio").unwrap().is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = root.join("outside");
            fs::create_dir_all(&outside).unwrap();
            let escaped = Bank {
                title: "Escaped".into(),
                questions: vec![ai::Question {
                    question: "Q?".into(),
                    choices: ["a", "b", "c", "d"].map(String::from).into(),
                    answer_index: 0,
                    explanation: "e".into(),
                    source: "s".into(),
                    topic: None,
                }],
            };
            fs::write(
                outside.join("leak.json"),
                serde_json::to_string(&escaped).unwrap(),
            )
            .unwrap();
            fs::create_dir_all(root.join("resources/EscapeDir")).unwrap();
            symlink(&outside, root.join("resources/EscapeDir/exams")).unwrap();
            assert!(discover_banks(root, "EscapeDir").is_err());

            symlink(
                outside.join("leak.json"),
                root.join("data/exams/Bio/leak.json"),
            )
            .unwrap();
            let bio = discover_banks(root, "Bio").unwrap();
            assert!(bio.iter().all(|b| b.slug != "leak"));
        }
    }
}
