# CLAUDE.md — sc-nav

Star Citizen navigation as a modular Rust workspace. Graduated from the
sc-grpc lab (`E:\vscode\sc-grpc`, no git), where the overlay reader, the
director and the HUD were worked out; the lab's `FINDINGS.md` is the record of
what was measured (overlay layout, `CamDir`'s convention, phases, the clock).
New measurements go to `docs/findings.md` here.

## Layers (dependencies point down)

```
sc-nav-hud ──► sc-nav-pilot[runtime] ──► sc-nav-capture ──► sc-nav-overlay ──► sc-nav-glyph
                     │                                           │
                     ├──► sc-nav-clock                           └──► sc-nav-geo
                     └──► sc-nav-geo ◄── sc-nav-data (sc-holotable)
```

- **Pure crates stay pure**: `sc-nav-glyph`, `sc-nav-geo` (serde only),
  `sc-nav-overlay` (takes an image and font bytes), `sc-nav-pilot` without
  `runtime`. No threads, no files (except the pilot's marks/phases saving),
  no platform code. Platform and I/O live in `capture`, `data`, `clock` and
  `pilot`'s `runtime` feature.
- **Game data comes from the install at runtime** (`sc-nav-data` through
  sc-holotable, pinned by tag), cached per exact build. Never commit
  pre-extracted game data.
- **Telemetry only through sc-dossier's `telemetry` feature** (git tag).
- **Every input to the pilot is an `Input` with its corrected time.** Live
  feeds, recordings and replays use the same path; a new data source becomes
  a new `Input` variant, not a side channel.

## Hot loops

Glyph matching (`sc-nav-glyph::hits`/`place`), line finding and the checksum
search (`sc-nav-overlay::read`) run once a second on ~25 lines. Measure before
and after changing them; keep the recorded-session tests passing.

## Conventions

- English everywhere. Conventional Commits. Pushing and tagging are Valerie's
  call: ask once when something is ready.
- Captured data is adversarial: readers return `None`/`Err`, never panic.
- Tests against real recordings read them from the lab
  (`E:\vscode\sc-grpc\.scratch\hud-sessions`) and skip when absent.

## Build / test

```bash
cargo test --workspace --all-features
cargo run --release -p sc-nav-data --example load   # reads the install, fills the cache
```
