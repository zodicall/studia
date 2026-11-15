mod app;
mod ui;

use crate::{Result, i18n::Lang};
use crossterm::event::{self, Event};
use std::{
    path::Path,
    time::{Duration, Instant},
};

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

pub fn run_with(root: &Path, lang: Lang) -> Result<()> {
    let mut app = app::App::open_with(root, lang)?;
    // try_init installs ratatui's panic hook, which restores before reporting a panic.
    let _guard = TerminalGuard;
    let mut terminal = ratatui::try_init()?;
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
                app.status = format!("{}: {error}", app.lang.pick("Koç", "Coach"));
            }
            terminal.draw(|frame| ui::draw(frame, &app))?;
            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
                && let Err(error) = app.key(key)
            {
                app.status = format!(
                    "{}: {error}",
                    app.lang.pick("İşlem yapılamadı", "Action failed")
                );
            }
        }
        Ok(())
    })();
    // Close only this UI's session; never close a concurrent listener's session.
    app.stop_listener();
    let closed = app.close_session();
    result.and(closed)
}
