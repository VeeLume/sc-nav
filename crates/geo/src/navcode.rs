//! Nav codes: a place as a short line of characters, with no names in it,
//! short enough to say over voice.
//!
//! **Call-out codes** are nine characters, read as three groups of three,
//! and lean on where the reader is:
//!
//! - **ground**: a spot on the planet or moon the reader is on, anywhere on
//!   it (about 1–2 m on moons, 45 m on Crusader; the height is the ground);
//! - **space**: a point within about ±200 km of the reader, to 50 m.
//!   Every further group of three multiplies the range by 32: 12 characters
//!   reach ±6,500 km, 15 reach ±210,000 km.
//!
//! The **full code** (22 characters) needs no reader, and may start with
//! its system: the three-letter tag (`PYR`) or, without names, the galactic
//! group (the cell of the system's star in the star map).
//!
//! Space points are coded by their **still position**: where they would be
//! if their planet or moon never turned (the body's centre plus the offset
//! in its own frame). Bodies never move in the game data, so a spot keeps
//! its code as planets turn; decoding finds the body by geometry (inside a
//! body's zone, the point turns with it).
//!
//! The first bit says ground or space; the last character is a check over
//! the whole position (the parts the reader filled in too) and its body or
//! system. A typo, a reader too far away, or one on another body is an
//! error, not a wrong place (31 in 32).
//!
//! Alphabet: Crockford's base 32. No I, L, O or U; when reading, O is 0
//! and I and L are 1, case and separators don't matter.

use crate::space::{norm, sub};
use crate::{Body, Geo, Location, System, V3, location::system_of_id};

pub const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Length of a call-out code.
pub const CALLOUT: usize = 9;
/// Length of a full code (without its system).
pub const FULL: usize = 22;
/// Space cells, m.
pub const CELL_M: f64 = 50.0;
/// Bits per axis of the whole grid: 2^34 cells of 50 m, ±430 million km.
const AXIS_BITS: u32 = 34;
/// A point within this many body radii of a body's centre turns with it.
///
/// Open: the game's own zone boundary isn't measured yet (a climb out of
/// an atmosphere watching `CamPos Planet Zone` disappear settles it).
pub const ZONE_RADII: f64 = 1.5;
/// The galactic grid of the system slot: star-map units, 8 per cell.
pub const GALAXY_BOX: f64 = 256.0;

/// Where the reader is: what fills in a call-out code.
#[derive(Clone, Copy, Debug, Default)]
pub struct Reader<'a> {
    /// The reader's system (`stanton`).
    pub system: &'a str,
    /// The reader's still position, for space codes.
    pub still: Option<V3>,
    /// The planet or moon the reader is on or near, for ground codes.
    pub body: Option<&'a Body>,
}

/// The still position of a location; `None` for an unknown body.
pub fn still(loc: &Location, bodies: &[Body]) -> Option<V3> {
    match loc {
        Location::Space { xyz, .. } => Some(*xyz),
        Location::Surface { body, geo } => {
            let b = bodies.iter().find(|b| &b.id == body)?;
            let v = geo.to_fixed();
            Some([b.center[0] + v[0], b.center[1] + v[1], b.center[2] + v[2]])
        }
    }
}

/// The body whose zone holds a still position in `system`.
pub fn zone_of<'a>(p: V3, system: &str, bodies: &'a [Body]) -> Option<&'a Body> {
    bodies
        .iter()
        .filter(|b| system_of_id(&b.id) == system)
        .map(|b| (b, norm(sub(p, b.center)) / b.radius_m))
        .filter(|(_, radii)| *radii <= ZONE_RADII)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(b, _)| b)
}

/// The location at a still position in `system`: on the body whose zone
/// holds it, else in space.
pub fn location(p: V3, system: &str, bodies: &[Body]) -> Location {
    match zone_of(p, system, bodies) {
        Some(b) => Location::Surface { body: b.id.clone(), geo: Geo::from_fixed(sub(p, b.center)) },
        None => Location::Space { system: system.to_string(), xyz: p },
    }
}

/// How far (along each axis) the reader may be for a space code of `len`
/// characters; `None` for a full code, which needs no reader.
pub fn reach(len: usize) -> Option<f64> {
    let k = space_bits(len);
    (k < AXIS_BITS).then(|| 2f64.powi(k as i32 - 1) * CELL_M)
}

/// Bits per axis a space code of `len` characters carries.
fn space_bits(len: usize) -> u32 {
    let bits = 5 * (len.saturating_sub(1)) as u32;
    (bits.saturating_sub(1) / 3).min(AXIS_BITS)
}

// --- the grid ---

fn grid(p: V3) -> Option<[u64; 3]> {
    let n = (1u64 << AXIS_BITS) as f64;
    let mut out = [0; 3];
    for (o, v) in out.iter_mut().zip(p) {
        let u = v / CELL_M + n / 2.0;
        if !(0.0..n).contains(&u) {
            return None;
        }
        *o = u as u64;
    }
    Some(out)
}

fn centre(c: [u64; 3]) -> V3 {
    let half = (1u64 << AXIS_BITS) as f64 / 2.0;
    c.map(|i| (i as f64 + 0.5 - half) * CELL_M)
}

/// Ground cells: longitude and latitude bits of a ground code of `len`.
fn ground_bits(len: usize) -> (u32, u32) {
    let bits = 5 * (len.saturating_sub(1)) as u32 - 1;
    (bits.div_ceil(2), bits / 2)
}

// --- characters and the check ---

fn digit(ch: char) -> Option<u8> {
    let up = match ch.to_ascii_uppercase() {
        'O' => '0',
        'I' | 'L' => '1',
        c => c,
    };
    ALPHABET.iter().position(|&a| a as char == up).map(|i| i as u8)
}

/// Luhn mod 32: catches every single wrong character and most swapped
/// neighbours.
fn luhn(seq: &[u8]) -> u8 {
    let mut sum = 0u32;
    for (i, d) in seq.iter().rev().enumerate() {
        let mut v = u32::from(*d) * if i % 2 == 0 { 2 } else { 1 };
        v = v / 32 + v % 32;
        sum += v;
    }
    ((32 - sum % 32) % 32) as u8
}

/// The check character: Luhn over a hash of what the reader supplies (the
/// body or system, the length, the start of the position it fills in) and
/// the characters sent. A typo is always caught; another body, another
/// system or a reader too far away changes the hash (31 in 32 caught).
fn check(context: &str, len: usize, filled: &[u64], sent: &[u8]) -> u8 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    context.to_ascii_lowercase().bytes().for_each(&mut eat);
    eat(len as u8);
    for v in filled {
        v.to_le_bytes().into_iter().for_each(&mut eat);
    }
    let mut seq: Vec<u8> = (0..13).map(|i| ((h >> (5 * i)) & 31) as u8).collect();
    seq.extend_from_slice(sent);
    luhn(&seq)
}

/// The characters of a value, `data_len` of them.
fn chars_of(value: u128, data_len: usize) -> Vec<u8> {
    (0..data_len).rev().map(|i| ((value >> (5 * i)) & 31) as u8).collect()
}

fn render(sent: &[u8], check: u8) -> String {
    let chars: Vec<char> = sent.iter().chain([&check]).map(|&d| ALPHABET[d as usize] as char).collect();
    chars.chunks(3).map(|g| g.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

// --- making codes ---

/// A ground code (`len` characters, 9 by default) for a spot on `body`
/// (game-frame lat/long; the height isn't kept).
pub fn ground(geo: &Geo, body: &Body, len: usize) -> String {
    let (lon_bits, lat_bits) = ground_bits(len);
    let lon = ((geo.lon / std::f64::consts::TAU + 0.5).rem_euclid(1.0) * (1u64 << lon_bits) as f64) as u64;
    let lat = (((geo.lat / std::f64::consts::PI + 0.5) * (1u64 << lat_bits) as f64) as u64).min((1 << lat_bits) - 1);
    let sent = chars_of((u128::from(lon) << lat_bits) | u128::from(lat), len - 1);
    render(&sent, check(&body.id, len, &[], &sent))
}

/// A space code of `len` characters (9, 12, 15; 22 or more is a full
/// code) for a still position in `system`. `None` outside the grid.
pub fn space(p: V3, system: &str, len: usize) -> Option<String> {
    let len = len.clamp(CALLOUT, FULL);
    let k = space_bits(len);
    let c = grid(p)?;
    let mask = (1u64 << k) - 1;
    let mut value = 1u128; // the space bit
    for axis in c {
        value = (value << k) | u128::from(axis & mask);
    }
    let spare = 5 * (len - 1) as u32 - 1 - 3 * k;
    value <<= spare;
    let sent = chars_of(value, len - 1);
    let filled = c.map(|a| a >> k);
    Some(render(&sent, check(system, len, &filled, &sent)))
}

/// A full code with its system in front: the tag (`PYR-…`), or with
/// `galactic` the galactic group (none without a galactic position).
pub fn full(p: V3, system: &System, galactic: bool) -> Option<String> {
    let slot = if galactic { galactic_group(system)? } else { system.tag() };
    Some(format!("{slot}-{}", space(p, &system.id, FULL)?))
}

/// A system's galactic group: the cell its star sits in.
pub fn galactic_group(system: &System) -> Option<String> {
    let g = system.galactic?;
    let mut out = String::new();
    for v in g {
        let u = ((v / GALAXY_BOX + 0.5) * 32.0).floor();
        if !(0.0..32.0).contains(&u) {
            return None;
        }
        out.push(ALPHABET[u as usize] as char);
    }
    Some(out)
}

// --- reading codes ---

/// Read any code where the reader is. Ground codes land on the reader's
/// body at its datum radius; space codes on the body whose zone holds them,
/// or in space; a full code may name its own system.
pub fn read(code: &str, reader: &Reader, systems: &[System], bodies: &[Body]) -> Result<Location, String> {
    let raw: Vec<char> = code.chars().filter(|c| !matches!(c, '-' | ' ' | '.' | '+' | '/')).collect();
    if raw.len() == FULL + 3 {
        let slot = raw[..3].iter().collect::<String>().to_ascii_uppercase();
        let rest: String = raw[3..].iter().collect();
        let named: Vec<&System> =
            systems.iter().filter(|s| s.tag() == slot || galactic_group(s).as_deref() == Some(slot.as_str())).collect();
        if named.is_empty() {
            return Err(format!("no system '{slot}'"));
        }
        let mut fits = vec![];
        for s in named {
            let here = Reader { system: &s.id, still: None, body: None };
            if let Ok(loc) = read(&rest, &here, systems, bodies) {
                fits.push(loc);
            }
        }
        return match fits.len() {
            1 => Ok(fits.remove(0)),
            0 => Err("check failed: a typo".into()),
            _ => Err(format!("ambiguous: '{slot}' fits more than one system")),
        };
    }
    let mut d = vec![];
    for ch in raw {
        d.push(digit(ch).ok_or_else(|| format!("'{ch}' is not a code character"))?);
    }
    let len = d.len();
    if !(5..=FULL).contains(&len) {
        return Err(format!("{len} characters: a code has 9 (or 12, 15, 22)"));
    }
    let given = d.pop().unwrap_or_default();
    let value = d.iter().fold(0u128, |v, &x| (v << 5) | u128::from(x));
    let bits = 5 * (len - 1) as u32;
    if (value >> (bits - 1)) & 1 == 0 {
        read_ground(value, &d, given, reader)
    } else {
        read_space(value, &d, given, reader, bodies)
    }
}

fn read_ground(value: u128, sent: &[u8], given: u8, reader: &Reader) -> Result<Location, String> {
    let len = sent.len() + 1;
    let body = reader.body.ok_or("a ground code is read on its planet or moon: no position yet")?;
    let (lon_bits, lat_bits) = ground_bits(len);
    let lat = (value & ((1 << lat_bits) - 1)) as u64;
    let lon = ((value >> lat_bits) & ((1 << lon_bits) - 1)) as u64;
    if check(&body.id, len, &[], sent) != given {
        return Err("check failed: a typo, or a spot on another planet or moon".into());
    }
    let geo = Geo {
        lat: ((lat as f64 + 0.5) / (1u64 << lat_bits) as f64 - 0.5) * std::f64::consts::PI,
        lon: ((lon as f64 + 0.5) / (1u64 << lon_bits) as f64 - 0.5) * std::f64::consts::TAU,
        r: body.radius_m,
    };
    Ok(Location::Surface { body: body.id.clone(), geo })
}

fn read_space(value: u128, sent: &[u8], given: u8, reader: &Reader, bodies: &[Body]) -> Result<Location, String> {
    let len = sent.len() + 1;
    let k = space_bits(len);
    let spare = 5 * (len - 1) as u32 - 1 - 3 * k;
    let v = value >> spare;
    let mask = (1u128 << k) - 1;
    let low = [(v >> (2 * k)) & mask, (v >> k) & mask, v & mask].map(|x| x as u64);
    let whole = if k >= AXIS_BITS {
        low
    } else {
        let r = reader.still.ok_or("a short space code needs your position: no fix yet")?;
        let rc = grid(r).ok_or("your position is outside the grid")?;
        let block = 1u64 << k;
        let mut out = [0; 3];
        for i in 0..3 {
            // the cell nearest the reader that ends in these bits
            let base = (rc[i] & !(block - 1)) | low[i];
            out[i] = [base.checked_sub(block), Some(base), base.checked_add(block)]
                .into_iter()
                .flatten()
                .min_by_key(|&c| c.abs_diff(rc[i]))
                .unwrap_or(base);
        }
        out
    };
    if check(reader.system, len, &whole.map(|a| a >> k), sent) != given {
        return Err(if k >= AXIS_BITS {
            "check failed: a typo, or another system".into()
        } else {
            "check failed: a typo, or you're too far away for this code".into()
        });
    }
    Ok(location(centre(whole), reader.system, bodies))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lyria() -> Body {
        Body {
            id: "stanton3a".into(),
            name: "OOC_Stanton_3a_Lyria".into(),
            center: [18_703_607_171.6, -22_121_650_133.9, 0.0],
            radius_m: 223_500.0,
            period_h: 6.4,
            atmo_km: None,
            phase_deg: None,
        }
    }

    fn arial() -> Body {
        Body { id: "stanton1b".into(), name: "OOC_Stanton_1b_Arial".into(), center: [12.89e9, -31.81e6, 0.0], radius_m: 344_500.0, ..lyria() }
    }

    fn systems() -> Vec<System> {
        let s = |id: &str, g: [f64; 3]| System { id: id.into(), galactic: Some(g) };
        vec![
            s("nyx", [20.0, 8.414866, -17.2174]),
            s("pyro", [40.12613, 8.414866, -17.2174]),
            s("stanton", [49.53472, -2.63396, 16.47529]),
        ]
    }

    fn close(a: V3, b: V3) -> f64 {
        norm(sub(a, b))
    }

    fn space_at(loc: &Location, bodies: &[Body]) -> V3 {
        still(loc, bodies).unwrap()
    }

    #[test]
    fn call_outs_are_three_groups_of_three() {
        let body = lyria();
        let g = ground(&Geo { lat: 0.3, lon: -1.2, r: 0.0 }, &body, CALLOUT);
        assert_eq!(g.len(), 11, "{g}");
        let s = space([1.0e9, 2.0e9, 3.0e5], "stanton", CALLOUT).unwrap();
        assert_eq!(s.len(), 11, "{s}");
        assert_eq!(space([1.0, 2.0, 3.0], "stanton", 12).unwrap().len(), 15);
        assert_eq!(space([1.0, 2.0, 3.0], "stanton", FULL).unwrap().len(), FULL + 7);
        // the range per length
        assert_eq!(reach(9).unwrap(), 204_800.0);
        assert_eq!(reach(12).unwrap(), 6_553_600.0);
        assert!(reach(15).unwrap() > 200e6);
        assert!(reach(FULL).is_none());
    }

    #[test]
    fn ground_codes_cover_the_whole_body() {
        let body = lyria();
        let bodies = [body.clone()];
        let reader = Reader { system: "stanton", still: None, body: Some(&body) };
        for (lat, lon) in [(-15.66, 68.29), (89.9, -179.9), (-89.9, 179.9), (0.0, 0.0), (45.0, -120.0)] {
            let geo = Geo { lat: f64::to_radians(lat), lon: f64::to_radians(lon), r: body.radius_m };
            let code = ground(&geo, &body, CALLOUT);
            let Location::Surface { body: b, geo: g } = read(&code, &reader, &[], &bodies).unwrap() else { panic!() };
            assert_eq!(b, "stanton3a");
            assert!(close(g.to_fixed(), geo.to_fixed()) < 3.0, "{code}: {}", close(g.to_fixed(), geo.to_fixed()));
        }
        // read on another moon: caught
        let other = arial();
        let elsewhere = Reader { body: Some(&other), ..reader };
        let wrong = (0..20)
            .filter(|i| {
                let geo = Geo { lat: 0.1 * *i as f64 - 1.0, lon: 0.3 * *i as f64, r: 0.0 };
                read(&ground(&geo, &body, CALLOUT), &elsewhere, &[], &bodies).is_ok()
            })
            .count();
        assert!(wrong <= 2, "{wrong} of 20 read on the wrong moon");
    }

    #[test]
    fn space_codes_fill_in_from_the_reader() {
        let spot = [18.7036e9, -22.12165e9, 410_000.0];
        for len in [9, 12, 15] {
            let away = reach(len).unwrap() * 0.9;
            let code = space(spot, "stanton", len).unwrap();
            let reader = Reader { system: "stanton", still: Some([spot[0] + away, spot[1] - away * 0.7, spot[2] - away * 0.3]), body: None };
            let back = space_at(&read(&code, &reader, &[], &[]).unwrap(), &[]);
            assert!(close(spot, back) <= CELL_M, "{len}: {code}");
        }
    }

    #[test]
    fn a_reader_too_far_away_is_told() {
        // readers whole blocks off (each fills in a different wrong start),
        // along every axis: about one in 32 slips through
        let spot = [18.7036e9, -22.12165e9, 410_000.0];
        let code = space(spot, "stanton", 9).unwrap();
        let block = 2.0 * reach(9).unwrap();
        let mut wrong = 0;
        for n in 1..=64 {
            for axis in 0..3 {
                let mut r = spot;
                r[axis] += n as f64 * block * if n % 2 == 0 { 1.0 } else { -1.0 } + 1_000.0;
                let reader = Reader { system: "stanton", still: Some(r), body: None };
                wrong += usize::from(read(&code, &reader, &[], &[]).is_ok());
            }
        }
        assert!(wrong <= 14, "{wrong} of 192 readers got a wrong place");
    }

    #[test]
    fn typos_fail_and_lookalikes_are_forgiven() {
        let code = space([18.7e9, -22.1e9, 1234.5], "stanton", FULL).unwrap();
        let reader = Reader { system: "stanton", ..Default::default() };
        let chars: Vec<char> = code.chars().collect();
        for i in (0..chars.len()).filter(|&i| chars[i] != '-') {
            for &a in ALPHABET {
                let a = a as char;
                if a == chars[i] {
                    continue;
                }
                let mut typo = chars.clone();
                typo[i] = a;
                let typo: String = typo.into_iter().collect();
                assert!(read(&typo, &reader, &[], &[]).is_err(), "{typo} passed");
            }
        }
        let sloppy = code.to_lowercase().replace('-', " ").replace('0', "o").replace('1', "l");
        assert_eq!(read(&sloppy, &reader, &[], &[]), read(&code, &reader, &[], &[]));
        assert!(read(&code, &Reader { system: "pyro", ..reader }, &[], &[]).is_err());
    }

    #[test]
    fn full_codes_name_their_system_either_way() {
        let systems = systems();
        let groups: Vec<_> = systems.iter().map(|s| galactic_group(s).unwrap()).collect();
        assert!(groups[0] != groups[1] && groups[1] != groups[2] && groups[0] != groups[2], "{groups:?}");
        let p = [1.2e9, -3.4e9, 56_789.0];
        let pyro = &systems[1];
        let in_stanton = Reader { system: "stanton", ..Default::default() };
        for code in [full(p, pyro, false).unwrap(), full(p, pyro, true).unwrap()] {
            let back = read(&code, &in_stanton, &systems, &[]).unwrap();
            let Location::Space { system, xyz } = back else { panic!() };
            assert_eq!(system, "pyro", "{code}");
            assert!(close(p, xyz) <= CELL_M);
        }
        let tagged = full(p, pyro, false).unwrap();
        assert!(tagged.starts_with("PYR-"), "{tagged}");
        assert!(read(&tagged.replacen("PYR", "STA", 1), &in_stanton, &systems, &[]).is_err());
        assert!(read(&tagged.replacen("PYR", "XYZ", 1), &in_stanton, &systems, &[]).is_err());
    }

    #[test]
    fn space_spots_near_a_body_turn_with_it() {
        let body = lyria();
        let bodies = [body.clone()];
        // a rock 100 km above Lyria's surface, coded by its still position
        let rock = Location::Surface { body: body.id.clone(), geo: Geo { lat: 0.2, lon: 1.1, r: body.radius_m + 100_000.0 } };
        let p = still(&rock, &bodies).unwrap();
        let code = space(p, "stanton", 9).unwrap();
        let reader = Reader { system: "stanton", still: Some([p[0] + 50_000.0, p[1], p[2]]), body: Some(&body) };
        let Location::Surface { body: b, geo } = read(&code, &reader, &[], &bodies).unwrap() else { panic!() };
        assert_eq!(b, body.id);
        assert!((geo.r - (body.radius_m + 100_000.0)).abs() < CELL_M);
        assert!(matches!(location([1e9, 1e9, 0.0], "stanton", &bodies), Location::Space { .. }));
    }
}
