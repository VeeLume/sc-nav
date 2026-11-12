//! sc-nav's game data, read from the Star Citizen install at runtime through
//! sc-holotable, so a patch brings its own numbers instead of breaking stale
//! ones.
//!
//! - [`bodies`]: every planet and moon with centre, radius, day length and
//!   atmosphere (`<system>system.socpak` nests `<Child>` nodes with a `pos`
//!   relative to their parent; each body's socpak has `planetRadius` /
//!   `planetRotationSpeed` / `planetAxis`, its `.ale` the `AtmoHeight`).
//!   A few socpak reads: done at every start.
//! - [`ships`]: braking and speeds per ship from the full DataCore parse:
//!   seconds of work, so cached per game build under the caller's directory.
//! - [`overlay_font`]: `Engine/Fonts/VeraMono.ttf`, the `r_DisplayInfo` font.
//!
//! [`load`] does all three.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context, Result};
use sc_holotable::asset::object_container::{Socpak, XmlNode, decode};
use sc_holotable::asset::{AssetConfig, AssetData, AssetSource, Datacore, Guid, RecordCollection};
use sc_holotable::install::{Installation, discover_primary};
use sc_holotable::items::Items;
use sc_holotable::ship_components::ShipComponents;
use sc_holotable::vehicles::{Flight, PortContainers, Vehicles, port_axes};
use sc_nav_geo::{Body, Ship, System, World};

/// Bumped when [`World`] gains what an older cache lacks.
const CACHE: u32 = 2;

/// The world and the overlay font from the primary install. Everything is
/// cached in `cache_dir` per exact build (`world-<channel>-<version>.json` and
/// `font-….ttf`): the first start after a patch reads the game files (about
/// two minutes, the ships' DataCore walk), later starts read the cache.
pub fn load(cache_dir: &Path) -> Result<(World, Vec<u8>)> {
    let install = discover_primary().context("finding the Star Citizen install")?;
    let key = cache_key(&install);
    let (world_file, font_file) = (
        cache_dir.join(format!("world{CACHE}-{key}.json")),
        cache_dir.join(format!("font-{key}.ttf")),
    );
    if let (Some(world), Ok(font)) = (
        std::fs::read(&world_file)
            .ok()
            .and_then(|b| serde_json::from_slice::<World>(&b).ok()),
        std::fs::read(&font_file),
    ) {
        return Ok((world, font));
    }
    let assets = AssetSource::from_install(&install).context("opening Data.p4k")?;
    let bodies = bodies(&assets)?;
    let font = overlay_font(&assets)?;
    let (ships, systems) = records(&install)?;
    let world = World {
        build: build_of(&install),
        bodies,
        ships,
        systems,
    };
    std::fs::create_dir_all(cache_dir)?;
    std::fs::write(&world_file, serde_json::to_vec(&world)?)?;
    std::fs::write(&font_file, &font)?;
    Ok((world, font))
}

/// Channel and the manifest's full version: one key per exact build.
pub fn cache_key(install: &Installation) -> String {
    format!("{}-{}", install.channel, install.manifest.version).replace(
        |c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-',
        "_",
    )
}

/// The build as a person reads it: the launcher's label (`4.10.2-live.…`)
/// when known, else channel and the manifest's major.minor.
pub fn build_of(install: &Installation) -> String {
    match &install.launcher_version_label {
        Some(label) => format!("{} {label}", install.channel),
        None => format!("{} {}", install.channel, install.short_version()),
    }
}

/// `Engine/Fonts/VeraMono.ttf` from the game files.
pub fn overlay_font(assets: &AssetSource) -> Result<Vec<u8>> {
    let entry = assets
        .find(|n| {
            n.to_ascii_lowercase()
                .replace('\\', "/")
                .ends_with("engine/fonts/veramono.ttf")
        })
        .next()
        .context("VeraMono.ttf not in Data.p4k")?;
    Ok(assets.read_entry(&entry)?)
}

/// Every ship's flight figures with its stock loadout. Runs on its own
/// thread with a large stack: the DataCore walk overflows Windows' 1 MB main
/// stack.
pub fn records(install: &Installation) -> Result<(BTreeMap<String, Ship>, Vec<System>)> {
    let install = install.clone();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || records_inner(&AssetSource::from_install(&install)?))?
        .join()
        .map_err(|_| anyhow::anyhow!("DataCore extraction panicked"))?
}

fn records_inner(assets: &AssetSource) -> Result<(BTreeMap<String, Ship>, Vec<System>)> {
    let data = AssetData::extract(assets, &AssetConfig::standard())?;
    let datacore = Datacore::parse(assets, &data)?;
    Ok((ships(assets, &datacore)?, systems(&datacore)))
}

/// The star systems and their galactic positions (`SSolarSystem`). The id is
/// the record's `name` lowercased, which is the object containers' folder
/// (`stanton`, `pyro`, `nyx`): the id bodies carry.
pub fn systems(datacore: &Datacore) -> Vec<System> {
    let store = datacore.records();
    let pools = &store.pools;
    let mut out: Vec<System> = store
        .records
        .multi_feature
        .ssolar_system
        .values()
        .filter_map(|h| h.get(pools))
        .map(|s| System {
            id: s.name.to_ascii_lowercase(),
            galactic: s.galactic_position.and_then(|h| h.get(pools)).map(|v| [v.x as f64, v.y as f64, v.z as f64]),
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn ships(assets: &AssetSource, datacore: &Datacore) -> Result<BTreeMap<String, Ship>> {
    let items = Items::build(datacore.records());
    let containers = PortContainers::build(datacore, assets);
    let vehicles = Vehicles::build(datacore, &items, &containers, assets);
    let components = ShipComponents::build(datacore, &items);
    let names: HashMap<Guid, String> = datacore
        .db()
        .all_records()
        .filter(|r| r.type_name() == Some("EntityClassDefinition"))
        .filter_map(|r| Some((r.id(), r.name()?.rsplit('.').next()?.to_string())))
        .collect();
    let mut ships = BTreeMap::new();
    for (guid, ship) in vehicles.iter() {
        let Some(name) = names.get(guid) else {
            continue;
        };
        let axes = port_axes(assets, ship);
        let Some(f) = Flight::compute(ship, &ship.stock, &vehicles, &items, &components, &axes)
        else {
            continue;
        };
        ships.insert(
            name.clone(),
            Ship {
                mass_kg: f.mass,
                forward_g: f.acceleration.forward,
                backward_g: f.acceleration.backward,
                backward_boost_g: f.acceleration_boosted.backward,
                scm_speed: f.scm_speed,
                max_speed: f.max_speed,
            },
        );
    }
    Ok(ships)
}

/// Every planet and moon (rotating about +Z; others are skipped).
pub fn bodies(assets: &AssetSource) -> Result<Vec<Body>> {
    let systems: Vec<String> = assets
        .find(|n| {
            let l = n.to_ascii_lowercase().replace('\\', "/");
            l.contains("/pu/system/") && l.ends_with("system.socpak")
        })
        .map(|e| e.name.to_string())
        .collect();

    let mut placed = Vec::new();
    for sp in &systems {
        let mut pak = Socpak::open(assets.read(sp)?)?;
        for m in 0..pak.len() {
            let member = pak.name(m).unwrap_or_default().to_ascii_lowercase();
            if !member.ends_with("system.xml") {
                continue;
            }
            let Ok(Some(root)) = decode(&pak.read(m)?) else {
                continue;
            };
            walk(&root, [0.0; 3], &mut placed);
        }
    }

    let mut out = Vec::new();
    for (path, name, center) in placed {
        match body_params(assets, &path) {
            Ok(Some((id, radius_m, period_h, axis, atmo_km))) => {
                if axis != [0.0, 0.0, 1.0] {
                    continue;
                }
                out.push(Body {
                    id,
                    name,
                    center,
                    radius_m,
                    period_h,
                    atmo_km,
                    phase_deg: None,
                });
            }
            Ok(None) => {}
            Err(_) => {}
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Collect every `<Child>` pointing at a socpak directly under `pu/system/<sys>/`,
/// with its position summed down the nesting.
fn walk(node: &XmlNode, base: [f64; 3], out: &mut Vec<(String, String, [f64; 3])>) {
    for child in &node.children {
        let mut here = base;
        if child.tag == "Child" {
            if let Some(p) = child.attr("pos").and_then(vec3) {
                here = [base[0] + p[0], base[1] + p[1], base[2] + p[2]];
            }
            if let Some(name) = child.attr("name") {
                let l = name.to_ascii_lowercase();
                let direct = l
                    .strip_prefix("data/objectcontainers/pu/system/")
                    .is_some_and(|r| r.matches('/').count() == 1);
                if direct && l.ends_with(".socpak") {
                    let label = child.attr("entityName").unwrap_or(name).to_string();
                    out.push((name.to_string(), label, here));
                }
            }
        }
        walk(child, here, out);
    }
}

type Params = (String, f64, f64, [f64; 3], Option<f64>);

fn body_params(assets: &AssetSource, path: &str) -> Result<Option<Params>> {
    let want = path.to_ascii_lowercase().replace('\\', "/");
    let entry = assets
        .find(|n| n.to_ascii_lowercase().replace('\\', "/") == want)
        .map(|e| e.name.to_string())
        .next();
    let Some(entry) = entry else { return Ok(None) };
    let id = want
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim_end_matches(".socpak")
        .to_string();

    let mut pak = Socpak::open(assets.read(&entry)?)?;
    let (mut params, mut atmo) = (None, None);
    for m in 0..pak.len() {
        let member = pak
            .name(m)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .replace('\\', "/");
        let file = member.rsplit('/').next().unwrap_or("");
        if file == format!("{id}.xml") {
            let Ok(Some(root)) = decode(&pak.read(m)?) else {
                continue;
            };
            params = root.descendants().find_map(|n| {
                let period: f64 = n.attr("planetRotationSpeed")?.parse().ok()?;
                let radius: f64 = n.attr("planetRadius")?.parse().ok()?;
                let axis = n.attr("planetAxis").and_then(vec3)?;
                Some((radius, period, axis))
            });
        } else if file == format!("{id}.ale") {
            atmo = atmo_height(&pak.read(m)?);
        }
    }
    Ok(params
        .filter(|(_, period, _)| *period > 0.0)
        .map(|(radius, period, axis)| (id, radius, period, axis, atmo)))
}

/// `AtmoHeight` from the JSON embedded in a `.ale` chunk file.
fn atmo_height(bytes: &[u8]) -> Option<f64> {
    let key = b"\"AtmoHeight\"";
    let at = bytes.windows(key.len()).position(|w| w == key)? + key.len();
    let rest = bytes.get(at..at + 40)?;
    let text = String::from_utf8_lossy(rest);
    let value = text.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    let end = value.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))?;
    value[..end].parse().ok()
}

fn vec3(s: &str) -> Option<[f64; 3]> {
    let mut it = s.split(',').map(|v| v.trim().parse::<f64>());
    let v = [it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?];
    it.next().is_none().then_some(v)
}
