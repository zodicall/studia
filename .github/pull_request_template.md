## Summary

Describe what changed and why.

## Checklist

- [ ] Updated relevant specs in `docs/specs/` and added/updated `spec_*` tests if behavior changed
- [ ] Updated both English and Turkish strings/docs (`README.md`, `README.tr.md`, `src/i18n.rs`, `src/bot.rs`, `src/tui/`)
- [ ] Verified locally:
  - `cargo fmt --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test --locked`
