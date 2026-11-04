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

### The minisnap MCP — the way to talk to the running app

The `minisnap` MCP server (workspace `.zcode/config.json`, binary
`src/bin/minisnap-mcp.rs`) is the control channel to a running minisnap —
the analog of BRP: it tells the app what to do and reads its state, over a
built-in HTTP agent server on `127.0.0.1:15790` (override:
`MINISNAP_AGENT_PORT`). Every request executes the same code path the tray
menu, hotkeys and mouse handlers run — no AppleScript, no synthetic mouse
events, no `screencapture`.

**When to use it.** Any time a task needs truth about the running app or
needs to drive it:

- verifying a change live after a rebuild (the overlay opens, the tool
  activates, the output file lands);
- asking what the app is doing right now (is the overlay up? which tool is
  active? what is selected? can it undo?);
- screenshots of the editor document or window for visual checks;
- driving the editor programmatically (add a callout, change a width, undo)
  instead of moving the user's cursor;
- chaining several sequential actions into one call — `batch`, preferred
  over repeated `action`/`editor` calls;
- reading the app's log without holding its launch stderr.

Do not build ad-hoc Swift/CGEvent/screencapture/osascript tooling for what
these tools already do — that class of scripts is how past sessions lost
hours to silent click failures, Space-mismatched captures and color-profile
confusion. Pixel-probing a screenshot to learn app state is archaeology;
`state` is the source of truth. Two honest limits: the MCP captures only
minisnap's own windows and document — for what the user's whole screen
shows, `screencapture` + vision is still the tool; and it drives the app's
own behaviors, not arbitrary macOS UI.

**The tools.**

- `instances` — live instances (checkout label, pid, port). Call it first.
- `state` — full snapshot: open windows with frames, `overlay_active`,
  `prefs_open`, `activation_policy` (`regular` while the editor window is
  visible — minisnap is in cmd+tab; `accessory` otherwise), the editor
  document (shot path, elements in journal shape
  with geometry, selection, active tool, color, stroke width, undo/redo,
  leafing availability) and the config. `editor: null` means the editor
  window was never opened this run; a hidden-but-opened editor still
  reports its document.
- `screenshot {target, path?}` — `target: "composite"` renders the editor
  document (pixel-exact code colors — no color-profile conversion);
  `target: "window"` draws the editor window content (toolbar + canvas)
  and works while the window is occluded or on another Space. Returns the
  image plus the PNG path.
- `action {action}` — app-level actions, serde enum shapes:
  `{"Capture":{"edit":true}}`, `{"OpenEditor":{"path":"…"}}`,
  `{"OverlaySelect":{"x":…,"y":…,"w":…,"h":…}}`, and the unit variants
  `OpenLatest`, `"CloseEditor"`, `"OverlayCancel"`, `"PrefsOpen"`,
  `"PrefsClose"`, `"Quit"`.
- `editor {op}` — editor operations:
  `{"SetTool":{"tool":"arrow"|"rect"|"line"|"pen"|"text"|"callout"|"blur"|"crop"|"resize"|null}}`,
  `{"SetColor":{"rgba":[r,g,b,a]}}`, `{"SetWidth":{"width":…}}`,
  `{"Command":{"AddElement":…}}` (any journal `Command` in journal shape),
  `"Undo"`, `"Redo"`, `{"Select":{"id":…}}`,
  `{"Page":{"direction":"prev"|"next"}}`.
- `batch {steps}` — several sequential `action`/`editor` steps in one
  call; the preferred way to run a chain. `steps` is an ordered list, each
  step an object with exactly one key — `{"action": …}` (an `AgentAction`
  shape) or `{"editor": …}` (an `AgentEditorOp` shape). Steps run in
  order; the answer reports each with its number and status. Step
  statuses (`rejected`, step errors) do not stop the chain — a transport
  error does, reporting what executed. Reads are not steps, and polling
  does not batch: `Capture` is `initiated`, so poll `state` between it
  and `OverlaySelect`.
- `log {offset?}` — the app's log ring buffer; the response carries the
  next offset to continue from.

**Semantics worth knowing.**

- Action/operation results carry a status: `done` (synchronous effect),
  `initiated` (async — only `Capture`: poll `state` until
  `overlay_active`), `rejected` (current state forbids it, with the
  reason), `noop` (nothing to do). A dead or misconfigured app is a short
  readable error, never a crash.
- The capture flow: `Capture` → poll `state` → `OverlaySelect` with the
  rect in the captured frame's pixel space (origin top-left) → the full
  output pipeline runs exactly as for a user drag: crop, shutter sound,
  clipboard overwrite, PNG in the save dir. Those are user-visible side
  effects — in tests, point the config's `save_dir` at a temp dir and
  restore it afterwards, and do not leave test shots in the real folder.
- `SetTool` sets exactly (`null` = viewing mode); it does not toggle like
  clicking an active tool button does. `SetColor`/`SetWidth` recolor and
  restyle the selected element, like the palette and slider.
- Closing (`CloseEditor`) hides the window; the editor object keeps the
  document, so `editor` operations and `composite` screenshots keep
  working on it — that is the way to edit without popping a window in the
  user's face. An error comes only when the editor window never opened.
- Every answer names the instance it came from (checkout label + pid).
  With several instances alive and no `instance` argument, the tools
  target the main checkout and mention the others; pass `"instance":
  "<checkout label or pid>"` to reach a specific one.
- `instances` listing nothing means: the app is not running, the server is
  disabled (`agent-server-enabled = false` in the config), or the port was
  taken at startup — the app's log has the explicit "cannot bind" line.
  Stale registry records self-heal: a pid that no longer matches the port
  is pruned on the next read.

Rebuilding the bridge itself needs no bundle step: `cargo build --release`
produces `target/release/minisnap-mcp` at the path the workspace config
registers. Rebuild it whenever `src/agent_proto.rs` or the bridge changed,
or the tools will disagree with the running app's protocol.

### Rebuilding the app for a live run

The runnable app is the `.app` bundle, and a plain `cargo build --release`
does not produce it — it only refreshes the bare binary
`target/release/minisnap`, while `target/release/minisnap.app` keeps
whichever build was bundled last. Launching the bundle after a plain cargo
build silently runs a stale app (a live run on 2026-11-02 spent the whole
session testing hours-old code because of exactly this). Rebuild the
bundle with `scripts/build_app.sh release` (debug:
`scripts/build_app.sh debug`) — it runs cargo, repacks the bundle and
re-signs it with the stable identity, so the Screen Recording grant
survives the rebuild. After rebuilding, quit the running instance (by its
matched PID) and relaunch the bundle — a running process keeps executing
the old binary, and `state` will happily describe it. Verify who you are
talking to via `instances` (new pid) or the exe path in any response.

### Window hygiene while driving the app

When a session drives a running minisnap — relaunching it after a rebuild,
checking the editor window or overlay, any live interaction — hide the
utility's windows immediately after each such action, before starting the
next one (`CloseEditor` / `PrefsClose` via MCP do exactly this). Hiding
means closing the window (closing the editor/prefs window only hides it,
the process stays in the tray), not quitting the app. Exception: windows
the user explicitly opened themselves before the action — leave those as
the user left them. The editor window and the overlay float above
everything and ignore app deactivation, so a window left over from the
previous step steals focus and shows up in the next verification
screenshot. If the work happens in a worktree and is finished, quit the
minisnap instance launched from that worktree — closing its windows only
hides it into the tray, so a leftover worktree build would keep running
alongside the main checkout's app.

### Telling instances apart

Minisnap has no single-instance guard, and while the tray icon is shared,
each instance names its own checkout: the first, inert tray-menu item
reads `minisnap — <label>`, the `Preferences` window title is
`Preferences — <label>`, and at startup the process prints
`minisnap instance '<label>' from <exe path>` to stderr — the label is
the branch name for a worktree build, `main` otherwise. Treat these as
the cross-check; `ps -axo pid,comm | grep minisnap` stays the source of
last resort (under the repo root is the main checkout, under
`.worktrees/<branch>` is that worktree), and the `instances` MCP tool is
the first-line enumeration (checkout label, pid, port, exe). Quit
instances only by a matched PID, never by name. Do not attribute hotkey
presses by eye: global hotkeys belong to whichever instance registered
the combination first, so with two instances running the hotkey drives
the older one — the user's main instance, not the build you just
launched. While driving a worktree
instance keep it the only running one (quit the main-checkout instance
first, bring it back afterwards with `open
/Users/morr/develop/minisnap/target/release/minisnap.app`) — that rule is
about hotkeys and screen capture, not about the MCP: state queries and
screenshots over HTTP are pid-addressed and safe with several instances
up.

### Emulation vs. the user's own activity

When a session emulates mouse and window interaction with the running app
and can't get through because the user is actively using the PC, stop
retrying and ask the user: either keep attempting the emulation, or the
user performs the actions themselves and reports the results. Reaching for
emulation at all is a smell — most of those tasks are expressible as MCP
`action`/`editor` calls that need no cursor and no focus.

Deeper references: the mechanism and gotchas of the agent server live in
the `minisnap-shell` skill; the normative endpoint and MCP-tool contract
is the `agent-control` spec (`openspec/specs/agent-control/spec.md`).

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

Two sessions on one checkout is a lost-work race: commits interleave, and
a tree-wide revert (`git restore .` / `git checkout .`) in a shared tree
silently wipes the other session's uncommitted changes — on 2026-11-03 a
verified text-matrix fix was erased this way between its build and its
commit and had to be re-landed (213f6ac). When another session may be
working in this checkout, take a worktree instead, and in a shared tree
never run tree-wide reverts or `git add -A` blind — check `git status`
first and stage only your own files.
