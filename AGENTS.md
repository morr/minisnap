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
that domain's skill in the same change — the skill is what the next session
reads instead of the code, so a description left standing after the code
stopped obeying it is worse than none. Skills are maps with addresses into
the code, not a copy of it — verify against the actual source before editing.
What belongs in a skill is what orients a future session: mechanisms, module
boundaries, code-level invariants, gotchas. A skill carries what the code
itself won't tell you — mechanisms, invariants, gotchas, canon, and decision
rules; the repository's code is the working example. Nothing that a grep of
the repository reproduces gets into a skill: values of constants and
defaults, enumerations of helpers and signatures, flag and parameter tables,
code examples duplicating the files they reference. The test applies to every
line whenever a skill is written or updated: a line derivable from reading
the code is a candidate for deletion. Normative product behavior —
hotkey values and defaults, menu composition, user-facing scenarios — lives
in `openspec/specs/`; a skill links the capability as its reference instead
of restating it. The questions split three ways: the spec answers what the
product must do, SKILL.md answers where in the code and why it is built
this way, and the skill's `references/` carry the mechanics a task loads
on demand.

## Working with a running app

Talking to the running app: use the `minisnap` MCP server (workspace
`.zcode/config.json`, binary `src/bin/minisnap-mcp.rs`) — tools `instances`,
`state`, `screenshot`, `action`, `editor`, `log`. They drive the app's own
code paths over its built-in HTTP agent server: window screenshots without
Space/color-profile distortions, editor operations without synthetic mouse
events, queryable document and window state. Do not build ad-hoc
Swift/CGEvent/screencapture scripts for what the MCP tools already do;
AppleScript and cursor emulation are the last resort, not the default. If
the binary is missing, `cargo build --release` builds it (no bundle step
needed for the bridge itself). The agent server listens on
`127.0.0.1:15790` by default (`MINISNAP_AGENT_PORT` overrides); every
response carries the instance identity, so two running checkouts cannot be
confused.

Rebuilding for a live run: the runnable app is the `.app` bundle, and a
plain `cargo build --release` does not produce it — it only refreshes the
bare binary `target/release/minisnap`, while `target/release/minisnap.app`
keeps whichever build was bundled last. Launching the bundle after a plain
cargo build silently runs a stale app (a live run on 2026-11-02 spent the
whole session testing hours-old code because of exactly this). Rebuild the
bundle with `scripts/build_app.sh release` (debug: `scripts/build_app.sh
debug`) — it runs cargo, repacks the bundle and re-signs it with the stable
identity, so the Screen Recording grant survives the rebuild.

When a session drives a running minisnap — relaunching it after a rebuild,
checking the editor window or overlay, any live interaction — hide the
utility's windows immediately after each such action, before starting the
next one. Hiding means closing the window (closing the editor/prefs window
only hides it, the process stays in the tray), not quitting the app.
Exception: windows the user explicitly opened themselves before the action —
leave those as the user left them. The editor window and the overlay float
above everything and ignore app deactivation, so a window left over from the
previous step steals focus and shows up in the next verification screenshot.
If the work happens in a worktree and is finished, quit the minisnap
instance launched from that worktree — closing its windows only hides it
into the tray, so a leftover worktree build would keep running alongside
the main checkout's app.

Telling instances apart. Minisnap has no single-instance guard, and every
instance looks the same on screen — the same tray icon and menu, hidden
editor titles, the same `Preferences` window, the same shared config — so
the only reliable identity of a running instance is its executable path.
Before touching a running minisnap, enumerate it with
`ps -axo pid,comm | grep minisnap` and map each PID to its checkout: a
path under the repo root is the main checkout's build, under
`.worktrees/<branch>` is that worktree's. After launching an instance,
re-run the check and record which PID you are driving; quit instances only
by such a matched PID, never by name. Do not attribute windows or hotkey
presses by eye: global hotkeys belong to whichever instance registered the
combination first, so with two instances running the hotkey drives the
older one — the user's main instance, not the build you just launched.
While driving a worktree instance keep it the only running one: quit the
main-checkout instance first if it is up, and bring it back afterwards
(`open /Users/morr/develop/minisnap/target/release/minisnap.app`).

Emulation vs. the user's own activity. When a session emulates mouse and
window interaction with the running app and can't get through because the
user is actively using the PC, stop retrying and ask the user: either keep
attempting the emulation, or the user performs the actions themselves and
reports the results.

## OpenSpec workflow

This project is developed through OpenSpec: every feature starts as a change
in `openspec/changes/<change-name>/` (proposal, design, capability specs,
tasks) and is implemented task by task from that change's `tasks.md`.

- Before planning or implementing anything, read `openspec/config.yaml` — it
  carries the schema and mandatory context (artifact language, platform,
  stack conventions).
- Slash commands: `/opsx:propose` a new change, `/opsx:apply` to implement
  its tasks, `/opsx:archive` when the change is done.
- `/opsx:archive` also means merge and worktree cleanup: when a change was
  implemented in a worktree branch, archiving it includes merging that branch
  into master (a fast-forward is enough — the branch grows from master) and
  removing the worktree (`git worktree remove .worktrees/<branch>`; the
  branch ref stays).
- During apply, work strictly from the change's task list; if a task needs
  work beyond what its spec describes, surface it instead of silently
  widening the change.
- `/opsx:apply` ends with a domain-skills pass: over the change's whole
  diff, for every domain skill whose modules the change touched, either
  update the skill or state in the recap that its mechanism didn't change —
  the verdict is owed exactly when it feels unnecessary. Into the skill goes
  only what orients a future session (a mechanism, a boundary, a gotcha);
  normative behavior belongs in the change's specs.

## Worktrees

The project has no worktree helper scripts — plain git mechanics. A worktree
lives inside the project at `.worktrees/<branch>` (the directory is already
in `.gitignore`). Create the branch and the worktree in one step —
`git worktree add -b <branch> .worktrees/<branch> [base]` — and remove the
worktree with `git worktree remove .worktrees/<branch>`; the branch ref
itself stays. A minisnap built and run from a worktree follows the
instance rules in «Working with a running app» — identify it by executable
path, never by icon or window look.
