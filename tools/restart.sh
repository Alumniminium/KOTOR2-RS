#!/usr/bin/env bash
# Restart the running kotor2 where the player left off.
#
#   restart.sh [--build] [--force] [-- GAME_ARGS...]
#
# 1. --build: cargo build --release first (the game keeps running); on a
#    green build the binary is snapshotted atomically to $K2_PLAY_BIN. A
#    failed build aborts before the game is touched.
# 2. Asks the running game (pid file in the control dir) to save the slot
#    "restart" and waits for the ack; on timeout the stale restart slot is
#    set aside so --continue falls back to the newest autosave/quicksave.
# 3. Quits it (control channel, then SIGTERM, then SIGKILL), only for the
#    pid in the pid file and only if that pid is a kotor2 process.
# 4. Relaunches detached with --continue and reports the load result:
#    "continue: PASS" (same state digest), a WARN "continue: loaded (digest
#    differs, engine changed)" after a new build (informational), or an
#    ERROR line if the slot could not be loaded.
#
# Environment overrides (also used for tests): K2_CTL_DIR, K2_SAVE_DIR,
# K2_PLAY_BIN, K2_PLAY_DIR, K2_SESSION_LOG, K2_REPO, K2_INSTALL_ROOT.
set -u

here=$(cd "$(dirname "$0")" && pwd)
if [ -n "${K2_REPO:-}" ]; then
  REPO=$K2_REPO
elif [ -f "$here/../Cargo.toml" ]; then
  REPO=$(cd "$here/.." && pwd)
else
  REPO="$HOME/.local/share/Steam/steamapps/common/Knights of the Old Republic II/kotor2-rs"
fi
INSTALL=${K2_INSTALL_ROOT:-$(dirname "$REPO")}
PLAY=${K2_PLAY_DIR:-/tmp/k2research/play}
BIN=${K2_PLAY_BIN:-$PLAY/kotor2}
DATA=${XDG_DATA_HOME:-$HOME/.local/share}/kotor2-rs
CTL=${K2_CTL_DIR:-$DATA/ctl}
SAVES=${K2_SAVE_DIR:-$DATA/saves}
LOG=${K2_SESSION_LOG:-$PLAY/session.log}

build=0
force=0
extra=()
while [ $# -gt 0 ]; do
  case $1 in
    --build) build=1 ;;
    --force) force=1 ;;
    --) shift; extra=("$@"); break ;;
    *) echo "restart: unknown option $1" >&2; exit 2 ;;
  esac
  shift
done

say() { echo "restart: $*"; }
alive() { [ -n "${1:-}" ] && [ -d "/proc/$1" ] && [ "$(cat "/proc/$1/comm" 2>/dev/null)" = "$(basename "$BIN" | cut -c1-15)" ]; }
strip() { sed 's/\x1b\[[0-9;]*m//g'; }

mkdir -p "$CTL" "$PLAY"

pid=$(cat "$CTL/pid" 2>/dev/null | tr -dc '0-9')
if ! alive "$pid"; then
  [ -n "$pid" ] && say "pid file names $pid, which is not a running $(basename "$BIN"); ignoring it"
  pid=
  # A copy of the game started before the control channel existed has no
  # pid file: never touch it, and do not start a second one beside it.
  foreign=
  for p in $(pgrep -x "$(basename "$BIN" | cut -c1-15)"); do
    exe=$(readlink "/proc/$p/exe" 2>/dev/null)
    case $exe in "$BIN" | "$BIN (deleted)") foreign=$p ;; esac
  done
  if [ -n "$foreign" ] && [ "$force" = 0 ]; then
    say "pid $foreign runs $BIN but has no pid file in $CTL; not touching it (use --force to launch anyway)"
    exit 3
  fi
fi

if [ "$build" = 1 ]; then
  say "building"
  out=$(cd "$REPO" && cargo build --release -p kotor2 2>&1)
  if [ $? -ne 0 ]; then
    echo "$out" | tail -n 15
    say "build failed; game left as it is"
    exit 4
  fi
  cp "$REPO/target/release/kotor2" "$BIN.new" && mv -f "$BIN.new" "$BIN" || { say "snapshot failed"; exit 4; }
  say "snapshotted $BIN"
fi
[ -x "$BIN" ] || { say "$BIN does not exist"; exit 4; }

saved=0
if [ -n "$pid" ]; then
  before=$(stat -c %Y "$SAVES/restart/save.bin" 2>/dev/null || echo 0)
  : > "$CTL/ack"
  echo save >> "$CTL/cmd"
  for _ in $(seq 1 75); do
    [ -s "$CTL/ack" ] && break
    alive "$pid" || break
    sleep 0.2
  done
  ack=$(cat "$CTL/ack" 2>/dev/null)
  say "ack: ${ack:-none}"
  case $ack in "ok save "*) saved=1 ;; esac

  if [ "$saved" = 1 ]; then
    : > "$CTL/ack"
    echo "quit now" >> "$CTL/cmd"
  fi
  for _ in $(seq 1 50); do alive "$pid" || break; sleep 0.2; done
  if alive "$pid"; then
    say "no clean exit; SIGTERM $pid"
    kill -TERM "$pid" 2>/dev/null
    for _ in $(seq 1 25); do alive "$pid" || break; sleep 0.2; done
  fi
  if alive "$pid"; then
    say "SIGKILL $pid"
    kill -KILL "$pid" 2>/dev/null
    sleep 0.5
  fi
  if [ "$saved" = 0 ]; then
    after=$(stat -c %Y "$SAVES/restart/save.bin" 2>/dev/null || echo 0)
    if [ -d "$SAVES/restart" ] && [ "$after" = "$before" ]; then
      mkdir -p "$SAVES/.deleted"
      mv "$SAVES/restart" "$SAVES/.deleted/restart-$(date +%s)"
      say "save failed; stale restart slot set aside, --continue will use the newest autosave/quicksave"
    fi
  fi
else
  say "no running game; launching with --continue"
fi

[ -f "$LOG" ] && mv -f "$LOG" "$LOG.prev"
cd "$PLAY" || exit 1
K2_CTL_DIR="$CTL" WAYLAND_DISPLAY= setsid nohup "$BIN" --install "$INSTALL" --continue "${extra[@]}" >"$LOG" 2>&1 < /dev/null &
newpid=$!
say "launched pid $newpid, log $LOG"

for _ in $(seq 1 300); do
  line=$(strip < "$LOG" | grep -m1 -E 'continue: (PASS|loaded|slot |no loadable)|ERROR.*saves: load ')
  [ -n "$line" ] && break
  kill -0 "$newpid" 2>/dev/null || { say "game exited early"; strip < "$LOG" | tail -n 5; exit 5; }
  sleep 0.2
done
say "${line:-no continue line within 60 s}"
