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

## Vision primitives and window capture speed (2026-11-24)

Bench: `cargo run --release -p sc-nav-vision --example primitives -- <screenshot>`
on a 2560x1440 Starlume screenshot (medians of 7), and
`cargo run --release -p sc-nav-capture --example window -- <exe>` for the
capture. Starlume's near search (panel on screen, 33 screenshots) spent
about 180 ms per read: names 80, refine 33, digits 26, the vote 25 (wall),
features of the window 15.

| step | before | after | same output |
|---|---|---|---|
| `Features::of`, whole frame | 155 ms | 29 ms | exact |
| `Features::of`, 572x700 window | 16 ms | 3 ms | exact |
| tilted window (rotate the frame, crop) | 79 ms | 7.5 ms (`rotate_rect`) | exact |
| FFT NCC, 60x10 template on the half frame | 19 ms | 11 ms (`Prepared`) | exact |
| 40 names slid over a 300x30 band | 123 ms | 15 ms (`Scorer`) | scores within 5e-6 |
| refine-sized check (13x13 spots, 9 scales, 200x20) | 6.2 ms | 1.8 + 1.1 (`Scorer::new`) | scores to float rounding |
| game window, one shot | ~300 ms (opening) | 3.8 ms whole 1920x1032, 0.9 ms a 600x700 region (`Session`) | same pixels |

- The shadow ink's 5x5 closing was slow because the filter took `max`/`min`
  as a function pointer, called per pixel. Now it is generic and runs on
  whole slices. Max and min are exact in any order.
- A whole-frame `rotate` is still ~70 ms, and that is arithmetic: 0° costs
  the same. Making it faster without changing a pixel needs SIMD across
  pixels. Only the full search's tilts pay this cost; a tracked panel turns
  just its window.
- A correlator plans its transforms once and is `Sync`, so one per feature
  image can be shared by threads. A `Prepared` template saves the template's
  forward transform for every correlator with the same padded size. A
  reader that keeps its search window's size fixed transforms each template
  once per session.
- Opening a WGC capture is the cost (~300 ms warm, 2 s cold). A kept
  `Session` copies the newest frame on the GPU and moves only the asked
  rectangle to the CPU.

## Whole-frame panel search (2026-11-24)

Starlume's full search (no panel on screen: 3.5 s) runs its vote five
times: upright, then four tilts. Each vote is 4 part threads × 5 scales ×
~5 templates, about 100 FFT correlations over the half frame (1280x720).
Each tilt also rotates the frame (~70 ms) and recomputes its features
(29 ms). The `primitives` example simulates the vote on a screenshot.

| step (half frame) | before | after |
|---|---|---|
| `Correlator::new` | 11 ms | 5.6 ms |
| `ncc`, 60x10 template | 19 ms | 7.6 ms |
| `ncc_prepared` | 11 ms | 5.0 ms |
| `peaks`, 8 | 4.4 ms | 1.6 ms |
| simulated vote, 4 threads | 620 ms | 327 ms (238 with templates prepared once) |

- Fresh 7 MB buffers per call cost their page faults every time: buffers
  are now kept per thread. Exact.
- The 2D FFT runs its rows in one call and its columns over a tiled
  transpose: bit-identical, but only 6.3 → 5.9 ms. The column gather was
  not the cost.
- `peaks` scans for its maxima eight lanes at a time and finds the last
  one block by block. Exact `max_by(total_cmp)`; zeros and NaN take the
  plain scan.
- Real-input transforms (realfft): half the spectrum, half the work and
  memory. Maps equal the complex ones to rounding (tests: within 2e-5).
  On 8 screenshots, every part's, scale's and template's peaks (14,000)
  are the same set as before. The only change is one pair of exactly
  tied scores that swapped order.
- With 4 threads the vote is memory-bound: 20 threads (one per part and
  scale) gain nothing. What is left on Starlume's side: preparing its
  templates once per session (the frame size is fixed), and running the
  four tilts at once instead of after each other.

## Overlay checksum search outliers (2026-11-24)

Four of the 3,246 recorded grabs took over half a second, and none of them
was read: 9.5 s, 9.1 s and 1.8 s with no solution, and 1.5 s ambiguous.
In all four the checksum's beam search ran up to its cap of 300,000
states, 52 to 61 times, on 44 to 70 uncertain digits.

- Exact: each state kept a copy of every pick so far (up to 70 bytes), and
  trimming the beam allocated a vector per state (about 18 million
  allocations on the worst grab). Now each step keeps a (parent, pick) per
  state, in the same order, and the best state's picks are traced back at
  the end. Reads byte-identical; the worst grab 9.5 → 1.35 s.
- The cap: 300,000 → 50,000 states. Reads are byte-identical on all
  3,246 grabs at 100,000, 50,000 and 30,000 alike; the slowest grab then
  takes 549, 451 and 167 ms. The beam keeps the cheapest states, and a
  verified fit costs 0 or close, so a real fit survives the trim.
- The overlay bench lists every grab over 500 ms (`slow:` on stderr).

## Glyph reads (2026-11-24)

Overlay bench, all 3,246 grabs, temporary timers per phase of
`glyph::Line` (removed again). A grab runs ~75 placements over ~348 hits,
each hit with ~43 neighbours.

| phase, per grab | before | after |
|---|---|---|
| hits | 12.3 ms | 7.7 ms |
| placement | 17.5 ms | 15.1 ms |
| naming (incl. exemplar prep 2.0) | 19.4 ms | 14.0 ms |
| whole read (mean) | 52.2 ms | 41.0 ms |

Reads byte-identical. The changes:

- Hits: each window's spread (box sums, √) is computed once per template
  size per line instead of once per template.
- Placement: which count ends a slot best at a hit, and the highest count
  set there, are found once the hit is done instead of for every later
  hit that follows it; the count loop is branch-free. Its cost is now the
  number of (hit, neighbour, slot) steps, about 2 million per grab.
- Naming: exemplars grouped by char once per line (an index per exemplar,
  not a map lookup and a string search for each of up to 1,240), and
  scored four windows at a time (`ncc::dot4`, each sum `dot`'s to the
  bit). A dot4 path without its per-chunk check was slower (15 → 18 ms)
  and was dropped.
- Not done: exemplar prep (2 ms) is per `Line`. Keeping it across lines
  needs an API that holds prepared templates next to the `TemplateSet`,
  which overlay and Starlume both edit in place.

## Overlay line roles (2026-11-25)

The `r_DisplayInfo` block is fixed to the screen, right-aligned, one line
every ~13 px. Which line sits on which row is not fixed. Lines come and go
with the player's state (a vehicle's or elevator's `Zone` line, the
planet's `OOC` line, CamPos missing on foot), and each one moves every line
below it. The line finder also reports rows between grid rows (on the
yellow and orange lines, which the min-over-RGB luminance makes faint).

Over the recorded sessions, the set of line rows:

| session | grabs | row sets | changes between grabs |
|---|---|---|---|
| 1794434626 | 1,164 | 44 | 284 |
| 1794440111 | 286 | 30 | 81 |
| 1794442146 | 1,748 | 152 | 571 |
| 1794444000 | 48 | 6 | 10 |

The `Root` search was 60% of a read: it went up from CamPos line by line,
reading each line's last three tokens as km and as metres, about 34 number
reads per grab. Now the reader remembers the row and label shape (the ink
widths of `Zone:`, `Root`, `Pos:`) of the line it took, and tries that line
first when a line with the same label shape is on that row. It then checks
lower lines with a `Zone:`-shaped label, because the full search takes the
lowest line that agrees. In Stanton, `SolarSystem` (just above `Root`)
prints the same position, and a frame where `Root` didn't read teaches the
`SolarSystem` row. When the remembered line doesn't check out, every line
is searched as before.

- Number reads per grab: ~45 → 32.7. Reads byte-identical on all 3,246
  grabs. Without the lower-line check, 3 reads took `SolarSystem`'s
  decimetres. With a row hint alone, without the label check, 59 reads
  differed, because reads teach exemplars and one changed read changes
  later frames too.
- Wall time, game closed, all 3,246 grabs, two rounds (the second in
  reverse order). Means per grab:

  | commit | round 1 | round 2 | worst grab |
  |---|---|---|---|
  | ffdc116 (before the outlier, glyph and roles work) | 73.0 ms | 64.3 ms | 9.6 / 8.9 s |
  | 60036a8 (outlier cap, glyph reads) | 54.9 ms | 57.1 ms | 0.72 / 0.31 s |
  | 947b8ca (Root line roles) | 34.3 ms | 31.0 ms | 0.42 / 0.31 s |

  Reads byte-identical across all three. Absolute times drift between
  days on this machine (the glyph section's 52 → 41 ms was measured on
  another run); the ratios hold.
- The radius line and the location name still scan every line. The full
  scan's result (the lowest qualifying line, the best name over all lines)
  can only be kept exactly by reading the lines a hint would skip.

## CPU while the game runs (2026-11-25)

Star Citizen keeps about ten cores busy, so what runs beside it matters.
Measured with the game closed (process CPU from `GetProcessTimes` /
`TotalProcessorTime`):

| part | CPU |
|---|---|
| overlay grab, whole screen through xcap + crop, once a second | 18 ms per grab (33 ms wall) |
| overlay grab, kept `Session` + `region` of the corner | < 1 ms per grab; an idle open session 0 ms over 5 s |
| overlay read | ~33 ms per grab (see above) |
| HUD, demo telemetry at 15 Hz, redrawn on every update and at least every 250 ms | ~5% of one core, 85–112 MB |
| game data load after a patch (sc-holotable) | ~130 s at ~1.15 cores, working set up to 5.8 GB, once per build |

- The watch loop now captures the game window with a kept `Session` and
  copies only the overlay corner off the GPU. xcap is gone from
  sc-nav-capture. A window in front of the game, sc-nav's own HUD
  included, no longer ends up in the grab. Without the game (not running,
  minimised) the loop looks for its window every 5 s; turned off, it
  closes the session.
- Not yet verified in the game: the corner of a borderless game window is
  the screen's corner. In windowed mode the window capture includes the
  title bar, which moves the lines down by its height. The reader finds
  lines wherever they are.
