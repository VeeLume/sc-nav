//! Load the world from the install and print what came out, with timings.
//!
//!     cargo run --release -p sc-nav-data --example load -- <cache dir>

fn main() -> anyhow::Result<()> {
    let cache = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/sc-nav-cache".into());
    let t = std::time::Instant::now();
    let (world, font) = sc_nav_data::load(std::path::Path::new(&cache))?;
    println!(
        "{} · {} bodies · {} ships · font {} bytes · {:.1}s",
        world.build,
        world.bodies.len(),
        world.ships.len(),
        font.len(),
        t.elapsed().as_secs_f64()
    );
    for b in world
        .bodies
        .iter()
        .filter(|b| ["stanton1", "stanton1a", "stanton3a", "stanton4"].contains(&b.id.as_str()))
    {
        println!(
            "  {} {} r {} day {:.4} h",
            b.id, b.name, b.radius_m, b.period_h
        );
    }
    if let Some(s) = world.ships.get("RSI_Constellation_Taurus") {
        println!("  RSI_Constellation_Taurus retro {:.2} g", s.backward_g);
    }
    Ok(())
}
