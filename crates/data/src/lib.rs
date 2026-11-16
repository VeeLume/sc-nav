//! sc-nav's game data, read from the Star Citizen install at runtime through
//! sc-holotable, so a patch brings its own numbers instead of breaking stale
//! ones.
//!
//! - [`bodies`]: every planet and moon with centre, radius, day length and
//!   atmosphere, from holotable's `Bodies`.
//! - ships: braking and speeds per ship.
//!
//! Both come from the full DataCore parse ([`records`]): seconds of work, so
//! cached per game build under the caller's directory.
//! - [`overlay_font`]: `Engine/Fonts/VeraMono.ttf`, the `r_DisplayInfo` font.
//!
//! [`load`] does all three.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context, Result};
use sc_holotable::asset::{
    AssetConfig, AssetData, AssetSource, Datacore, Guid, LocaleMap, RecordCollection,
};
use sc_holotable::install::{Installation, discover_primary};
use sc_holotable::items::Items;
use sc_holotable::locations::{Bodies, Locations};
use sc_holotable::ship_components::ShipComponents;
use sc_holotable::vehicles::{Flight, PortContainers, Vehicles, port_axes};
use sc_nav_geo::{Body, Ship, System, World};

/// Bumped when [`World`] gains what an older cache lacks, or its values
/// change (3: bodies from holotable, microTech's moons placed right).
const CACHE: u32 = 3;

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
    let font = overlay_font(&assets)?;
    let Records { bodies, ships, systems } = records(&install)?;
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

/// What the DataCore parse yields.
pub struct Records {
    pub bodies: Vec<Body>,
    pub ships: BTreeMap<String, Ship>,
    pub systems: Vec<System>,
}

/// Bodies, every ship's flight figures with its stock loadout, and the
/// systems. Runs on its own thread with a large stack: the DataCore walk
/// overflows Windows' 1 MB main stack.
pub fn records(install: &Installation) -> Result<Records> {
    let install = install.clone();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || records_inner(&AssetSource::from_install(&install)?))?
        .join()
        .map_err(|_| anyhow::anyhow!("DataCore extraction panicked"))?
}

fn records_inner(assets: &AssetSource) -> Result<Records> {
    let data = AssetData::extract(assets, &AssetConfig::standard())?;
    let datacore = Datacore::parse(assets, &data)?;
    let locations = Locations::build(datacore.records());
    Ok(Records {
        bodies: bodies(assets, &locations, &data.locale)?,
        ships: ships(assets, &datacore)?,
        systems: systems(&datacore),
    })
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

/// Every planet and moon from holotable's [`Bodies`], keyed by the
/// container's short id (`stanton1a`: nav codes and marks carry it). Bodies
/// spinning about another axis than +Z are skipped (the frames assume +Z).
/// Fuego's period is 0: it does not turn, see [`Body::spin`].
pub fn bodies(assets: &AssetSource, locations: &Locations, locale: &LocaleMap) -> Result<Vec<Body>> {
    let mut out: Vec<Body> = Bodies::build(assets, locations)?
        .values()
        .filter(|b| b.axis == [0.0, 0.0, 1.0])
        .map(|b| Body {
            id: b.short_id().to_string(),
            name: locations
                .get(&b.guid)
                .and_then(|l| l.display_name(locale))
                .unwrap_or(b.short_id())
                .to_string(),
            center: b.position,
            radius_m: b.radius,
            period_h: b.rotation_period_h,
            atmo_km: b.atmosphere_height.map(|m| m / 1000.0),
            phase_deg: None,
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}
