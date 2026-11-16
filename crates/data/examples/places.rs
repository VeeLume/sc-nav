//! Exploration: every star-map place with a position, in the frame it lives
//! in. A place on a planet or moon is placed in that body's object container,
//! so its offsets up to the body are body-fixed (the frame `CamPos Planet
//! Zone` reads in); a place in a `*system.socpak` is system-global.
//!
//! Prints `kind  frame  depth  |r| km  x y z (m)  name` per place, then a
//! count per kind and frame. Writes the rows to `target/places.json`.
//!
//! ```bash
//! cargo run --release -p sc-nav-data --example places -- [depth]
//! ```

use std::collections::BTreeMap;

use sc_holotable::asset::{AssetConfig, AssetData, AssetSource, Datacore, RecordCollection};
use sc_holotable::install::discover_primary;
use sc_holotable::locations::{Locations, ObjectContainers, Universe};

#[derive(serde::Serialize)]
struct Row {
    name: String,
    kind: String,
    /// Body id (`stanton3a`) or `system`.
    frame: String,
    /// Levels below the body or system container.
    depth: usize,
    /// The star the star map roots it at.
    system: String,
    pos: [f64; 3],
}

fn main() -> anyhow::Result<()> {
    let depth: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(2);
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || run(depth))?
        .join()
        .map_err(|_| anyhow::anyhow!("panicked"))?
}

fn stem(socpak: &str) -> String {
    socpak.rsplit(['/', '\\']).next().unwrap_or(socpak).trim_end_matches(".socpak").to_string()
}

fn run(depth: usize) -> anyhow::Result<()> {
    let install = discover_primary()?;
    let assets = AssetSource::from_install(&install)?;
    let data = AssetData::extract(&assets, &AssetConfig::standard())?;
    let datacore = Datacore::parse(&assets, &data)?;
    let t = std::time::Instant::now();
    let containers = ObjectContainers::systems(&assets, depth)?;
    eprintln!("systems({depth}): {} placements in {:?}", containers.len(), t.elapsed());
    let locations = Locations::build(datacore.records());
    // the bodies sc-nav-data reads
    let bodies: std::collections::HashSet<String> = sc_nav_data::bodies(&assets, &locations, &data.locale)?
        .into_iter()
        .map(|b| b.id)
        .collect();
    let universe = Universe::join(locations, containers);
    let containers = universe.containers();

    let mut rows = vec![];
    for (guid, location) in universe.star_map().iter() {
        let Some(place) = universe.by_guid(*guid) else { continue };
        let name = place.display_name(&data.locale).unwrap_or("?").to_string();
        let kind = format!("{:?}", location.kind);
        let system = universe
            .star_map()
            .ancestors(guid)
            .last()
            .and_then(|a| universe.by_guid(a.guid))
            .and_then(|p| p.display_name(&data.locale).map(str::to_string))
            .unwrap_or_else(|| name.clone());
        let probe = std::env::var("PLACE").is_ok_and(|p| name.contains(&p));
        if probe {
            eprintln!("== {name} ({kind}) holotable global {:?}", place.global_position());
        }
        for &id in containers.placements_of(*guid) {
            let chain = containers.chain_of(id);
            if probe {
                for &pid in &chain {
                    let p = containers.get(pid);
                    eprintln!("   {:<70} {:<40} {:?}", p.socpak(), p.entity_name(), p.position());
                }
            }
            let mut acc = [0.0f64; 3];
            let mut frame = None;
            // root first: walk up from the place itself
            let up: Vec<_> = chain.iter().rev().copied().collect();
            for (k, &pid) in up.iter().enumerate() {
                let p = containers.get(pid);
                if p.socpak().ends_with("system.socpak") {
                    if k == 0 {
                        acc = p.position().unwrap_or_default();
                        frame = Some(("system".to_string(), 0));
                    } else {
                        let container = stem(containers.get(up[k - 1]).socpak());
                        if bodies.contains(&container) {
                            frame = Some((container, k - 1));
                        } else {
                            // a Lagrange point, jump point or ring segment:
                            // fixed in the system, offsets add to its position
                            let base = p.position().unwrap_or_default();
                            for i in 0..3 {
                                acc[i] += base[i];
                            }
                            frame = Some(("system".to_string(), k));
                        }
                    }
                    break;
                }
                if let Some(pos) = p.position() {
                    for i in 0..3 {
                        acc[i] += pos[i];
                    }
                }
            }
            let Some((frame, depth)) = frame else { continue };
            rows.push(Row { name: name.clone(), kind: kind.clone(), frame, depth, system: system.clone(), pos: acc });
        }
    }
    rows.sort_by(|a, b| (&a.frame, &a.kind, &a.name).cmp(&(&b.frame, &b.kind, &b.name)));
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for r in &rows {
        let n = (r.pos[0].powi(2) + r.pos[1].powi(2) + r.pos[2].powi(2)).sqrt();
        println!(
            "{:<28} {:<10} {} {:>12.1}  {:>14.0} {:>14.0} {:>14.0}  {}",
            r.kind, r.frame, r.depth, n / 1000.0, r.pos[0], r.pos[1], r.pos[2], r.name
        );
        let frame = if r.frame == "system" { "system" } else { "body" };
        *counts.entry((r.kind.clone(), frame.to_string())).or_default() += 1;
    }
    eprintln!("\n{} rows", rows.len());
    for ((kind, frame), n) in counts {
        eprintln!("{kind:<28} {frame:<7} {n}");
    }
    std::fs::write("target/places.json", serde_json::to_vec_pretty(&rows)?)?;
    Ok(())
}
