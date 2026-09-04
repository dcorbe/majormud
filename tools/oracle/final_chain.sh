#!/usr/bin/env bash
# The dig's final field chain (2026-07-30): everything the board taught
# us, applied at once. The world is only populated ~02:00-08:00; both
# remaining measurements run inside the window on separate accounts.
#   - E3 engage-lock: Oracle vs kobold slaves (aggression 10) with the
#     zero-damage flurry of blades (P2's premise on a DR-0 target).
#   - Charm lifecycle take-3: Bard vs kobold slaves (charmlvl 1),
#     arrival-only follow detection, mortal recovery.
set -u
cd "$(dirname "$0")"
LOG=../../re/oracle/final_chain.log
exec >>"$LOG" 2>&1

say() { echo "$(date +%F\ %T) $*"; }

now=$(date +%s)
target=$(date -d "02:30" +%s)
[ "$target" -le "$now" ] && target=$(date -d "tomorrow 02:30" +%s)
say "sleeping $((target - now))s until 02:30"
sleep $((target - now))

for attempt in 1 2 3; do
    if ~/.local/bin/board-safe-to-restart; then
        say "gate clear"
        break
    fi
    if ! ss -tn | grep 2327 | grep -vq LISTEN; then
        say "gate self-blocked only; proceeding"
        break
    fi
    say "live connection on the board; waiting 10 min"
    sleep 600
done

pid=$(pgrep -f "MBBSEmu -M WCCMMUD" | head -1)
if [ -n "${pid:-}" ]; then
    kill -INT "$pid"
    while kill -0 "$pid" 2>/dev/null; do sleep 1; done
    say "board stopped"
fi
tmux -L mbbs kill-session -t board 2>/dev/null || true
tmux -L mbbs new-session -d -s board \
    "cd ~/mbbsemu && ./MBBSEmu -M WCCMMUD -P wccmmud/ 2>&1 | tee -a ~/mbbsemu/board-console.log"
until ss -tln | grep -q 2327; do sleep 2; done
say "board up; settling 60s"
sleep 60

say "=== E3 (slave) and charm take-3 in parallel ==="
E3_TARGET=slave python3 oracle_engage_lock.py &
E3_PID=$!
sleep 30    # stagger the logins
python3 oracle_charm_lifecycle.py Bard test123 &
CHARM_PID=$!
wait $E3_PID;   say "E3 done (exit $?)"
wait $CHARM_PID; say "charm done (exit $?)"
say "final chain complete"
