#!/usr/bin/env bash
# Slice-8 overnight chain (2026-07-29): the board's world is only fully
# alive 02:00-08:00 (bats nocturnal; populations diffuse with uptime).
# Sleep to 02:30, restart the board for fresh spawns (gated), then run
# b3 (the d=3 parry point) and E3 (engage lock vs night bats).
set -u
cd /home/daniel/bbs/tools/oracle
LOG=/home/daniel/bbs/re/oracle/overnight_chain.log
exec >>"$LOG" 2>&1

say() { echo "$(date +%F\ %T) $*"; }

# --- wait for the window ---------------------------------------------------
now=$(date +%s)
target=$(date -d "02:30" +%s)
[ "$target" -le "$now" ] && target=$(date -d "tomorrow 02:30" +%s)
say "sleeping $((target - now))s until 02:30"
sleep $((target - now))

# --- gated board restart ---------------------------------------------------
for attempt in 1 2 3; do
    if ~/.local/bin/board-safe-to-restart; then
        say "gate clear (attempt $attempt)"
        break
    fi
    # blocked only by our own recent files? no established connections = go
    if ! ss -tn | grep 2327 | grep -vq LISTEN; then
        say "gate blocked but zero live connections (attempt $attempt) — self-block, proceeding"
        break
    fi
    say "gate blocked by a live connection; waiting 10 min (attempt $attempt)"
    sleep 600
done

pid=$(pgrep -f "MBBSEmu -M WCCMMUD" | head -1)
if [ -n "${pid:-}" ]; then
    say "SIGINT board pid $pid"
    kill -INT "$pid"
    while kill -0 "$pid" 2>/dev/null; do sleep 1; done
    say "board stopped"
fi
tmux -L mbbs kill-session -t board 2>/dev/null || true
tmux -L mbbs new-session -d -s board \
    "cd ~/mbbsemu && ./MBBSEmu -M WCCMMUD -P wccmmud/ 2>&1 | tee -a ~/mbbsemu/board-console.log"
until ss -tln | grep -q 2327; do sleep 2; done
say "board relaunched, port up; settling 60s"
sleep 60

# --- b3: the d=3 parry point (malachite, light band, accuracy 29) ----------
say "=== b3 start ==="
python3 oracle_dodge_parry.py b3 400 90
say "=== b3 done (exit $?) ==="

# b3 wore the cursed malachite; E3 does not care about worn gear, so no
# death needed between blocks. The morning staging can shed it.

# --- E3: engage lock vs night bats ------------------------------------------
say "=== E3 start ==="
E3_TARGET=bat python3 oracle_engage_lock.py
say "=== E3 done (exit $?) ==="

say "chain complete"
