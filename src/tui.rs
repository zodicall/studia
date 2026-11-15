mod app;
mod modals;
mod ui;

use crate::{Result, config, i18n::Lang};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
};
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};

fn status_error(root: &Path, label: &str, err: &dyn std::fmt::Display) -> String {
    format!("{label}: {}", config::redact_error(root, err))
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        ratatui::restore();
    }
}

pub fn run_with(root: &Path, lang: Lang) -> Result<()> {
    let mut app = app::App::open_with(root, lang)?;
    // try_init installs ratatui's panic hook, which restores before reporting a panic.
    let _guard = TerminalGuard;
    let mut terminal = ratatui::try_init()?;
    let _ = execute!(io::stdout(), EnableMouseCapture);
    let mut refreshed = Instant::now();
    let result = (|| -> Result<()> {
        while !app.quit {
            if app.poll().is_err() {
                app.status = app
                    .lang
                    .pick(
                        "İşlem tamamlanamadı. Kaynakları ve AI sağlayıcı ayarını kontrol edin.",
                        "Task failed. Check your sources directory and AI provider setup.",
                    )
                    .into();
            }
            if refreshed.elapsed() >= Duration::from_secs(1) {
                if app.refresh().is_err() {
                    app.status = app
                        .lang
                        .pick("Kayıtlar yenilenemedi.", "Could not refresh records.")
                        .into();
                }
                refreshed = Instant::now();
            }
            app.tick();
            if let Err(error) = app.coach_tick() {
                app.status = status_error(root, app.lang.pick("Koç", "Coach"), &error);
            }
            terminal.draw(|frame| ui::draw(frame, &app))?;
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) => {
                        if let Err(error) = app.key(key) {
                            app.status = status_error(
                                root,
                                app.lang.pick("İşlem yapılamadı", "Action failed"),
                                &error,
                            );
                        }
                    }
                    Event::Mouse(mouse) => app.mouse(mouse),
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    // Close only this UI's session; never close a concurrent listener's session.
    app.stop_listener();
    let closed = app.close_session();
    result.and(closed)
}

#[cfg(test)]
mod tests {
    use super::status_error;
    use std::fs;

    #[test]
    fn status_error_redacts_secrets_and_workspace_path() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join(".env"),
            "OPENAI_API_KEY=sk-tui-secret-1234\n",
        )
        .unwrap();
        let raw = format!(
            "failed reading {} with sk-tui-secret-1234",
            root.path().join("resources/doc.md").display()
        );
        let formatted = status_error(root.path(), "Action failed", &raw);
        assert!(formatted.starts_with("Action failed: "));
        assert!(!formatted.contains("sk-tui-secret-1234"));
        assert!(formatted.contains("[REDACTED]"));
        assert!(!formatted.contains(&root.path().to_string_lossy().into_owned()));
        assert!(formatted.contains("<workspace>"));
    }
}
