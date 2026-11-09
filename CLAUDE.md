# mac-anal

## Stack and layout

Rust and egui (eframe), a macOS app. The code lives in `app/`, a Cargo project; the version is `version` in `app/Cargo.toml`. Install Rust with rustup (`brew install rustup`), not Homebrew's `rust`.

Run the commands from `app/`:

```bash
cargo run            # build and run
cargo test           # unit tests, fast and offline
cargo test -- --ignored --nocapture   # slow tests that reach the network or external tools
cargo clippy         # lints, no warning allowed
```

Every new topic goes through the `topic` skill (`.claude/skills/topic/SKILL.md`): issue, worktree, tests, PR.

## Work items (GitHub)

Work items are GitHub issues of `C2-POLY/mac-anal`, tracked in the organisation's project **mac-anal** (<https://github.com/orgs/C2-POLY/projects/6>, number 6). `gh` must be signed in with the `project` scope (`gh auth refresh -s project`).

Issues are written in French and kept short:

- Title: `<Zone> : <ce qu'il apporte>`.
- Body: `**En tant que** …, **je veux** …, **afin de** …`, then `**Critères d'acceptation**` with 3 to 5 checkboxes, then `**Livré** : \`vX.Y.Z\`` once delivered.
- Type: the organisation's issue types, `Feature` (a capability, a parent issue when it is split), `Task` (an improvement, tooling, docs) or `Bug`. Split work is sub-issues; "B cannot start before A" is a `blocked by` relation, not a sentence.
- Labels: one `zone:` label (`zone:outillage` for tooling and docs; add the others as the app grows), and once delivered one version label (`v0.1.0`). Create a missing label rather than skipping it.
- Assignee: the person who did the work.
- Status on the project: `Backlog` (defined), `Ready` (ready to take), `In progress` (its worktree exists), `In review` (PR open), `To test` (merged, a criterion still untested by hand), `Done` (tested by the dev and merged).
- Every delivered change has an issue, including small ones only visible in `git log`.
- The PR closes its issue (`Closes #12` in its body).

Project ids for `gh project item-edit`: project `PVT_kwDODd-WqM4Bq3j1`, Status field `PVTSSF_lADODd-WqM4Bq3j1zhpMuhk`, options Backlog `94556cbe`, Ready `dd14524d`, In progress `dffc098d`, In review `4cb35a50`, To test `7101397e`, Done `252476c2`.

## Versioning

- The version lives only in `app/Cargo.toml` (`version`), as `MAJOR.MINOR.PATCH`.
- One working day is one minor version (0.x while the app is not distributed). Bump it in the first commit of the day that changes something visible in the app, inside that commit. A day that only fixes bugs bumps the patch instead. Commits that only touch docs or tooling do not bump it.
- Never put the version number in a commit message.
- After each merge into `main`, move the current version's tag to `main`. Tags only ever point at `main`:

```bash
git fetch --tags
git tag -f v0.1.0 origin/main && git push -f origin v0.1.0
```

## Repository

- Public repository: never commit secrets, `.env` files, keystores or certificates. `.gitignore` excludes them.
- Dual-licensed under MIT or Apache-2.0, like the other C2-POLY repositories.

## Commits and language

Commit messages, PR titles and code are in English and follow Conventional Commits (`feat(<zone>): …`, `fix(<zone>): …`). Issues and talk with the dev are in French. No AI attribution in commits, PRs or issues.
