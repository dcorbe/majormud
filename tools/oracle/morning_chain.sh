#!/usr/bin/env bash
# Morning chain: the Bard charm expedition (E4/E5/E6) against kobolds,
# which the a3/a4 blocks found plentiful 05:30-07:30. Runs after the
# overnight chain's blocks are long done.
set -u
cd /home/daniel/bbs/tools/oracle
LOG=/home/daniel/bbs/re/oracle/morning_chain.log
exec >>"$LOG" 2>&1

say() { echo "$(date +%F\ %T) $*"; }

now=$(date +%s)
target=$(date -d "05:45" +%s)
[ "$target" -le "$now" ] && target=$(date -d "tomorrow 05:45" +%s)
say "sleeping $((target - now))s until 05:45"
sleep $((target - now))

say "=== charm lifecycle (Bard vs kobolds) start ==="
python3 oracle_charm_lifecycle.py Bard test123
say "=== charm lifecycle done (exit $?) ==="
say "morning chain complete"
