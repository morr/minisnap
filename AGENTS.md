# minisnap

macOS screenshot utility (region / window / full-screen capture, annotation,
copy to clipboard / save / drag-out) — a lightweight Monosnap analog.
Greenfield Rust project.

## OpenSpec workflow

This project is developed through OpenSpec: every feature starts as a change
in `openspec/changes/<change-name>/` (proposal, design, capability specs,
tasks) and is implemented task by task from that change's `tasks.md`.

- Before planning or implementing anything, read `openspec/config.yaml` — it
  carries the schema and mandatory context (artifact language, platform,
  stack conventions).
- Slash commands: `/opsx:propose` a new change, `/opsx:apply` to implement
  its tasks, `/opsx:archive` when the change is done.
- During apply, work strictly from the change's task list; if a task needs
  work beyond what its spec describes, surface it instead of silently
  widening the change.

## Commits

One task = one commit. After finishing a task:

1. Mark its checkbox `- [x]` in the change's `tasks.md`.
2. `git add` the task's files together with `tasks.md` (the checkbox update
   belongs to the same commit) and commit immediately — no batching several
   tasks into one commit.
3. Follow Conventional Commits in English: `feat(capture): ...`,
   `fix(hotkeys): ...`; planning-only changes use `docs(openspec): ...`.
