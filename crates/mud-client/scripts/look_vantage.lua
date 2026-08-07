-- look_vantage: what does `look <direction>` actually print, and does it
-- move you?
--
-- The client used to read ANY room block as "this is where I am now".
-- `Navigator::localize` searches the current room plus every neighbour by
-- name, so if a directional look prints the NEIGHBOUR's block, the
-- client's tracked position steps one room in the direction looked --
-- without the character moving at all.
--
-- No capture in this repo contains a directional look (3553 bare `look`s,
-- zero directional ones) because the client's automation never sends one;
-- only an operator does, by hand. This probe closes that gap.
--
-- What each section answers:
--   bare_look         the baseline: room name + the exits actually listed
--   look_<dir>        a look down a REAL exit -- full room block, or not?
--   bare_look_after   did we move? if the name matches bare_look, no
--   look_noexit_<dir> the refusal wording for a direction with no exit,
--                     which the correlator's reply grammar needs
--
-- Run:
--   mmc run crates/mud-client/scripts/look_vantage.lua \
--       --profile ~/.config/mmc/salad.toml --capture re/oracle/look_vantage
--
-- The capture carries the account password in the TX direction. It stays
-- out of git; read the *_sections.json, which holds only what is saved
-- below and never the login.

local outcome = mud.login()
mud.log("login outcome: " .. tostring(outcome))
if outcome ~= "ingame" then
  mud.log("ABORT: wanted an existing character, got '" .. tostring(outcome) .. "'")
  return
end

-- Settle after login before measuring anything.
mud.sleep(2)

-- 1. Baseline.
local m = mud.mark()
mud.send("look")
mud.expect("Obvious exits:")
mud.sleep(1)
local base = mud.since(m)
mud.save_section("bare_look", base)

local start_room = mud.room_name()
mud.log("start room: " .. tostring(start_room))

local exits = string.match(base, "Obvious exits:%s*([^\r\n]*)") or ""
mud.log("exits listed: " .. exits)

-- 2. A look down a REAL exit. If this prints a full block it will carry
-- an "Obvious exits:" line of its own; if it prints something shorter,
-- the expect times out and pcall keeps the probe alive to record that.
local dir = string.match(exits, "(%a+)")
if dir then
  m = mud.mark()
  mud.send("look " .. dir)
  local got_block = pcall(function() mud.expect("Obvious exits:", 6) end)
  mud.sleep(1)
  mud.save_section("look_" .. dir, mud.since(m))
  mud.log("look " .. dir .. " -> full room block: " .. tostring(got_block))
else
  mud.log("no usable exit parsed from: " .. exits)
end

-- 3. THE control. A look moves nobody, so this must report the same room
-- as bare_look. If it does not, the board moved us and the whole premise
-- is wrong.
m = mud.mark()
mud.send("look")
mud.expect("Obvious exits:")
mud.sleep(1)
mud.save_section("bare_look_after", mud.since(m))
local end_room = mud.room_name()
mud.log("room after directional look: " .. tostring(end_room))
mud.log("MOVED? " .. tostring(start_room ~= end_room))

-- 4. The refusal wording for a direction with no exit. The correlator
-- needs this to retire a LookDir that answered with a refusal rather than
-- a block -- and _cmd_look's wording is documented as differing from
-- _move_user's ("The door is closed in that direction!" vs "There is a
-- closed door in that direction!").
local missing
for _, d in ipairs({ "north", "south", "east", "west", "up", "down" }) do
  if not string.find(exits, d, 1, true) then
    missing = d
    break
  end
end

if missing then
  m = mud.mark()
  mud.send("look " .. missing)
  mud.sleep(3)
  mud.save_section("look_noexit_" .. missing, mud.since(m))
  mud.log("looked " .. missing .. " (no exit listed that way)")
else
  mud.log("every direction is an exit here; no refusal captured")
end

mud.log("look_vantage complete")
