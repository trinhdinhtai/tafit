## Agent skills

### Domain docs

Single-context: one `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

ADRs in `docs/adr/` are written in Vietnamese (solo project). `GLOSSARY.md` definitions are in Vietnamese; term names stay in English.

## Git workflow

### Issue tracker

Issues live in GitHub Issues for trinhdinhtai/tafit (via the `gh` CLI). See `docs/agents/issue-tracker.md`.

### Triage labels

Default vocabulary: needs-triage, needs-info, ready-for-agent, ready-for-human, wontfix. See `docs/agents/triage-labels.md`.

### Branch per ticket

Before starting work on any ticket, create a new branch from an up-to-date `main`. Never commit ticket work directly on `main`. Name it `<type>/<issue#>-<short-slug>` (e.g. `feat/4-app-shell-base64`); drop the number when there is no issue (e.g. `chore/vietnamese-fonts`).

### Small commits

Commit ticket work as several small commits, one per logical step, so each can be reviewed on its own. Never put a whole ticket in one commit. Put new dependencies in their own `build:` commit. Every commit must stand alone: typecheck, lint and its tests pass. Fold review fixes into the commit they fix while the branch is unpushed; after that, add a small `fix:` commit.
