//! A corrected wall clock. [`now_ms`] is the PC clock plus an offset measured
//! with SNTP (RFC 4330) against public time servers, so maths that depends on
//! absolute time (a planet's spin angle at a `/showlocation` fix) doesn't
//! inherit a drifting PC clock. Until the first good answer the offset is 0.
//!
//! Standard library only: one UDP packet per query, nothing about the user in
//! it. An answer counts only if it echoes our transmit time, says it is a
//! server (mode 4), is synchronised (leap indicator not 3, stratum 1..=15) and
//! came back within [`MAX_RTT_MS`]. Of each round, the answer with the
//! shortest round trip wins: its offset error is at most half its round trip.

use std::io;
use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Queried in turn each round.
pub const SERVERS: &[&str] = &[
    "time.cloudflare.com",
    "pool.ntp.org",
    "time.google.com",
    "time.windows.com",
];
/// Answers slower than this are discarded.
pub const MAX_RTT_MS: f64 = 500.0;
/// Between rounds.
pub const INTERVAL: Duration = Duration::from_secs(300);

/// Seconds from 1900 (NTP) to 1970 (Unix).
const NTP_UNIX: u64 = 2_208_988_800;

/// One answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Server time minus PC time, ms.
    pub offset_ms: f64,
    /// Round trip less the server's own processing, ms.
    pub rtt_ms: f64,
    pub stratum: u8,
}

/// Where the clock stands, for display.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub offset_ms: Option<f64>,
    pub rtt_ms: Option<f64>,
    pub server: Option<String>,
    pub synced_at: Option<Instant>,
    /// The last round's failure, if it found no usable answer.
    pub error: Option<String>,
}

static OFFSET_US: AtomicI64 = AtomicI64::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static WAKE: AtomicBool = AtomicBool::new(false);

fn status_cell() -> &'static Mutex<Status> {
    static S: OnceLock<Mutex<Status>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Status::default()))
}

/// The PC clock, Unix ms, uncorrected.
pub fn pc_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Corrected Unix ms: the PC clock plus the measured offset (0 until synced).
pub fn now_ms() -> i64 {
    (pc_ms() + OFFSET_US.load(Ordering::Relaxed) as f64 / 1000.0).round() as i64
}

/// The offset in use, ms (0 until synced).
pub fn offset_ms() -> f64 {
    OFFSET_US.load(Ordering::Relaxed) as f64 / 1000.0
}

pub fn status() -> Status {
    status_cell().lock().map(|s| s.clone()).unwrap_or_default()
}

/// Start the background sync (once per process; later calls do nothing).
pub fn start() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        loop {
            sync_round();
            let until = Instant::now() + INTERVAL;
            while Instant::now() < until && !WAKE.swap(false, Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    });
}

/// Ask the background thread for a round now.
pub fn sync_now() {
    WAKE.store(true, Ordering::SeqCst);
}

/// Query every server once and apply the best answer.
pub fn sync_round() -> Option<(String, Sample)> {
    let mut best: Option<(String, Sample)> = None;
    let mut errors = vec![];
    for server in SERVERS {
        match query(server, Duration::from_millis(1500)) {
            Ok(s) if s.rtt_ms <= MAX_RTT_MS => {
                if best.as_ref().is_none_or(|(_, b)| s.rtt_ms < b.rtt_ms) {
                    best = Some((server.to_string(), s));
                }
            }
            Ok(s) => errors.push(format!("{server}: round trip {:.0} ms", s.rtt_ms)),
            Err(e) => errors.push(format!("{server}: {e}")),
        }
    }
    let mut st = status_cell().lock().ok()?;
    match &best {
        Some((server, s)) => {
            OFFSET_US.store((s.offset_ms * 1000.0).round() as i64, Ordering::Relaxed);
            *st = Status {
                offset_ms: Some(s.offset_ms),
                rtt_ms: Some(s.rtt_ms),
                server: Some(server.clone()),
                synced_at: Some(Instant::now()),
                error: None,
            };
        }
        None => st.error = Some(errors.join("; ")),
    }
    best
}

/// One SNTP query.
pub fn query(server: &str, timeout: Duration) -> io::Result<Sample> {
    let addr = (server, 123)
        .to_socket_addrs()?
        .find(|a| a.is_ipv4())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no IPv4 address"))?;
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(timeout))?;
    sock.connect(addr)?;
    let t1 = pc_ms();
    let req = request(t1);
    sock.send(&req)?;
    let mut buf = [0u8; 128];
    let n = sock.recv(&mut buf)?;
    let t4 = pc_ms();
    parse(&buf[..n], &req[40..48], t1, t4)
}

/// A client request carrying `t1` as its transmit timestamp.
fn request(t1_ms: f64) -> [u8; 48] {
    let mut p = [0u8; 48];
    p[0] = 0b00_100_011; // LI 0, version 4, mode 3 (client)
    p[40..48].copy_from_slice(&to_ntp(t1_ms));
    p
}

/// Check a reply and work out offset and round trip (RFC 4330 §5).
fn parse(p: &[u8], sent: &[u8], t1: f64, t4: f64) -> io::Result<Sample> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    if p.len() < 48 {
        return Err(bad("short reply"));
    }
    let (li, mode, stratum) = (p[0] >> 6, p[0] & 0b111, p[1]);
    if mode != 4 {
        return Err(bad("not a server reply"));
    }
    if li == 3 || stratum == 0 || stratum > 15 {
        return Err(bad("server not synchronised"));
    }
    if &p[24..32] != sent {
        return Err(bad("reply doesn't answer our request"));
    }
    let t2 = from_ntp(&p[32..40]);
    let t3 = from_ntp(&p[40..48]);
    if t2 == 0.0 || t3 == 0.0 {
        return Err(bad("empty timestamps"));
    }
    Ok(Sample {
        offset_ms: ((t2 - t1) + (t3 - t4)) / 2.0,
        rtt_ms: (t4 - t1) - (t3 - t2),
        stratum,
    })
}

fn to_ntp(unix_ms: f64) -> [u8; 8] {
    let secs = (unix_ms / 1000.0).floor();
    let frac = ((unix_ms / 1000.0 - secs) * 4_294_967_296.0) as u32;
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&((secs as u64 + NTP_UNIX) as u32).to_be_bytes());
    b[4..].copy_from_slice(&frac.to_be_bytes());
    b
}

fn from_ntp(b: &[u8]) -> f64 {
    let secs = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64;
    let frac = u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as f64 / 4_294_967_296.0;
    if secs == 0.0 && frac == 0.0 {
        return 0.0;
    }
    (secs - NTP_UNIX as f64 + frac) * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server 250 ms ahead, 20 ms each way, 2 ms inside: offset 250, rtt 40.
    fn reply(sent: &[u8], t1: f64) -> Vec<u8> {
        let mut p = vec![0u8; 48];
        p[0] = 0b00_100_100;
        p[1] = 2;
        p[24..32].copy_from_slice(sent);
        p[32..40].copy_from_slice(&to_ntp(t1 + 20.0 + 250.0));
        p[40..48].copy_from_slice(&to_ntp(t1 + 22.0 + 250.0));
        p
    }

    #[test]
    fn offset_and_round_trip() {
        let t1 = 1_794_400_000_123.0;
        let req = request(t1);
        let s = parse(&reply(&req[40..48], t1), &req[40..48], t1, t1 + 42.0).unwrap();
        assert!((s.offset_ms - 250.0).abs() < 0.01, "{s:?}");
        assert!((s.rtt_ms - 40.0).abs() < 0.01, "{s:?}");
    }

    #[test]
    fn refuses_bad_replies() {
        let t1 = 1_794_400_000_123.0;
        let req = request(t1);
        let good = reply(&req[40..48], t1);
        let mut other = good.clone();
        other[24] ^= 1; // answers someone else's request
        assert!(parse(&other, &req[40..48], t1, t1 + 42.0).is_err());
        let mut unsync = good.clone();
        unsync[0] = 0b11_100_100; // leap indicator 3
        assert!(parse(&unsync, &req[40..48], t1, t1 + 42.0).is_err());
        let mut client = good.clone();
        client[0] = 0b00_100_011; // mode 3
        assert!(parse(&client, &req[40..48], t1, t1 + 42.0).is_err());
        assert!(parse(&good[..40], &req[40..48], t1, t1 + 42.0).is_err());
    }

    #[test]
    fn ntp_timestamps_round_trip() {
        let ms = 1_794_400_000_123.456;
        assert!((from_ntp(&to_ntp(ms)) - ms).abs() < 0.001);
    }

    /// Against the real servers: `cargo test -p sc-clock -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_servers() {
        for server in SERVERS {
            match query(server, Duration::from_millis(1500)) {
                Ok(s) => println!(
                    "{server}: offset {:+.1} ms, rtt {:.1} ms, stratum {}",
                    s.offset_ms, s.rtt_ms, s.stratum
                ),
                Err(e) => println!("{server}: {e}"),
            }
        }
        let best = sync_round();
        println!(
            "best: {best:?}, now_ms - pc_ms = {:+.1}",
            now_ms() as f64 - pc_ms()
        );
        assert!(best.is_some());
    }
}
