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
