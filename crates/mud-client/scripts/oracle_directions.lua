-- oracle_directions: movement and error-response probes.
-- Port of tools/oracle/oracle_directions.py essentials. Works against
-- the Rust server (creates a fresh character); on the live board run
-- with an existing character instead.

local outcome = mud.login()

if outcome == "create" then
  -- login() left us at the race list.
  mud.send("1")
  mud.expect("Please choose a class from the following list:")
  mud.send("1")
  mud.expect("Do you want to be Lawful?")
  mud.send("No")
  mud.expect("[HP=")
end

-- look
local m = mud.mark()
mud.send("look")
mud.expect("Obvious exits:")
mud.save_section("look", mud.since(m))

-- move north, then back south
m = mud.mark()
mud.send("n")
mud.expect("Obvious exits:")
mud.save_section("move_n", mud.since(m))

m = mud.mark()
mud.send("s")
mud.expect("Obvious exits:")
mud.save_section("move_back_s", mud.since(m))

-- a direction with no exit
m = mud.mark()
mud.send("e")
mud.expect("There is no exit in that direction!")
mud.save_section("bad_direction", mud.since(m))

mud.log("oracle_directions complete")
