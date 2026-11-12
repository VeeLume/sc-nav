//! Locations that span planets and space.
//!
//! A [`Location`] is either a point in a body's own frame (lat/long, of which
//! SCAR is a planet-local notation) or a point fixed in a system's space
//! (system frame, the `Root` frame). Surface points turn around with their
//! body; space points don't. [`Location::to_system`] and [`surface_of`]
//! convert between the two at a moment in time; [`Location::code`] /
//! [`Location::parse_code`] carry either as one line of text.

use serde::{Deserialize, Serialize};

use crate::space::{norm, sub};
use crate::{Body, Geo, V3};

/// A place to navigate to. Untagged, so a stored surface mark is just
/// `{"body", "geo"}` and a space mark `{"system", "xyz"}`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Location {
    /// On or above a body, in its own (rotating) frame.
    Surface { body: String, geo: Geo },
    /// Fixed in a system's space, metres; `system` is e.g. `stanton`.
    Space { system: String, xyz: V3 },
}

/// The system of a body id: `stanton1a` -> `stanton`.
pub fn system_of_id(id: &str) -> String {
    id.chars().take_while(|c| c.is_ascii_alphabetic()).collect()
}

/// The system a system-frame point belongs to: that of the nearest body
/// surface. Systems' coordinates overlap, so prefer a system the overlay
/// names when there is one.
pub fn system_near(bodies: &[Body], p: V3) -> Option<String> {
    bodies
        .iter()
        .min_by(|a, b| {
            (norm(sub(p, a.center)) - a.radius_m).total_cmp(&(norm(sub(p, b.center)) - b.radius_m))
        })
        .map(|b| system_of_id(&b.id))
}

/// The surface location under a system-frame point at `unix_ms`, on the
/// nearest body within four radii. `None` without a body or its phase.
pub fn surface_of(bodies: &[Body], xyz: V3, unix_ms: i64) -> Option<Location> {
    let body = crate::nearest_body(bodies, xyz)?;
    let game = body.epoch_to_game(body.to_fixed(xyz, unix_ms))?;
    Some(Location::Surface {
        body: body.id.clone(),
        geo: Geo::from_fixed(game),
    })
}

impl Location {
    /// The body id or the system it belongs to.
    pub fn place(&self) -> &str {
        match self {
            Location::Surface { body, .. } => body,
            Location::Space { system, .. } => system,
        }
    }

    /// The system either way.
    pub fn system(&self) -> String {
        match self {
            Location::Surface { body, .. } => system_of_id(body),
            Location::Space { system, .. } => system.clone(),
        }
    }

    /// A surface point from game-frame lat/long (degrees), at the body's
    /// radius. `None` for an unknown body or out-of-range angles.
    pub fn surface(bodies: &[Body], body_id: &str, lat: f64, lon: f64) -> Option<Location> {
        let b = bodies.iter().find(|b| b.id == body_id)?;
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=360.0).contains(&lon) {
            return None;
        }
        Some(Location::Surface {
            body: b.id.clone(),
            geo: Geo {
                lat: lat.to_radians(),
                lon: lon.to_radians(),
                r: b.radius_m,
            },
        })
    }

    /// Lat/long in degrees (lon -180..180) of a surface point.
    pub fn lat_lon(&self) -> Option<(f64, f64)> {
        match self {
            Location::Surface { geo, .. } => Some((
                geo.lat.to_degrees(),
                (geo.lon.to_degrees() + 180.0).rem_euclid(360.0) - 180.0,
            )),
            Location::Space { .. } => None,
        }
    }

    /// SCAR of a surface point (planet-local).
    pub fn scar(&self) -> Option<String> {
        self.lat_lon().map(|(lat, lon)| crate::scar::scar(lat, lon))
    }

    /// The point in the system frame at `unix_ms`: a surface point turned
    /// with its body (needs the body's phase), a space point as is.
    pub fn to_system(&self, bodies: &[Body], unix_ms: i64) -> Option<V3> {
        match self {
            Location::Space { xyz, .. } => Some(*xyz),
            Location::Surface { body, geo } => {
                let b = bodies.iter().find(|b| &b.id == body)?;
                b.game_to_global(geo.to_fixed(), unix_ms)
            }
        }
    }

    /// One line to share: `SCNAV1;S;<body>;<lat>;<lon>;<name>` or
    /// `SCNAV1;P;<system>;<x>;<y>;<z>;<name>` (metres, system frame).
    pub fn code(&self, name: &str) -> String {
        match self {
            Location::Surface { body, .. } => {
                let (lat, lon) = self.lat_lon().unwrap_or_default();
                format!("SCNAV1;S;{body};{lat:.6};{lon:.6};{name}")
            }
            Location::Space { system, xyz } => format!(
                "SCNAV1;P;{system};{:.1};{:.1};{:.1};{name}",
                xyz[0], xyz[1], xyz[2]
            ),
        }
    }

    /// The reverse of [`Self::code`]: name and location. Surface codes need
    /// the body (for its radius).
    pub fn parse_code(code: &str, bodies: &[Body]) -> Result<(String, Location), String> {
        let f: Vec<&str> = code.trim().split(';').collect();
        let num = |s: &str| {
            s.trim()
                .parse::<f64>()
                .map_err(|_| format!("not a number: {s}"))
        };
        match f.as_slice() {
            ["SCNAV1", "S", body, lat, lon, name @ ..] => {
                let loc = Location::surface(bodies, body, num(lat)?, num(lon)?)
                    .ok_or_else(|| format!("unknown body {body} or lat/long out of range"))?;
                Ok((name.join(";"), loc))
            }
            ["SCNAV1", "P", system, x, y, z, name @ ..] => {
                let xyz = [num(x)?, num(y)?, num(z)?];
                if xyz.iter().any(|v| !v.is_finite() || v.abs() > 1e13) {
                    return Err("coordinates out of range".into());
                }
                Ok((
                    name.join(";"),
                    Location::Space {
                        system: system.to_lowercase(),
                        xyz,
                    },
                ))
            }
            _ => Err("not a mark code (SCNAV1;S;... or SCNAV1;P;...)".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lyria() -> Body {
        Body {
            id: "stanton3a".into(),
            name: "OOC_Stanton_3a_Lyria".into(),
            center: [18_778_498.0, -21_918_664.0, 0.0],
            radius_m: 223_000.0,
            period_h: 6.43,
            atmo_km: Some(15.0),
            phase_deg: Some(175.37),
        }
    }

    #[test]
    fn codes_round_trip_and_old_marks_load() {
        let bodies = [lyria()];
        for loc in [
            Location::Space {
                system: "stanton".into(),
                xyz: [12_892_406_889.4, -31_460_113.7, -222_064.1],
            },
            Location::surface(&bodies, "stanton3a", -15.63768, -107.39331).unwrap(),
        ] {
            let (name, back) = Location::parse_code(&loc.code("Rock; field"), &bodies).unwrap();
            assert_eq!(name, "Rock; field");
            assert_eq!(back.place(), loc.place());
        }
        let old: Location = serde_json::from_str(
            r#"{"body":"stanton3a","geo":{"lat":-0.27,"lon":-1.87,"r":224081.5}}"#,
        )
        .unwrap();
        assert!(matches!(old, Location::Surface { .. }));
        assert!(Location::parse_code("hello", &bodies).is_err());
    }

    /// A surface point turned into space and back lands on itself, and the
    /// system comes from the body id.
    #[test]
    fn surface_to_system_and_back() {
        let bodies = [lyria()];
        let loc = Location::surface(&bodies, "stanton3a", -15.6, -107.4).unwrap();
        let ms = 1_794_400_000_000;
        let xyz = loc.to_system(&bodies, ms).unwrap();
        let back = surface_of(&bodies, xyz, ms).unwrap();
        let (a, b) = (loc.lat_lon().unwrap(), back.lat_lon().unwrap());
        assert!((a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
        assert_eq!(loc.system(), "stanton");
        assert_eq!(system_near(&bodies, xyz).as_deref(), Some("stanton"));
    }
}
