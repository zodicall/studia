# SPEC-06: Public Release Hygiene, Documentation, Examples & CI/CD

- **Scope:** Repo root (`Cargo.toml`, `.gitignore`, `LICENSE`, `.env.example`, `README.md`, `README.tr.md`, `CONTRIBUTING.md`, `CHANGELOG.md`, `SECURITY.md`), `examples/`, `.github/workflows/`
- **Status:** Draft -> Implementation Contract

---

## 1. Repository Hygiene & Licensing (`REL-1..2`)

| ID | Requirement |
| :--- | :--- |
| `REL-1` | Ship only clean public project files in the repository. Personal/local workspace directories (`kaynaklar/`, `data/`, `.cargo/`, `tools/`) stay excluded via `.gitignore`. |
| `REL-2` | Configure `Cargo.toml` (`rust-version = "1.89"`, `license = "MIT"`, `description`, `repository`, `readme = "README.md"`, `keywords`, valid `crates.io` `categories = ["command-line-utilities"]`). Include standard MIT `LICENSE` (`2026`), `.gitattributes` (`* text=auto eol=lf`), `.gitignore` (ignoring `.env`, `.env.*`, `!.env.example`, `*.db`, `data/`, `kaynaklar/`, `anki/`, `target/`, `target_test/`, `.cargo/`, `tools/`), and clean `.env.example`. |

---

## 2. Public Documentation & Copyright-Free Examples (`REL-3`)

| ID | Requirement |
| :--- | :--- |
| `REL-3` | - `README.md` (English primary with Turkish switcher link at top) and `README.tr.md` (full Turkish mirror) covering: overview, quickstart, AI backend setup (`GEMINI_API_KEY`, OpenAI/Ollama, `agy`/`claude`/`codex`/`gemini`/`custom`), Telegram setup, custom `course.json`, CLI/TUI/Bot commands, `doctor` troubleshooting, and MIT license.<br>- `CONTRIBUTING.md` (dev setup, Spec-Driven check workflow `fmt` / `clippy` / `test --locked`, localization checklist), `CHANGELOG.md` (`v0.1.0`), `SECURITY.md` (threat boundary, local-first storage, responsible disclosure).<br>- `examples/general-course/course.json` and `examples/academic-english/course.json` clean, original, copyright-free templates validated directly by unit tests against `CourseConfig`. |

---

## 3. Hardened CI/CD & Clean Git History (`REL-4..5`)

| ID | Requirement |
| :--- | :--- |
| `REL-4` | - `.github/workflows/ci.yml` (`push` to `main`, `pull_request`): matrix `[ubuntu-latest, windows-latest, macos-latest]`, top-level `permissions: contents: read`, `actions/checkout` with `persist-credentials: false`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked`, `taiki-e/install-action@cargo-audit` + `cargo audit`, and static hygiene scan rejecting personal paths and committed `.env` / DB / material files.<br>- `.github/workflows/release.yml` (tags `v*`): top-level `permissions: contents: read`, job-scoped `permissions: contents: write` ONLY on release publishing, `cargo build --release --locked` across `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin` (`macos-15-intel`), `aarch64-apple-darwin` (`macos-latest`), packaging binary + docs + `examples/` + `.env.example` + SHA-256 checksums. |
| `REL-5` | Prepare clean orphan `main` branch locally free of historical private paths/logs/emails. Remote `git push` is NEVER run without explicit user instruction. |
