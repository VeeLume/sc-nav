//! Nav codes of some named places (from `--example places`' output) at
//! every length.
//!
//! ```bash
//! cargo run --release -p sc-nav-data --example navcodes
//! ```

use sc_nav_geo::navcode::{CALLOUT, full, ground, space};
use sc_nav_geo::Geo;
use sc_nav_geo::{World, location::system_of_id};

#[derive(serde::Deserialize)]
struct Row {
    name: String,
    frame: String,
    system: String,
    pos: [f64; 3],
}

fn main() -> anyhow::Result<()> {
    let rows: Vec<Row> = serde_json::from_slice(&std::fs::read("target/places.json")?)?;
    let world = std::fs::read_dir("target/sc-nav-cache")?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("world2-")))
        .ok_or_else(|| anyhow::anyhow!("no cached world"))?;
    let world: World = serde_json::from_slice(&std::fs::read(world)?)?;
    let want = ["Shubin Mining Facility SM0-18", "Lorville", "Everus Harbor", "ARC-L1 Wide Forest Station", "Grim HEX", "Orison", "Ruin Station"];
    for r in rows.iter().filter(|r| want.iter().any(|w| r.name.starts_with(w))) {
        let body = world.bodies.iter().find(|b| b.id == r.frame);
        let (still, system) = match body {
            Some(b) => ([b.center[0] + r.pos[0], b.center[1] + r.pos[1], b.center[2] + r.pos[2]], system_of_id(&b.id)),
            None => (r.pos, r.system.to_lowercase()),
        };
        println!("{} ({}, {})", r.name, r.frame, system);
        if let Some(b) = body {
            println!("   ground on {}:     {}", r.frame, ground(&Geo::from_fixed(r.pos), b, CALLOUT));
        }
        for len in [9, 12, 15] {
            println!("   space, {len:>2} chars:   {}", space(still, &system, len).unwrap_or_default());
        }
        if let Some(sys) = world.systems.iter().find(|s| s.id == system) {
            println!("   full, with tag:   {}", full(still, sys, false).unwrap_or_default());
            println!("   full, no names:   {}", full(still, sys, true).unwrap_or_default());
        }
    }
    Ok(())
}
