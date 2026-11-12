//! Ground proximity: is the ship sinking faster than it can stop above the
//! terrain? Independent of any target. Height above ground comes from the
//! overlay's `Altitude` (printed even from space) or the radar altitude when
//! low; sink rate from telemetry's vertical speed, else from successive
//! readings.

/// How urgent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroundLevel {
    /// Impact within half a minute, or stopping takes half the height left.
    Caution,
    /// Stopping (after the reaction time) takes the height left: brake now.
    PullUp,
}

#[derive(Debug, Clone, Copy)]
pub struct GroundWarning {
    /// Height above ground, m.
    pub agl: f64,
    /// Sink rate, m/s (positive down).
    pub sink: f64,
    pub time_to_impact_s: f64,
    /// Height needed to stop the sink, m.
    pub stop_m: f64,
    pub level: GroundLevel,
}

/// `brake` and `gravity` in m/s²: stopping a sink works against gravity.
pub fn ground_warning(
    agl: f64,
    sink: f64,
    brake: f64,
    gravity: f64,
    reaction_s: f64,
) -> Option<GroundWarning> {
    if sink < 5.0 || agl <= 0.0 || !agl.is_finite() {
        return None;
    }
    let decel = (brake - gravity).max(0.5);
    let stop_m = sink * sink / (2.0 * decel) + sink * reaction_s;
    let time_to_impact_s = agl / sink;
    let level = if stop_m >= agl * 0.9 {
        GroundLevel::PullUp
    } else if time_to_impact_s < 30.0 || stop_m * 2.0 >= agl {
        GroundLevel::Caution
    } else {
        return None;
    };
    Some(GroundWarning {
        agl,
        sink,
        time_to_impact_s,
        stop_m,
        level,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::G;

    /// The recorded crash on Arial: 940 m/s down, 2.44 g retro, ~0.37 g there.
    #[test]
    fn the_arial_crash() {
        let (brake, grav) = (2.44 * G, 0.37 * G);
        // 9.2 km up at 940 m/s: far too late, PULL UP
        let w = ground_warning(9_200.0, 940.0, brake, grav, 2.0).unwrap();
        assert_eq!(w.level, GroundLevel::PullUp);
        // the warning would have started well above the atmosphere
        let caution_from = (1..200)
            .map(|k| k as f64 * 500.0)
            .rev()
            .find(|h| ground_warning(*h, 940.0, brake, grav, 2.0).is_some());
        assert!(caution_from.unwrap() > 40_000.0, "{caution_from:?}");
        let pull_up_from = (1..200).map(|k| k as f64 * 500.0).rev().find(|h| {
            ground_warning(*h, 940.0, brake, grav, 2.0)
                .is_some_and(|w| w.level == GroundLevel::PullUp)
        });
        assert!(pull_up_from.unwrap() > 20_000.0, "{pull_up_from:?}");
    }

    #[test]
    fn hovering_or_climbing_is_quiet() {
        assert!(ground_warning(50.0, 0.0, 20.0, 9.8, 2.0).is_none());
        assert!(ground_warning(50.0, -10.0, 20.0, 9.8, 2.0).is_none());
        // a gentle 3 m/s landing at 20 m is fine
        assert!(ground_warning(20.0, 3.0, 20.0, 9.8, 2.0).is_none());
    }
}
