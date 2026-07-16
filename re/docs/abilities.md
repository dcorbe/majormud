# MajorMUD ability-id enum (WG3-NT)

The shared ability system: spells, monsters, items, races, classes all encode effects
as `(ability-id, value)` pairs. Source: `ability.mdb` (Nightmare-Redux, id/name/description)
+ `update_dynamic_with_ability` dispatch (32-bit decompile, effect semantics).
`*` = referenced by the WG3-NT engine (99 of 188). Full raw list: `ability_ids.tsv`.

| id | hex | used | name | description | dynamic effect |
|---|---|---|---|---|---|
| 0 | 0x00 |  | empty | The "nothing" attribute. New objects automatically have thei |  |
| 1 | 0x01 |  | Damage | Does damage in the amount specified by the parameter. MR is  |  |
| 2 | 0x02 | * | AC | Modifies the AC value specified by the parameter, this AC is | += AC @player+0x70c |
| 3 | 0x03 | * | Rcol | Modifies resistance to cold by the value specified by the pa |  |
| 4 | 0x04 | * | Max Damage | Modifies max damage to the value specified by the parameter. | += maxdmg @+0x70e |
| 5 | 0x05 | * | Rfir | Modifies resistance to fire by the value specified by the pa |  |
| 6 | 0x06 |  | Enslave | Charms the target. |  |
| 7 | 0x07 | * | DR | Modifies the DR value specified by the parameter. Note that  | += DR @+0x7b6 |
| 8 | 0x08 | * | Drain | Drains HP from the target to the caster. (usually seen in sp |  |
| 9 | 0x09 | * | Shadow | Displays "A shadowy figure!" when looked at by other players |  |
| 10 | 0x0a | * | AC(Blur) | Modifies the AC value specified by the parameter, this AC IS | += AC(blur) @+0x7bc |
| 11 | 0x0b |  | EnergyLevel | Modifies the Energy level specified by the parameter. |  |
| 12 | 0x0c |  | Summon | Summons a monster specified by the parameter. The value in p |  |
| 13 | 0x0d | * | Illu | Increases or decreases the light value for the target ONLY b |  |
| 14 | 0x0e |  | RoomIllu | Increases or decreases the light value for the entire room b |  |
| 15 | 0x0f |  | Alterhunger | Unknown |  |
| 16 | 0x10 |  | Alter thirst | Unknown |  |
| 17 | 0x11 |  | Damage(-MR) | Does damage which takes into account the targets MR (or so t |  |
| 18 | 0x12 |  | Heal | Heals damage specified by the value in the parameter. |  |
| 19 | 0x13 | * | Poison | Poisons the target doing damage specified by the value in th |  |
| 20 | 0x14 |  | Cure Poison | Cures poison (cure poison has a parameter of 8, as does anti |  |
| 21 | 0x15 | * | ImmuPoison | Used in duration spells to make the target immune to poison. |  |
| 22 | 0x16 | * | Accuracy | Increases the targets accuracy by the value specified in the |  |
| 23 | 0x17 |  | AffectsUndead | Used to make a spell ONLY work on undead monsters. |  |
| 24 | 0x18 | * | ProtEvil | Protection from evil. |  |
| 25 | 0x19 | * | ProtGood | Protection from good. |  |
| 26 | 0x1a |  | DetectMagic | Detects the magic in an item. |  |
| 27 | 0x1b |  | Stealth | Modifies stealth by the value in the parameter. |  |
| 28 | 0x1c | * | Magical | Defines the magical potency of an item. Witchunters cannot u |  |
| 29 | 0x1d | * | Punch | Used in the Mystic class definition to grant the Punch abili |  |
| 30 | 0x1e | * | Kick | Used in the Mystic class definition to grant the Kick abilit |  |
| 31 | 0x1f | * | Bash | Used in almost all class definition to grant the ability to  |  |
| 32 | 0x20 | * | Smash | Unknown |  |
| 33 | 0x21 |  | Killblow | Unknown |  |
| 34 | 0x22 | * | Dodge | Increases or decreases the targets ability to Dodge by the % |  |
| 35 | 0x23 | * | JumpKick | Used in the Mystic class definition to grant the JumpKick ab |  |
| 36 | 0x24 | * | M.R. | Increases or decreases the targets MR by the value specified |  |
| 37 | 0x25 | * | Picklocks | In Race and Class definitions, this grants the ability to Pi |  |
| 38 | 0x26 | * | Tracking | Used in Race and Class definitions to grant Tracking to a Ra |  |
| 39 | 0x27 | * | Thievery | In Race and Class definitions, this grants the ability to Ro |  |
| 40 | 0x28 | * | FindTraps | In Race and Class definitions, this grants the ability to fi |  |
| 41 | 0x29 |  | DisarmTraps | In Race and Class definitions, this grants the ability to di |  |
| 42 | 0x2a |  | LearnSp | Places the spell specified in the parameter into the players |  |
| 43 | 0x2b |  | CastsSp | Casts the spell when an item is 'used', or, with the %Spell  |  |
| 44 | 0x2c | * | Intel | Increases or decreases the Intel stat by the value specified |  |
| 45 | 0x2d | * | Wisdom | Increases or decreases the Wisdom/Willpower stat by the valu |  |
| 46 | 0x2e | * | Strength | Increases or decreases the Strength stat by the value specif |  |
| 47 | 0x2f | * | Health | Increases or decreases the Health stat by the value specifie |  |
| 48 | 0x30 | * | Agility | Increases or decreases the Agility stat by the value specifi |  |
| 49 | 0x31 | * | Charm | Increases or decreases the Charm stat by the value specified |  |
| 50 | 0x32 |  | MageBaneQuest | This Determines if the player has completed the MageBane que |  |
| 51 | 0x33 | * | AntiMagic | Used in the Witchunter class definition, apperently the only |  |
| 52 | 0x34 | * | EvilInCombat | Used in spells to generate evil warnings and give EP when us |  |
| 53 | 0x35 |  | BlindingLight | Unknown |  |
| 54 | 0x36 | * | IlluTarget | Increases or decreases the light level for the target specif |  |
| 55 | 0x37 |  | AlterGeneralLightDuration | Unknown |  |
| 56 | 0x38 |  | RechargeItem | Unknown |  |
| 57 | 0x39 | * | SeeHidden | Grants the ability to see hidden players. For monsters, this | set SEE-HIDDEN flag @+0x6f4 |
| 58 | 0x3a |  | Crits | Increases or decreases the targets critical hit chance by th | += crits @+0x7b4 |
| 59 | 0x3b |  | ClassOk | Used in items to make the item equippable by classes they ar |  |
| 60 | 0x3c | * | Fear | Makes the target flee in fear. Used in duration spells. | set FEAR flag @+0x7c8 |
| 61 | 0x3d |  | AffectExit | Unknown |  |
| 62 | 0x3e |  | EvilChance | Unknown |  |
| 63 | 0x3f |  | Experience | Unknown (although one would assume this can be used to add/r |  |
| 64 | 0x40 |  | AddCP | Unknown (although one would assume this can be used to add/r |  |
| 65 | 0x41 | * | ResistStone | Modifies resistance to stone by the value specified by the p |  |
| 66 | 0x42 | * | Rlit | Modifies resistance to lightning by the value specified by t |  |
| 67 | 0x43 |  | Quickness | does in fact make you walk faster (but only to a point so th |  |
| 68 | 0x44 | * | Slowness | Modifies the room movement time while not affecting stealth  | set SLOW flag @+0x7c8 |
| 69 | 0x45 | * | MaxMana | Increases or decreases the targets maximum mana by the value |  |
| 70 | 0x46 | * | S.C. | Incresaes or decreases the targets SC by the value specified |  |
| 71 | 0x47 | * | Confusion | Confuses the target, making them fumble in the % specified b | set CONFUSE flag @+0x6f4 |
| 72 | 0x48 | * | DamageShield | Used to setup a shockshield for the player. Shockshield uses |  |
| 73 | 0x49 |  | DispellMagic | cancels spells that affect the ability in the value (ie disp |  |
| 74 | 0x4a | * | HoldPerson | Holds the target in the room. Used with duration spells. Ent | set HOLD flag @+0x7c8 |
| 75 | 0x4b |  | Paralyze | Unknown | set PARALYZE flag @+0x6f4 |
| 76 | 0x4c |  | Mute | Strikes the target mute, making them unable to talk in rooms | set MUTE flag @+0x6f4 |
| 77 | 0x4d | * | Percep | Increases or decreases the targets perception by the value s |  |
| 78 | 0x4e | * | Animal | Used in monster attributes to flag a monster as an animal. S |  |
| 79 | 0x4f | * | MageBind | did use to make spellcasting impossible but not sure it's st |  |
| 80 | 0x50 |  | AffectsAnimals | Used in spells to make it ONLY affect animals. |  |
| 81 | 0x51 | * | Freedom | Frees the target from holding spells and spells that alter t |  |
| 82 | 0x52 | * | Cursed | Used on items, once worn, a Cursed item can't be removed. |  |
| 83 | 0x53 | * | CURSED | has the same affects as 82 (curse) so you can't remove an it |  |
| 84 | 0x54 |  | Rcrs | Remove curse. Seen in spells. The Priest spell "Remove Curse |  |
| 85 | 0x55 |  | Shatter | Shatters the targets weapon. The parameter (in theory) speci |  |
| 86 | 0x56 | * | Quality | Specifies an items "Quality". The only effect of this is to  |  |
| 87 | 0x57 | * | Speed | Increases or decreases the targets swing speed with their we | += speed @+0x7ba |
| 88 | 0x58 | * | Alter HP | increases or decreases the targets max HP |  |
| 89 | 0x59 | * | PunchACY | believed to alter punch accuracy value |  |
| 90 | 0x5a | * | KickACY | believed to alter kick accuracy value |  |
| 91 | 0x5b | * | JumpKACY | believed to alter jump accuracy value |  |
| 92 | 0x5c |  | PunchDmg | Increases punch damage for the Mystic punch attack. |  |
| 93 | 0x5d |  | KickDmg | Increases kick damage for the Mystic kick attack. |  |
| 94 | 0x5e |  | JumpKDmg | Increases jumpkick damage for the Mystic jumpkick attack. |  |
| 95 | 0x5f |  | Slay | Unknown. Used by the "laen longsword" item. |  |
| 96 | 0x60 | * | Encum | Increases or decreases the targets encumberance % by the val |  |
| 97 | 0x61 | * | Good | In spells and items, makes the spell/item useable only by Go |  |
| 98 | 0x62 | * | Evil | In spells and items, makes the spell/item useable only by Ev |  |
| 99 | 0x63 |  | AlterDRpercent | changes DR by the % in the value (is cast by some Mod 9 mons | += DR% @+0x7b8 |
| 100 | 0x64 | * | LoyalItem | Makes the item stay with the player even into death. Seen in |  |
| 101 | 0x65 | * | ConfuseMsg | Sets the confusion message for a previous "Confusion" attrib |  |
| 102 | 0x66 | * | RaceStealth | Used in Race definitions to grant racial stealth. |  |
| 103 | 0x67 | * | ClassStealth | Used in Class definitions to grant class stealth. |  |
| 104 | 0x68 | * | DefenseModifier | Unknown | += defmod @+0x7be |
| 105 | 0x69 | * | Accuracy(2) | Increases or decreases accuracy by the value specified in th |  |
| 106 | 0x6a | * | Accuracy (3) | Increases or decreases accuracy by the value specified in th |  |
| 107 | 0x6b |  | BlindUser | Blinds the target. | set BLIND flag @+0x6f4 |
| 108 | 0x6c |  | AffectsLiving | Used in spells to limit their abilities to only living playe |  |
| 109 | 0x6d | * | NonLiving | Used in monster attributes to flag the monster as being Non  |  |
| 110 | 0x6e | * | NotGood | Used in items/spells to flag the object as being useable by  |  |
| 111 | 0x6f | * | NotEvil | Used in items/spells to flag the object as being useable by  |  |
| 112 | 0x70 | * | Neutral | In spells and items, makes the spell/item useable only by Ne |  |
| 113 | 0x71 |  | NotNeutral | Used in items/spells to flag the object as being useable by  |  |
| 114 | 0x72 | * | %Spell | Used in items to make a "spell effect" occur on a percent ch |  |
| 115 | 0x73 | * | DescMsg | Used in spells. Specifies the message in WCCMSG.DAT to use w |  |
| 116 | 0x74 | * | BSAccu | Used in items (although I can see no reason why it couldn't  |  |
| 117 | 0x75 | * | BsMinDmg | Used in items (although I can see no reason why it couldn't  |  |
| 118 | 0x76 |  | BsMaxDmg | Used in items (although I can see no reason why it couldn't  |  |
| 119 | 0x77 | * | Del@Maint | Used in items to delete the item, hidden or not, at cleanup. |  |
| 120 | 0x78 | * | StartMsg | Used in spells to specify the 'start message' for the spell. |  |
| 121 | 0x79 | * | Recharge | Used in items to specify the amount of "uses" to recharge at |  |
| 122 | 0x7a |  | RemovesSpell | Used in spells to remove other spells (cancel them out). |  |
| 123 | 0x7b |  | HPRegen | Used in any object to modify the targets HP regen rate. The  | += hpregen @+0x7d8 |
| 124 | 0x7c |  | NegateAbility | Removes the ability number? |  |
| 125 | 0x7d |  | IceSorcQuest | This is the progress the player has made with the Ice Sorcer |  |
| 126 | 0x7e |  | GoodQuest | This is the progress the player has made with the GOOD quest |  |
| 127 | 0x7f |  | NeutralQuest | This is the progress the player has made with the NEUTRAL qu |  |
| 128 | 0x80 |  | EvilQuest | This is the progress the player has made with the EVIL quest |  |
| 129 | 0x81 |  | DarkDruidQuest | This is the progress the player has made with the Dark Druid |  |
| 130 | 0x82 |  | BloodChampQuest | This is the progress the player has made with the Blood Cham |  |
| 131 | 0x83 |  | SheDragonQuest | This is the progress the player has made with the SheDragon  |  |
| 132 | 0x84 |  | WereratQuest | This is the progress the player has made with the Wererat qu |  |
| 133 | 0x85 |  | PhoenixQuest | This is the progress the player has made with the Phoenix fe |  |
| 134 | 0x86 |  | DaoLordQuest | This is the progress the player has made with the DaoLord qu |  |
| 135 | 0x87 |  | MinLevel | Typically seen in items (useless for spells since they have  |  |
| 136 | 0x88 |  | MaxLevel | Typically seen in items to restrict them to players ONLY of  |  |
| 137 | 0x89 | * | Shock | Sets the Shockshield message for the spell/item. See "DmgShi |  |
| 138 | 0x8a | * | RoomVisible | Makes the item visible to the entire room. |  |
| 139 | 0x8b | * | SpellImmu | Used in objects to make the target immune to spells below or |  |
| 140 | 0x8c |  | TeleportRoom | Teleports the target to the specified room in the parameter. |  |
| 141 | 0x8d |  | TeleportMap | Teleports the target to the specified map in the parameter.  |  |
| 142 | 0x8e | * | HitMagic | Seen in weapons (and possibly could be used in spells) and u |  |
| 143 | 0x8f |  | ClearItem | Poofs or unequips item number? |  |
| 144 | 0x90 |  | NonMagicalSpell | Flags the spell as non-magical, thus making it immune to MR. |  |
| 145 | 0x91 |  | ManaRgn | Used in objects to increase or decrease the targets mana reg |  |
| 146 | 0x92 |  | MonsGuards | Used in monster definitions to define a monster that protect |  |
| 147 | 0x93 | * | ResistWater | Increases or decreases the targets resistance to Water by th |  |
| 148 | 0x94 |  | TextBlock | Executes the text block specified by the parameter. |  |
| 149 | 0x95 | * | Remove@Maint | Used in items-- removes the item at cleanup, poofing it. |  |
| 150 | 0x96 |  | HealMana | Regenerates mana back by the value specified in the paramete |  |
| 151 | 0x97 |  | EndCast | Used in spell definitions to chain to another spell at the e |  |
| 152 | 0x98 |  | Rune | Flags the object as a Rune. Seen in one item. |  |
| 153 | 0x99 |  | KillSpell | Kills a spell without calling it's EndCast effect (if one ex |  |
| 154 | 0x9a | * | Visible@Maint | Used in items to keep them visible at cleanup. |  |
| 155 | 0x9b | * | DeathText | Unknown (executes the specified text block when the player d |  |
| 156 | 0x9c |  | QuestItem | Makes the item a quest item |  |
| 157 | 0x9d |  | ScatterItems | Scatters the items in the room to the specified room(s). You |  |
| 158 | 0x9e | * | ReqToHit | Monsters with this flag require that weapons (and spells?) w |  |
| 159 | 0x9f | * | KaiBind | Prevents the target from using Kai powers. Seen in 1.1u's ra |  |
| 160 | 0xa0 |  | GiveTempSpell | Unknown (gives the spell specified in Value/Parameter tempor |  |
| 161 | 0xa1 |  | OpenDoor | Unknown (opens a door, I assume, but how it determines direc |  |
| 162 | 0xa2 |  | Lore | Unknown |  |
| 163 | 0xa3 |  | SpellComponent | Make the item a spell component. |  |
| 164 | 0xa4 |  | CastOnEnd% | cast a spell specificed in the next EndCast ability when thi |  |
| 165 | 0xa5 | * | AlterSpDmg | Unknown (alter spells damage (eg: used in an item or another |  |
| 166 | 0xa6 | * | AlterSpLength | Unknown (alter the length of duration spells (eg: used in an |  |
| 167 | 0xa7 |  | UnEquipItem | Unknown (remove either the specific item, or, perhaps the va |  |
| 168 | 0xa8 |  | EquipItem | Unknown (equip either the specific item, or, perhaps the val |  |
| 169 | 0xa9 |  | CannotWearLocation | Unknown (perhaps used to make an item that can be worn on mu |  |
| 170 | 0xaa |  | Sleep | Unknown |  |
| 171 | 0xab |  | Invisibility | Unknown (some new form of stealth perhaps that doesn't rely  |  |
| 172 | 0xac |  | SeeInvisible | Unknown (most likely a way for monsters/players to see other |  |
| 173 | 0xad |  | Scry | Unknown (perhaps show the room specificed in the value? No i |  |
| 174 | 0xae |  | StealMana | Unknown (most likely steals Mana from another Player or mons |  |
| 175 | 0xaf |  | StealHPtoMP | Unknown (convert stolen HP to Mana instead? Works like the o |  |
| 176 | 0xb0 |  | StealMPtoHP | Unknown (convert stolen Mana to HP instead? Works like the o |  |
| 177 | 0xb1 |  | SpellColours | Unknown (perhaps a way to specific the ANSI colour coding fo |  |
| 178 | 0xb2 | * | Shadowform | This functions much like shadowform except the value states  |  |
| 179 | 0xb3 | * | FindTrapsValue | alters user's Traps Value |  |
| 180 | 0xb4 |  | PickLocksValue | alters user's PickLocks Value |  |
| 181 | 0xb5 | * | GHouseDeed | Gang House Deed value |  |
| 182 | 0xb6 | * | GHouseTax | Gang House Tax value |  |
| 183 | 0xb7 | * | GHouseItem | Gang House Item |  |
| 184 | 0xb8 | * | GShopItem | Gang Shop Item (controller) |  |
| 185 | 0xb9 | * | BadAttk | Do not Attack if Item Nmbr |  |
| 186 | 0xba | * | PerStealth | Perfect Stealth |  |
| 187 | 0xbb | * | Meditate | Meditate skill granted |  |
