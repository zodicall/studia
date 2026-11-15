//! Ders bazlı çalışma planı takibi. Günler isteğe bağlı olarak `resources/<ders>/course.json`
//! dosyasından yüklenir; sabit gün listesi veya gömülü müfredat yoktur.

use crate::{
    Result,
    course::{DayConfig, TaskConfig, validate_identifier},
    invalid,
    store::Store,
};
use chrono::{Days, NaiveDate, NaiveDateTime, NaiveTime};

const START_KEY_PREFIX: &str = "plan_start";
const DATE_FORMAT: &str = "%Y-%m-%d";

/// Plandaki durum; `today` her çağrıda yerel tarihten verilir.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    NotStarted,
    Upcoming {
        start: NaiveDate,
    },
    Active {
        start: NaiveDate,
        day: usize,
        done: Vec<bool>,
    },
    Finished {
        start: NaiveDate,
    },
}

/// Plan günü `number` (1-tabanlı) bilgisini döndürür.
pub fn day(days: &[DayConfig], number: usize) -> Option<&DayConfig> {
    days.get(number.checked_sub(1)?)
}

/// Plan günü `number` (1-tabanlı) içindeki `index` (0-tabanlı) görevini döndürür.
pub fn task(days: &[DayConfig], number: usize, index: usize) -> Option<&TaskConfig> {
    day(days, number)?.tasks.get(index)
}

/// Gün `number` (1-tabanlı) için tarih: başlangıç + (gün - 1).
pub fn date_of(start: NaiveDate, number: usize) -> NaiveDate {
    start + Days::new(number.saturating_sub(1) as u64)
}

fn start_key(subject: &str) -> Result<String> {
    validate_identifier(subject)?;
    Ok(format!("{START_KEY_PREFIX}:{subject}"))
}

fn done_key(subject: &str, start: NaiveDate, day_num: usize, task_idx: usize) -> Result<String> {
    validate_identifier(subject)?;
    Ok(format!(
        "plan_done:{subject}:{}:{day_num}:{task_idx}",
        start.format(DATE_FORMAT)
    ))
}

pub fn start_date(store: &Store, subject: &str) -> Result<Option<NaiveDate>> {
    let key = start_key(subject)?;
    store
        .state(&key)?
        .map(|value| {
            NaiveDate::parse_from_str(&value, DATE_FORMAT)
                .map_err(|_| invalid("Plan başlangıç tarihi bozuk / Invalid plan start date."))
        })
        .transpose()
}

pub fn set_start(store: &Store, subject: &str, start: NaiveDate) -> Result<()> {
    let key = start_key(subject)?;
    store.set_state(&key, &start.format(DATE_FORMAT).to_string())
}

pub fn day_done(
    store: &Store,
    subject: &str,
    days: &[DayConfig],
    start: NaiveDate,
    day_num: usize,
) -> Result<Vec<bool>> {
    let Some(info) = day(days, day_num) else {
        return Ok(Vec::new());
    };
    let mut done = Vec::with_capacity(info.tasks.len());
    for idx in 0..info.tasks.len() {
        let key = done_key(subject, start, day_num, idx)?;
        done.push(store.state(&key)?.is_some());
    }
    Ok(done)
}

pub fn status(
    store: &Store,
    subject: &str,
    days: &[DayConfig],
    today: NaiveDate,
) -> Result<Status> {
    if days.is_empty() {
        return Ok(Status::NotStarted);
    }
    let Some(start) = start_date(store, subject)? else {
        return Ok(Status::NotStarted);
    };
    let offset = (today - start).num_days();
    if offset < 0 {
        return Ok(Status::Upcoming { start });
    }
    let Some(index) = usize::try_from(offset).ok().filter(|idx| *idx < days.len()) else {
        return Ok(Status::Finished { start });
    };
    let day_num = index + 1;
    let done = day_done(store, subject, days, start, day_num)?;
    Ok(Status::Active {
        start,
        day: day_num,
        done,
    })
}

/// Görevi tamamlandı olarak işaretler.
pub fn mark_done(
    store: &Store,
    subject: &str,
    start: NaiveDate,
    day_num: usize,
    task_idx: usize,
) -> Result<()> {
    let key = done_key(subject, start, day_num, task_idx)?;
    store.set_state(&key, "1")
}

pub fn callback_data(day_num: usize, task_idx: usize) -> String {
    format!("p:{day_num}:{task_idx}")
}

pub fn parse_callback(data: &str, total_days: usize) -> Option<(usize, usize)> {
    let (day_str, task_str) = data.strip_prefix("p:")?.split_once(':')?;
    if day_str.is_empty()
        || task_str.is_empty()
        || !day_str.bytes().all(|b| b.is_ascii_digit())
        || !task_str.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let day_num: usize = day_str.parse().ok()?;
    let task_idx: usize = task_str.parse().ok()?;
    (total_days > 0 && (1..=total_days).contains(&day_num)).then_some((day_num, task_idx))
}

pub fn day_callback_data(day_num: usize) -> String {
    format!("pday:{day_num}")
}

pub fn parse_day_callback(data: &str, total_days: usize) -> Option<usize> {
    let day_str = data.strip_prefix("pday:")?;
    if day_str.is_empty() || !day_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let day_num: usize = day_str.parse().ok()?;
    (total_days > 0 && (1..=total_days).contains(&day_num)).then_some(day_num)
}

/// Sabah bildirimi saati: `HH:MM`; geçersizse 08:30.
pub fn morning_time(value: &str) -> NaiveTime {
    if value.len() == 5
        && value.as_bytes()[2] == b':'
        && value
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 2 || b.is_ascii_digit())
        && let Ok(time) = NaiveTime::parse_from_str(value, "%H:%M")
    {
        return time;
    }
    NaiveTime::from_hms_opt(8, 30, 0).unwrap_or(NaiveTime::MIN)
}

pub fn morning_due(now: NaiveDateTime, time: NaiveTime, last: Option<&str>) -> bool {
    now.time() >= time && last != Some(now.date().to_string().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::course::TaskConfig;

    fn date(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, DATE_FORMAT).unwrap()
    }

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("studia.db")).unwrap();
        (dir, store)
    }

    fn sample_days() -> Vec<DayConfig> {
        vec![
            DayConfig {
                day: 1,
                title: "Diagnostic".into(),
                tasks: vec![
                    TaskConfig {
                        command: "/review".into(),
                        label: "Review due cards".into(),
                    },
                    TaskConfig {
                        command: "/lesson Basics".into(),
                        label: "Read basics".into(),
                    },
                ],
            },
            DayConfig {
                day: 2,
                title: "Practice".into(),
                tasks: vec![
                    TaskConfig {
                        command: "/review".into(),
                        label: "Review".into(),
                    },
                    TaskConfig {
                        command: "/quiz Basics 5".into(),
                        label: "Quick quiz".into(),
                    },
                    TaskConfig {
                        command: "".into(),
                        label: "Read notes".into(),
                    },
                ],
            },
            DayConfig {
                day: 3,
                title: "Exam Day".into(),
                tasks: vec![],
            },
        ]
    }

    #[test]
    fn spec_plan_1_2_empty_and_multi_subject_isolation() {
        let (_dir, store) = store();
        let days = sample_days();
        let d1 = date("2026-11-05");

        // Flexible course without plan returns NotStarted cleanly.
        assert_eq!(status(&store, "flex", &[], d1).unwrap(), Status::NotStarted);
        assert!(day(&[], 1).is_none());

        // Strictly namespaced per subject: starting math leaves history and legacy bare key untouched.
        set_start(&store, "math", d1).unwrap();
        mark_done(&store, "math", d1, 1, 0).unwrap();
        assert_eq!(
            store.state("plan_start:math").unwrap().as_deref(),
            Some("2026-11-05")
        );
        assert!(store.state("plan_start").unwrap().is_none());
        assert_eq!(start_date(&store, "history").unwrap(), None);

        let math_day1 = day_done(&store, "math", &days, d1, 1).unwrap();
        let hist_day1 = day_done(&store, "history", &days, d1, 1).unwrap();
        assert_eq!(math_day1, vec![true, false]);
        assert_eq!(hist_day1, vec![false, false]);

        assert!(set_start(&store, "../escape", d1).is_err());
    }

    #[test]
    fn status_follows_arbitrary_plan_length_and_stops_after_last_day() {
        let (_dir, store) = store();
        let days = sample_days();
        let start = date("2026-11-05");

        assert_eq!(
            status(&store, "math", &days, start).unwrap(),
            Status::NotStarted
        );
        set_start(&store, "math", start).unwrap();
        assert_eq!(
            status(&store, "math", &days, date("2026-11-04")).unwrap(),
            Status::Upcoming { start }
        );
        assert_eq!(
            status(&store, "math", &days, start).unwrap(),
            Status::Active {
                start,
                day: 1,
                done: vec![false, false]
            }
        );
        assert_eq!(
            status(&store, "math", &days, date("2026-11-07")).unwrap(),
            Status::Active {
                start,
                day: 3,
                done: vec![]
            }
        );
        assert_eq!(
            status(&store, "math", &days, date("2026-11-08")).unwrap(),
            Status::Finished { start }
        );
    }

    #[test]
    fn spec_plan_3_callback_data_respects_dynamic_total_days() {
        assert_eq!(parse_callback(&callback_data(3, 1), 3), Some((3, 1)));
        assert_eq!(parse_callback(&callback_data(4, 0), 3), None);
        assert_eq!(parse_callback(&callback_data(1, 0), 0), None);
        for bad in [
            "", "p:", "p:1", "p:0:1", "p:a:1", "p:1:x", "t:1:1", "p:1:2:3",
        ] {
            assert_eq!(parse_callback(bad, 10), None, "{bad}");
        }
        assert_eq!(parse_day_callback(&day_callback_data(3), 3), Some(3));
        assert_eq!(parse_day_callback(&day_callback_data(4), 3), None);
        assert_eq!(parse_day_callback(&day_callback_data(1), 0), None);
        for bad in ["", "pday:", "pday:0", "pday:x", "p:1"] {
            assert_eq!(parse_day_callback(bad, 10), None, "{bad}");
        }
    }

    #[test]
    fn date_of_and_task_lookup_work_across_custom_days() {
        let days = sample_days();
        assert_eq!(date_of(date("2026-11-05"), 1), date("2026-11-05"));
        assert_eq!(date_of(date("2026-11-05"), 3), date("2026-11-07"));
        assert!(task(&days, 0, 0).is_none());
        assert!(task(&days, 1, 99).is_none());
        assert_eq!(task(&days, 1, 0).unwrap().command, "/review");
        assert!(day(&days, 99).is_none());
    }

    #[test]
    fn morning_clock_supports_late_start_new_day_and_strict_time() {
        let time = morning_time("08:30");
        let at = |s| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap();
        assert!(!morning_due(at("2026-10-14 08:29:59"), time, None));
        assert!(morning_due(at("2026-10-14 08:30:00"), time, None));
        assert!(morning_due(
            at("2026-10-14 23:59:00"),
            time,
            Some("2026-10-13")
        ));
        assert!(!morning_due(
            at("2026-10-14 23:59:59"),
            time,
            Some("2026-10-14")
        ));
        for invalid in ["", "8:30", "24:00", "08:60", "08:30:00", "aa:bb"] {
            assert_eq!(morning_time(invalid), time);
        }
    }
}
