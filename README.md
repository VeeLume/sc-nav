# sc-nav

Navigation for Star Citizen, as a set of small Rust crates: read where you
are from the game's debug overlay and its local telemetry, steer to marks on
planets or in space, and get warned before you hit the ground.

Nothing here touches the game process or its network traffic. It reads the
screen (`r_DisplayInfo 1` or `2`), the clipboard (`/showlocation`), the
game's own local telemetry stream (when started with
`GRPC_CLIENT_SERVER_ENABLED=true`), and the game files (through
[sc-holotable](https://github.com/VeeLume/sc-holotable)).

## Crates

| Crate | What it is |
|---|---|
| `sc-nav-glyph` | Generic glyph matcher: read a line of game text from an ink image against templates, with ranked candidates per position. Pure. |
| `sc-nav-vision` | Image tools for HUD panels: ink and shadow feature images, rotation, resampled bands, NCC over a whole image (FFT), peaks. Pure. |
| `sc-nav-font` | The game's HUD fonts from its own `fonts_en.gfx`, rendered to glyph templates. Pure; the fonts come from the install. |
| `sc-nav-geo` | Pure maths: body frames, lat/long, SCAR (planet-local), surface and space guidance, the ground warning, and `Location` (a surface or a space point, one shareable code for both). |
| `sc-nav-clock` | A corrected wall clock (SNTP), so nothing trusts the PC clock. |
| `sc-nav-overlay` | Reads `r_DisplayInfo` from pixels: positions under a checksum, the view direction, the location name. No capture. |
| `sc-nav-data` | Bodies, ships' braking and the overlay font from the installed game, cached per build. |
| `sc-nav-capture` | Windows: the game window as a frame; with the default `overlay` feature, the overlay region of the screen (with recording) and `/showlocation` from the clipboard. |
| `sc-nav-pilot` | The convenient layer: everything above fused into one state with a plain snapshot (position, steering, braking cue, PULL UP). Live feeds, recording and replay behind the `runtime` feature. |
| `sc-nav-hud` | The egui app: a click-through HUD and a control window. |

Pick what you need: a Stream Deck plugin takes `sc-nav-pilot`, something that
only wants SCAR takes `sc-nav-geo`.

## Run

```bash
cargo run --release -p sc-nav-hud            # the game started with GRPC_CLIENT_SERVER_ENABLED=true
cargo run --release -p sc-nav-hud -- --demo  # a simulated flight, no game
```

The first start after a patch reads the game files (about two minutes).
Everything the app keeps (that cache, marks, measured phases, recorded
sessions) lives in `%LOCALAPPDATA%\sc-nav`; `SC_NAV_HOME` moves it. Play in
borderless windowed mode: exclusive fullscreen hides the HUD.

## Ethics

Read-only and local. The telemetry is the game's own opt-in local service; the
overlay is a debug display the game shows on request.

## License

MIT.
