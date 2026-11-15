use crate::{Result, invalid};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, io::ErrorKind, path::Path};

pub const MAX_COURSE_JSON_BYTES: u64 = 2 * 1024 * 1024;
pub const DEFAULT_EXAM_QUESTIONS: usize = 20;
pub const DEFAULT_CHOICES_COUNT: usize = 5;

fn default_exam_count() -> usize {
    DEFAULT_EXAM_QUESTIONS
}

fn default_choices_count() -> usize {
    DEFAULT_CHOICES_COUNT
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CourseConfig {
    pub title: Option<String>,
    pub persona: Option<String>,
    pub exam: Option<ExamConfig>,
    #[serde(default)]
    pub plan: Vec<DayConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExamConfig {
    #[serde(default = "default_exam_count")]
    pub question_count: usize,
    #[serde(default = "default_choices_count")]
    pub choices_count: usize,
    #[serde(default)]
    pub categories: Vec<CategoryWeight>,
    pub rules: Option<String>,
}

impl Default for ExamConfig {
    fn default() -> Self {
        Self {
            question_count: DEFAULT_EXAM_QUESTIONS,
            choices_count: DEFAULT_CHOICES_COUNT,
            categories: Vec::new(),
            rules: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CategoryWeight {
    pub name: String,
    pub count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DayConfig {
    pub day: usize,
    pub title: String,
    pub tasks: Vec<TaskConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    #[serde(default)]
    pub command: String,
    pub label: String,
}

pub fn validate_identifier(name: &str) -> Result<&str> {
    if name.trim().is_empty()
        || name == "."
        || name == ".."
        || name.starts_with('-')
        || name.contains(['/', '\\', ':', '\0'])
        || name.chars().any(char::is_control)
    {
        return Err(invalid(format!("Invalid identifier: {name:?}")));
    }
    Ok(name)
}

impl CourseConfig {
    pub fn display_title<'a>(&'a self, subject: &'a str) -> &'a str {
        self.title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(subject)
    }

    pub fn choices_count(&self) -> usize {
        self.exam
            .as_ref()
            .map_or(DEFAULT_CHOICES_COUNT, |exam| exam.choices_count)
    }

    pub fn exam_question_count(&self) -> usize {
        self.exam
            .as_ref()
            .map_or(DEFAULT_EXAM_QUESTIONS, |exam| exam.question_count)
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(exam) = &self.exam {
            if !(1..=200).contains(&exam.question_count) {
                return Err(invalid("exam.question_count must be between 1 and 200."));
            }
            if !(4..=5).contains(&exam.choices_count) {
                return Err(invalid("exam.choices_count must be 4 or 5."));
            }
            let mut sum = 0usize;
            for cat in &exam.categories {
                if cat.name.trim().is_empty() || cat.count == 0 {
                    return Err(invalid(
                        "exam.categories entries must have non-empty name and count > 0.",
                    ));
                }
                sum = sum.saturating_add(cat.count);
            }
            if !exam.categories.is_empty() && sum > exam.question_count {
                return Err(invalid(
                    "Sum of exam.categories counts exceeds exam.question_count.",
                ));
            }
        }
        let mut seen_days = BTreeSet::new();
        for (idx, day) in self.plan.iter().enumerate() {
            if day.day != idx + 1 || day.day == 0 {
                return Err(invalid(format!(
                    "plan days must be numbered sequentially starting at 1 (got {}).",
                    day.day
                )));
            }
            if !seen_days.insert(day.day) {
                return Err(invalid(format!("Duplicate plan day: {}", day.day)));
            }
            if day.title.trim().is_empty() {
                return Err(invalid(format!(
                    "plan day {} title must not be empty.",
                    day.day
                )));
            }
            for task in &day.tasks {
                if task.label.trim().is_empty() {
                    return Err(invalid(format!(
                        "plan day {} task label must not be empty.",
                        day.day
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn load_or_default(root: &Path, subject: &str) -> Result<Self> {
        validate_identifier(subject)?;
        let sources_root = crate::sources::sources_dir(root);
        let path = sources_root.join(subject).join("course.json");
        let meta = match fs::metadata(&path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(err.into()),
        };
        let canon_root = match fs::canonicalize(&sources_root) {
            Ok(canon) => canon,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(err.into()),
        };
        let canon_path = match fs::canonicalize(&path) {
            Ok(canon) => canon,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(err.into()),
        };
        if canon_path == canon_root || !canon_path.starts_with(&canon_root) {
            return Err(invalid(format!(
                "course.json resources kökünün dışına taşıyor / escapes resources root: {}",
                path.display()
            )));
        }
        if meta.len() > MAX_COURSE_JSON_BYTES {
            return Err(invalid(format!(
                "course.json exceeds 2 MiB limit: {}",
                path.display()
            )));
        }
        let raw = fs::read_to_string(&path)?;
        let parsed: Self = serde_json::from_str(&raw)
            .map_err(|err| invalid(format!("Invalid JSON in {}: {err}", path.display())))?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn save(&self, root: &Path, subject: &str) -> Result<()> {
        validate_identifier(subject)?;
        self.validate()?;
        let sources_root = crate::sources::sources_dir(root);
        fs::create_dir_all(&sources_root)?;
        let canon_root = fs::canonicalize(&sources_root)?;
        let subject_dir = sources_root.join(subject);
        if fs::symlink_metadata(&subject_dir).is_err() {
            fs::create_dir_all(&subject_dir)?;
        }
        let canon_subject = fs::canonicalize(&subject_dir)?;
        if canon_subject == canon_root || !canon_subject.starts_with(&canon_root) {
            return Err(invalid(format!(
                "course.json resources kökünün dışına taşıyor / escapes resources root: {}",
                subject_dir.display()
            )));
        }
        let path = subject_dir.join("course.json");
        if let Ok(canon_path) = fs::canonicalize(&path)
            && (canon_path == canon_root || !canon_path.starts_with(&canon_root))
        {
            return Err(invalid(format!(
                "course.json resources kökünün dışına taşıyor / escapes resources root: {}",
                path.display()
            )));
        }
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        crate::config::atomic_write_private(&path, json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_course_2_zero_config_fallback_without_course_json() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("resources/Rust101")).unwrap();
        let cfg = CourseConfig::load_or_default(dir.path(), "Rust101").unwrap();
        assert_eq!(cfg.display_title("Rust101"), "Rust101");
        assert_eq!(cfg.choices_count(), 5);
        assert_eq!(cfg.exam_question_count(), 20);
        assert!(cfg.plan.is_empty());
    }

    #[test]
    fn spec_course_3_examples_deserialize_and_validate() {
        let general: CourseConfig =
            serde_json::from_str(include_str!("../examples/general-course/course.json")).unwrap();
        general.validate().unwrap();
        assert_eq!(general.choices_count(), 4);
        assert_eq!(general.plan.len(), 3);

        let academic_example: CourseConfig =
            serde_json::from_str(include_str!("../examples/academic-english/course.json")).unwrap();
        academic_example.validate().unwrap();
        assert_eq!(academic_example.choices_count(), 5);
        assert_eq!(academic_example.exam_question_count(), 80);
        assert_eq!(academic_example.plan.len(), 6);
    }

    #[test]
    fn spec_course_3_and_4_reject_invalid_configs_and_traversal() {
        for bad in ["", ".", "..", "../secret", "a/b", r"a\b", "x\0y", "-opt"] {
            assert!(
                validate_identifier(bad).is_err(),
                "expected err for {bad:?}"
            );
        }
        assert!(validate_identifier("Algorithms_101").is_ok());

        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("resources/Bad")).unwrap();
        fs::write(
            dir.path().join("resources/Bad/course.json"),
            r#"{"exam":{"choices_count":7}}"#,
        )
        .unwrap();
        assert!(CourseConfig::load_or_default(dir.path(), "Bad").is_err());

        fs::write(
            dir.path().join("resources/Bad/course.json"),
            r#"{"plan":[{"day":0,"title":"Zero","tasks":[{"command":"/quiz x","label":"x"}]}]}"#,
        )
        .unwrap();
        assert!(CourseConfig::load_or_default(dir.path(), "Bad").is_err());
    }

    #[test]
    fn spec_sec_colon_identifier_and_canonical_containment() {
        for bad in ["C:secret", "foo:bar", ":ads", "C:\\Windows"] {
            assert!(
                validate_identifier(bad).is_err(),
                "expected colon/traversal error for {bad:?}"
            );
        }

        let dir = tempfile::tempdir().unwrap();
        // Missing resources returns default safely.
        let missing = CourseConfig::load_or_default(dir.path(), "Physics").unwrap();
        assert_eq!(missing, CourseConfig::default());

        let resources = dir.path().join("resources");
        fs::create_dir_all(&resources).unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("course.json"), r#"{"title":"Escaped"}"#).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&outside, resources.join("Escaped")).unwrap();
            assert!(CourseConfig::load_or_default(dir.path(), "Escaped").is_err());

            fs::create_dir_all(resources.join("LinkedFile")).unwrap();
            symlink(
                outside.join("course.json"),
                resources.join("LinkedFile/course.json"),
            )
            .unwrap();
            assert!(CourseConfig::load_or_default(dir.path(), "LinkedFile").is_err());
        }
    }

    #[test]
    fn save_roundtrip_preserves_metadata_and_rejects_invalid_plan() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = CourseConfig {
            title: Some("YDS".into()),
            persona: Some("Helpful coach".into()),
            exam: Some(ExamConfig {
                question_count: 80,
                choices_count: 5,
                categories: vec![CategoryWeight {
                    name: "Grammar".into(),
                    count: 10,
                }],
                rules: Some("YDS rules".into()),
            }),
            plan: vec![DayConfig {
                day: 1,
                title: "Tenses".into(),
                tasks: vec![TaskConfig {
                    command: "/anlat Tenses".into(),
                    label: "Ders: Zamanlar".into(),
                }],
            }],
        };
        cfg.save(dir.path(), "yds").unwrap();
        let loaded = CourseConfig::load_or_default(dir.path(), "yds").unwrap();
        assert_eq!(loaded, cfg);

        cfg.plan.push(DayConfig {
            day: 99,
            title: "Bad".into(),
            tasks: vec![],
        });
        assert!(cfg.save(dir.path(), "yds").is_err());
        assert!(
            CourseConfig::default()
                .save(dir.path(), "../escape")
                .is_err()
        );
    }
}
