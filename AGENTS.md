# AGENTS.md

Working notes for AI agents (and humans) on kotor2-rs.

## Agent policy

- The orchestrating session delegates implementation, verification,
  exploration and web searches to subagents on Haiku 5.5 (`model: haiku`).
  No forks: a fork inherits the parent model and cannot be overridden.
- Run 5 or more Haiku agents in parallel when the work splits naturally into
  independent parts. Do not split for its own sake.
- Each new agent costs about 40k tokens of harness context. Do not spawn one
  for a single-file read or a one-file write; do that inline or fold it into
  a larger agent task.
- Haiku 5.5 handles long-horizon work well (comparable to Sonnet 5 / Opus
  4.8, much faster and cheaper). Prefer fewer agents with whole, end-to-end
  tasks over many tiny ones.
- Agents share one working tree. Change shared files only with small in-place
  edits after re-reading them; never rewrite a whole existing file or restore
  one from a copy (that silently drops other agents' concurrent edits). Keep
  every saved state compiling.
- Only the orchestrating session runs builds and tests (cargo build, check,
  test, clippy, k2world harness runs and the game). Agents edit and read
  (data lookups with the already built k2probe are fine); when they need a
  build or test result they ask the orchestrator, naming the exact command.
- Only the orchestrating session touches the git index: it stages reviewed
  changes. Agents never run git add, rm, mv, stash, restore, reset,
  checkout, commit or any other command that writes the index or HEAD.

## Layout

- The repo sits inside the Steam game folder; the install root is `..`
  (data in `../steamassets`). Never write into the game folder outside this
  repo.
- `crates/k2-res`: install discovery and resource resolution.
- `crates/k2-mdl`: binary MDL/MDX reader vendored from rakata, K2-aware.
- `crates/k2probe`: CLI to list, extract and survey resources.
- `docs/notes/`: what we learned about formats and engine behaviour. This is
  the shareable output of research; keep it current.

## Rules

- Never commit game data, extracted assets, decompiled code or RE databases.
  The `.gitignore` is a whitelist; add new tracked file kinds deliberately.
- Scratch extraction goes to `/tmp/k2research/` or another dir outside the
  repo.
- Prefer documented formats and existing open-source readers over
  disassembly. Do not decompile the game binary.
- Credit what we learn from; keep upstream license headers on vendored code.
- Verify every format change with `k2probe survey` against the real install
  before calling it done.
- Tests live in their own files, never inline in a source file. Tests of the
  public API go in `crates/<crate>/tests/`. Unit tests that need private items
  go in a sibling file `src/<module>/tests.rs` (for `lib.rs`, `main.rs` or
  `mod.rs`: `tests.rs` next to it), declared in the module as
  `#[cfg(test)] mod tests;`. A module with several test modules keeps their
  names as files (`src/game/sit_tests.rs`, `#[cfg(test)] mod sit_tests;`).
- A test may skip only when the install is missing (K2_INSTALL unset and
  none found; a set K2_INSTALL that is not an install fails) or when an
  opt-in fixture variable it documents (e.g. K2_ICON_SAVE) is unset, and then
  says so with
  `let _ = writeln!(std::io::stderr(), "SKIP ...")` (libtest captures
  `eprintln!`, so it would be invisible). Any other failure to load (World::new,
  enter_module, a missing object) fails the test.
- ASCII only in code, comments and docs.

## Run

    cargo build --release
    target/release/k2probe info | modules | ls [MODULE] | cat NAME.EXT [MODULE] | survey [MODULE...]

## Restart workflow

To restart the running game where the player left off (after a build, or to
pick up new code): `tools/restart.sh --build` (or without `--build` to reuse
the snapshot). It saves slot `restart` through the control channel, quits
the game, snapshots a green build to `/tmp/k2research/play/kotor2` and
relaunches with `--continue`; the log is `/tmp/k2research/play/session.log`.
Manual: `echo save >> ~/.local/share/kotor2-rs/ctl/cmd`, then read `ack`
in that dir. Details and the test overrides are in `docs/notes/saves.md`.
Never kill a game process you did not start; the script only signals the pid
in the control dir's `pid` file.

## Remote control (MCP)

An MCP server (`crates/k2-mcp`, binary `k2-mcp`) builds the game, runs it in a
window and plays it: `game_start`, `game_status`, `game_stop`, `game_restart`,
screenshots, pick and project (what is under the pointer, where an object is),
keyboard and mouse input (injected into the game, never into the desktop), logs
and traces, and the semantic verbs of the control API in `k2-world`
(`control.rs`). The game serves the control socket with `--control`. Design,
tools, launch environment, safety and the live checks still to run are in
`docs/notes/mcp-control.md`.

The server is a thin supervisor with no game logic; game logic lives in the
game process. MCP play is windowed only: there is no headless mode through it.
Games started through it use only their own folders under
`/tmp/k2research/mcp` (saves, settings, control socket, shots, traces, logs).
The server refuses game flags and environment that would change that
(allowlist in `crates/k2-mcp/src/policy.rs`). The player's saves are never
touched, and API saves use `mcp-` slots only. `game_start` takes `start`:
`menu`, `new`, `chargen`, `continue`, `load:<slot>` or `module:<name>`.
Screenshot and trace files stay under the run's `shots/` and `traces/`; an
existing file is never overwritten. One goal (move_to, interact, ...) runs at a
time; keys, pointer input and pick are refused while it runs, and nothing is
queued. A goal whose game time stands still for 20 s (paused, movie, loading) is
stopped with the reason. One server per run root (`server.lock`). The debug_*
verbs are listed only when the server runs with `K2_MCP_DEBUG=1`. Restart builds
first, then saves, then relaunches. Signals to the server (SIGTERM, SIGINT,
SIGHUP) stop its game and any build first. Start a game through it only when the
user asks.

## Test windows

Normal play opens a window titled `kotor2-rs` with class / app_id `kotor2`;
`--screenshot` runs and any run with `K2_TEST_WINDOW=1` use title
`kotor2-rs-test` and class `kotor2-test`. The user's Hyprland rules (title
and class) float every one of them at 800x600 on workspace 4 silent. If that
workspace is not visible, Hyprland delivers no frames and a `--screenshot`
run hangs, so show workspace 4 (or ask the user) before running tests.
