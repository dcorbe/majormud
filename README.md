# majormud

A from-scratch MajorMUD server and an automated client for the original game.

MajorMUD (WCCMMUD 1.11p, Metropolis Inc.) is a BBS door game. `re/docs/`
holds a behavioural specification recovered from the game's own binaries and
data files and checked against the real game running under MBBSEmu. The crates
here are built from that specification.

## Crates

| Crate | What it is |
|---|---|
| `mud-core` | The game engine. Pure game logic, deterministic given state, input and RNG seed. |
| `mud-server` | Content loading, networking and persistence around `mud-core`. Listens on port 2325. |
| `mud-client` | `mmc`, an automated client for the original game: interactive play, headless Lua runs, routing, farming. See `docs/mud-client.md`. |
| `textscreen` | A CP437 text screen: the cell grid and the painter that turns it into ANSI. Used by the server and the client's terminal UI. |

## Building and testing

    cargo build
    cargo test

The test suite is self-contained. Running the server or the client against
real content needs the untracked `re/` directory described below.

## Running the server

    cargo run -p mud-server -- --content re/mmud_wgnt.sqlite --listen 0.0.0.0:2325

Accounts and saved players are written to `state.sqlite` in the working
directory. Gang house files are read## Layout

- `crates/mud-core/ability_ids.tsv` the ability table. The crate's build script generates the `Ability` enum from it.
- `tools/oracle/` scripts that drive the real game under MBBSEmu to capture transcripts. Run them from that directory. See its README.
- `docs/` the client manual, and the design documents and plans behind each milestone.
- `re/` is gitignored. It holds the reverse-engineered material the code was written against and is not redistributed: the behavioural specification (`re/docs/`, one file per game system), the content database (`re/mmud_wgnt.sqlite`, built from the game's Btrieve data files by `re/import_mmud.py`), captured transcripts from the real game (`re/oracle/`), and the gang house files (`re/hse_files/`). The server's `--content` and `--houses` defaults point into it.

behind each milestone.

## History

This repo was split out of a larger BBS emulation repo with `git filter-repo`.
The commits are the ones that touched these crates and their data, with the
rest of that repo removed.

## License

MIT. See `LICENSE.md`.
