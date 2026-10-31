# minisnap

macOS screenshot utility (region / window / full-screen capture, annotation,
copy to clipboard / save / drag-out) — a lightweight Monosnap analog.
Rust with objc2 crates (AppKit, ScreenCaptureKit, CoreGraphics, CoreText),
no external UI framework.

## Domain skills

Before exploring the code for a task, load the matching domain skill from
`.zcode/skills/`:

- `minisnap-architecture` — module map, end-to-end flow, cross-domain
  conventions (coordinate spaces, CGImage, objc2, the T0 freeze contract);
  the entry point for orientation and OpenSpec change planning;
- `minisnap-shell` — app lifecycle, tray, config, prefs, hotkeys, permissions;
- `minisnap-capture` — ScreenCaptureKit T0 snapshot and the selection overlay;
- `minisnap-output` — clipboard, PNG saving, retention, shot listing;
- `minisnap-editor` — editor window, annotation elements, rendering, edit
  sessions and undo.

A change that alters the behavior or structure of a domain's modules updates
that domain's skill in the same change. Skills are maps with addresses into
the code, not a copy of it — verify against the actual source before editing.

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

## Worktrees

The project has no worktree helper scripts — plain git mechanics. A worktree
lives inside the project at `.worktrees/<branch>` (the directory is already
in `.gitignore`). Create the branch and the worktree in one step —
`git worktree add -b <branch> .worktrees/<branch> [base]` — and remove the
worktree with `git worktree remove .worktrees/<branch>`; the branch ref
itself stays.
