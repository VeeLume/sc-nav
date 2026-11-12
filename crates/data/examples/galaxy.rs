//! Exploration: the systems' galactic positions (`SSolarSystem`).
//!
//! ```bash
//! cargo run --release -p sc-nav-data --example galaxy
//! ```

use sc_holotable::asset::{AssetConfig, AssetData, AssetSource, Datacore};
use sc_holotable::install::discover_primary;

fn main() -> anyhow::Result<()> {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(run)?
        .join()
        .map_err(|_| anyhow::anyhow!("panicked"))?
}

fn run() -> anyhow::Result<()> {
    let install = discover_primary()?;
    let assets = AssetSource::from_install(&install)?;
    let data = AssetData::extract(&assets, &AssetConfig::standard())?;
    let datacore = Datacore::parse(&assets, &data)?;
    let store = datacore.records();
    let pools = &store.pools;
    let mut rows = vec![];
    for (guid, handle) in store.records.multi_feature.ssolar_system.iter() {
        let Some(s) = handle.get(pools) else { continue };
        let pos = s.galactic_position.and_then(|h| h.get(pools)).map(|v| (v.x, v.y, v.z));
        rows.push((s.name.clone(), pos, *guid));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, pos, guid) in &rows {
        println!("{name:<24} {pos:?}  {guid}");
    }
    let find = |n: &str| rows.iter().find(|r| r.0.eq_ignore_ascii_case(n)).and_then(|r| r.1);
    for (a, b) in [("Stanton", "Pyro"), ("Pyro", "Nyx"), ("Stanton", "Nyx")] {
        if let (Some(p), Some(q)) = (find(a), find(b)) {
            let d = ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2) + (p.2 - q.2).powi(2)).sqrt();
            println!("{a} - {b}: {d}");
        }
    }
    Ok(())
}
