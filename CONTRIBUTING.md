# Contributing to `studia`

Thank you for helping improve `studia`! This project uses **Spec-Driven Development** and follows a minimalist, zero-bloat Rust architecture.

## Development Setup

- **Rust toolchain**: `1.89` or newer (edition `2024`, matches [`Cargo.toml`](Cargo.toml)).
- **System tools**: `curl` (for network integration tests if any; unit tests run fully offline) and optional `pdftotext` for PDF extraction tests.
- Keep `Cargo.lock` unchanged unless a dependency addition is genuinely unavoidable — prefer standard library solutions first.

## Spec-Driven Workflow

1. Read the relevant specification in [`docs/specs/`](docs/specs/) before modifying behavior:
   - [`01-config-i18n-cli.md`](docs/specs/01-config-i18n-cli.md)
   - [`02-course-plan-sources.md`](docs/specs/02-course-plan-sources.md)
   - [`03-ai-providers.md`](docs/specs/03-ai-providers.md)
   - [`04-store-and-exams.md`](docs/specs/04-store-and-exams.md)
   - [`05-bot-and-tui.md`](docs/specs/05-bot-and-tui.md)
   - [`06-release-hygiene-ci.md`](docs/specs/06-release-hygiene-ci.md)
   - [`07-security-and-stability.md`](docs/specs/07-security-and-stability.md)
2. Add or update targeted `spec_*` unit tests verifying new requirements alongside your implementation.

## Required Checks Before Opening a PR

Run these locally and ensure all three succeed cleanly:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

If you work in an offline environment, `cargo test --offline --locked` runs the complete offline test suite.

## Code & Localization Checklist

- **Bilingual parity**: Update both English (`Lang::En`) and Turkish (`Lang::Tr`) strings in `src/i18n.rs`, `src/bot.rs`, `src/tui/app.rs`, and `src/tui/ui.rs`, as well as both [`README.md`](README.md) and [`README.tr.md`](README.tr.md) for user-facing changes.
- **Course-agnostic defaults**: Never hardcode exam names, publishers, or domain-specific topics into general defaults or UI chrome; use `CourseConfig` fields and templates instead.
- **Safety & Stability**:
  - Zero `.unwrap()`, `.expect()`, or `todo!()` in non-`#[cfg(test)]` production paths.
  - Zero `eprintln!` inside TUI code (`src/tui.rs`, `src/tui/`).
  - Never commit `.env`, databases (`*.db`), private study materials (`kaynaklar/`, `data/`, `anki/`), personal filesystem paths, or tokens.

## Licensing & Copyright Policy

- **Inbound = Outbound (MIT)**: By submitting a pull request or other contribution to `studia`, you agree that your contribution is licensed under the project's [MIT License](LICENSE).
- **No Copyrighted Third-Party Materials**: Do not submit copyrighted third-party exam questions, textbooks, answer keys, or proprietary course materials in PRs, issues, or example templates. All sample courses and test fixtures must be original or permissively licensed.
