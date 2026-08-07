-- look_vantage2: is the directional-look block the NEIGHBOUR's, or the
-- current room's with extra prose?
--
-- Probe 1 answered from Slum Street, whose northern neighbour is ALSO
-- called "Slum Street" -- so the block it returned was consistent with
-- both readings. This walks until the room the character stands in and
-- the room it looks at have DIFFERENT names, which separates them.
--
-- Each step records: where we stand (bare look), what a directional look
-- returns, and where we stand afterwards. A look moves nobody, so the
-- third must always equal the first.

local outcome = mud.login()
if outcome ~= "ingame" then
  mud.log("ABORT: wanted an existing character, got '" .. tostring(outcome) .. "'")
  return
end
mud.sleep(2)

-- Walk a short way and, at each stop, look down every listed exit until
-- one returns a name different from the room we are standing in.
local function survey(tag)
  local m = mud.mark()
  mud.send("look")
  mud.expect("Obvious exits:")
  mud.sleep(1)
  local here_text = mud.since(m)
  local here_name = mud.room_name()
  mud.save_section(tag .. "_standing_in", here_text)

  local exits = string.match(here_text, "Obvious exits:%s*([^\r\n]*)") or ""
  mud.log(tag .. " standing in '" .. tostring(here_name) .. "' exits: " .. exits)

  for d in string.gmatch(exits, "(%a+)") do
    m = mud.mark()
    mud.send("look " .. d)
    pcall(function() mud.expect("Obvious exits:", 6) end)
    mud.sleep(1)
    local seen = mud.since(m)
    mud.save_section(tag .. "_look_" .. d, seen)

    -- The name on the block that came back, i.e. the first non-empty
    -- line after the echoed command.
    local looked = string.match(seen, "look %a+%s*\r?\n%s*([^\r\n]+)")
    mud.log(tag .. " look " .. d .. " -> '" .. tostring(looked) .. "'")

    -- Control: confirm we did not move.
    m = mud.mark()
    mud.send("look")
    mud.expect("Obvious exits:")
    mud.sleep(1)
    mud.save_section(tag .. "_after_look_" .. d, mud.since(m))
    mud.log(tag .. " still in '" .. tostring(mud.room_name()) .. "' (was '" .. tostring(here_name) .. "')")
  end

  return here_name, exits
end

survey("stop1")

-- Move one room and survey again; two stops is plenty to find a
-- differently-named neighbour.
mud.send("north")
mud.expect("Obvious exits:")
mud.sleep(2)

survey("stop2")

mud.log("look_vantage2 complete")
