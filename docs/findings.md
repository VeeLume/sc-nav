# Findings

Measurements and data facts behind sc-nav. The lab's `FINDINGS.md`
(`E:\vscode\sc-grpc`) holds the earlier record (overlay layout, `CamDir`,
phases, the clock).

## Named places from the game files (LIVE 4.10.2)

Source: sc-holotable `sc-locations`: `Locations` (StarMapObject records) joined
with `ObjectContainers::systems(assets, 2)` (40,012 placements, ~6 s).
Exploration: `cargo run --release -p sc-nav-data --example places`.

- 1,227 positioned places in Stanton, Pyro and Nyx: 517 outposts, 9 landing
  zones, 50 stations and other manmade places, 69 points of interest, 501
  asteroid places (Lagrange and ring clusters), jump points, comm arrays,
  QV breaker stations.
- `ObjectContainers::chain_of` runs **root first** (system → body → place).
- A place inside a planet's or moon's container (`stanton4.socpak`) has
  offsets in that body's own frame, the frame `CamPos Planet Zone` reads in:
  outposts sit a median 0.5 km (−2 to +88 km) above the body's datum radius.
  holotable's `global_position` adds these without the body's spin (off by
  up to a body radius); sc-nav keeps the body-frame offset instead.
- Containers that are not bodies (Lagrange points `stanton3_l1`, jump points,
  Pyro's Keeger and Glaciem ring segments) are fixed in the system: a place
  inside one is its offset plus the container's position.
- Orbital stations (Everus Harbor, Port Tressler, Baijini Point, Seraphim,
  Grim HEX) sit in their body's container, 110–8,200 km up. **Open:** whether
  that container turns with the body (stations geostationary) or not. A
  `/showlocation` or overlay fix near a station at two times settles it.
- No named places in Stanton's Aaron Halo.
- Per system: Stanton 820 places, Pyro 274, Nyx 133. Each system has its own
  frame (origin at its star), so a system position needs its system.
- Names: 1,023 of 1,043 (system, name) pairs are unique. 20 names cover 204
  places: generic names (`RSI Event Platform` ×49 over 11 bodies, Nyx's
  `QV Extraction Station` ×40 and `QV Breaker Station` ×31, Pyro's
  `Derelict Outpost` ×19, `Asteroid Cluster`, `Asteroid Mining Base`,
  `Contested Zone`), Nyx placeholders named `Nyx` ×17, `<= UNINITIALIZED =>`,
  and reused containers: ten Stanton `Mining Claim` / `Mining Base` names each
  placed twice, mostly ~3.8 million km apart (one pair 235 km). A name is a
  usable reference only when unique within its system.

## Systems' galactic positions (LIVE 4.10.2)

`SSolarSystem.galactic_position` (DataCore; sc-extract feature
`ssolarsystem`, or `megamap`). Unit not stated (light-years or the star
map's scale). Example: `cargo run --release -p sc-nav-data --example galaxy`.

| System | Galactic position | |
|---|---|---|
| Stanton | (49.53472, −2.63396, 16.47529) | Stanton–Pyro 36.685 |
| Pyro | (40.12613, 8.414866, −17.2174) | Pyro–Nyx 20.126 |
| Nyx | (20.0, 8.414866, −17.2174) | Stanton–Nyx 46.147 |

Only systems in the game files are listed. Use in sc-nav: a name-free
fallback for the system part of a full nav code (the galactic cell of the
system's star); the readable three-letter tag stays the default.

## Overlay reader speed (2026-11-24)

Bench: `cargo run --release -p sc-nav-capture --example bench -- out.jsonl`
replays every recorded grab under the lab's `hud-sessions` (3,246 grabs,
640x302 and 768x432 crops), times `Reader::read` alone and writes each
grab's log line, so a change can be diffed for identical reads.

Per grab, one thread, before → after (reads byte-identical on all 3,246):

| session | grabs | before mean / p90 | after mean / p90 |
|---|---|---|---|
| 1794434626 | 1,164 | 326 / 470 ms | 61 / 95 ms |
| 1794440111 | 286 | 386 / 525 ms | 55 / 71 ms |
| 1794442146 | 1,748 | 474 / 679 ms | 77 / 91 ms |
| 1794444000 | 48 | 141 / 140 ms | 19 / 20 ms |
| all | 3,246 | 408 / 605 ms | 68 / 91 ms (6x) |

Where the time went (timers, before): the glyph placement program
(`place_hits`) was ~80% — a `HashMap` per state and every earlier hit
tried as a predecessor, on ~290 hits per line. Then repeated reads: the
`Zone`, `Root` and `CamDir` searches read the same tokens again, number
reads found the hits twice (exact count, then the loose grammar), and
location names found them once per dictionary name.

What changed, all exact:

- `place_hits`: dense state arrays; predecessors only from hits whose ink
  can end within `gap` (sorted by x, so a binary search bounds them); one
  pass over the predecessors fills every count. Tie-breaking unchanged.
- NCC: templates centred and scaled to unit norm once; window sums from
  summed-area tables (f64); numerators accumulated across x so the loop
  vectorises; exemplar naming normalises each line window once and scores
  every exemplar of that size by a dot product.
- `glyph::Line`: a line's hits found once, read as several grammars
  (additive API; `read`/`place` are wrappers).
- Overlay: numbers memoised per frame; location names read from one set
  of hits over the letters of the names that fit the token's width; band
  backgrounds by selection instead of a full sort.

After, per frame (session 1794442146 dominates): number reads ~46 ms (hits
12, placement 17, naming 17 with up to 1,240 exemplars), the `Root` line
search reads most lines above the radius line, the checksum search ~6 ms.
The worst grab (9-10 s, session 1794442146) is the beam search at its
300,000-state cap, before and after.
