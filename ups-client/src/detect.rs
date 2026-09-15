//! Two-tier alert/transition detection. Pure logic, fully unit-tested — no
//! network, no notifications, no hardware.

use std::collections::HashMap;

use ups_common::{PowerState, UpsStatus};

/// What the detector decided for a single UPS on this poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlertEvent {
    /// Online -> OnBattery (Tier 1).
    WentOnBattery { id: String },
    /// OnBattery -> Online (recovery, quiet).
    Recovered { id: String },
    /// Crossed below the low-battery threshold while on battery (Tier 2).
    LowBattery {
        id: String,
        pct: u8,
        runtime_secs: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy, Default)]
struct Prev {
    status: Option<PowerState>,
    low_alerted: bool,
}

/// Tracks per-UPS state across polls so alerts fire once per transition,
/// not every poll.
pub struct TransitionDetector {
    threshold_pct: u8,
    prev: HashMap<String, Prev>,
}

impl TransitionDetector {
    pub fn new(threshold_pct: u8) -> Self {
        Self {
            threshold_pct,
            prev: HashMap::new(),
        }
    }

    /// Feed the latest snapshot for one UPS; get back any alerts to fire.
    pub fn observe(&mut self, s: &UpsStatus) -> Vec<AlertEvent> {
        let mut out = Vec::new();
        let entry = self.prev.entry(s.id.clone()).or_default();
        let prev_status = entry.status;

        // Power-state transitions.
        match (prev_status, s.status) {
            (Some(PowerState::Online), PowerState::OnBattery) => {
                out.push(AlertEvent::WentOnBattery { id: s.id.clone() });
                entry.low_alerted = false; // re-arm low-battery alert on new outage
            }
            (Some(PowerState::OnBattery), PowerState::Online) => {
                out.push(AlertEvent::Recovered { id: s.id.clone() });
                entry.low_alerted = false;
            }
            _ => {}
        }

        // Low-battery (Tier 2): only while on battery, fire once per crossing.
        if s.status == PowerState::OnBattery {
            if let Some(p) = s.battery_pct {
                if p < self.threshold_pct && !entry.low_alerted {
                    out.push(AlertEvent::LowBattery {
                        id: s.id.clone(),
                        pct: p,
                        runtime_secs: s.runtime_secs,
                    });
                    entry.low_alerted = true;
                }
                // Re-arm if it climbs back above threshold.
                if p >= self.threshold_pct {
                    entry.low_alerted = false;
                }
            }
        }

        entry.status = Some(s.status);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn status(id: &str, st: PowerState, pct: Option<u8>, rt: Option<u32>) -> UpsStatus {
        UpsStatus {
            id: id.into(),
            status: st,
            battery_pct: pct,
            runtime_secs: rt,
            load_pct: None,
            voltage_v: None,
            last_updated: Utc::now(),
        }
    }

    #[test]
    fn fires_once_on_online_to_onbattery() {
        let mut d = TransitionDetector::new(20);
        assert!(d
            .observe(&status("a", PowerState::Online, Some(100), Some(3600)))
            .is_empty());
        let ev = d.observe(&status("a", PowerState::OnBattery, Some(90), Some(3000)));
        assert_eq!(ev, vec![AlertEvent::WentOnBattery { id: "a".into() }]);
        // Staying on battery fires nothing further.
        assert!(d
            .observe(&status("a", PowerState::OnBattery, Some(85), Some(2800)))
            .is_empty());
    }

    #[test]
    fn fires_recovery_on_return_to_online() {
        let mut d = TransitionDetector::new(20);
        d.observe(&status("a", PowerState::Online, Some(100), None));
        d.observe(&status("a", PowerState::OnBattery, Some(90), None));
        let ev = d.observe(&status("a", PowerState::Online, Some(92), None));
        assert_eq!(ev, vec![AlertEvent::Recovered { id: "a".into() }]);
    }

    #[test]
    fn low_battery_fires_once_per_crossing_and_rearms() {
        let mut d = TransitionDetector::new(20);
        d.observe(&status("a", PowerState::Online, Some(100), None));
        d.observe(&status("a", PowerState::OnBattery, Some(50), Some(1800)));
        // Cross below 20.
        let ev = d.observe(&status("a", PowerState::OnBattery, Some(19), Some(900)));
        assert_eq!(
            ev,
            vec![AlertEvent::LowBattery {
                id: "a".into(),
                pct: 19,
                runtime_secs: Some(900)
            }]
        );
        // Staying below threshold does not re-fire.
        assert!(d
            .observe(&status("a", PowerState::OnBattery, Some(15), Some(700)))
            .is_empty());
        // Rising above threshold re-arms (no event), then dropping fires again.
        assert!(d
            .observe(&status("a", PowerState::OnBattery, Some(25), Some(1100)))
            .is_empty());
        let ev2 = d.observe(&status("a", PowerState::OnBattery, Some(10), Some(400)));
        assert!(matches!(ev2.as_slice(), [AlertEvent::LowBattery { .. }]));
    }

    #[test]
    fn going_online_rearms_low_battery_for_next_outage() {
        let mut d = TransitionDetector::new(20);
        d.observe(&status("a", PowerState::Online, Some(100), None));
        // First outage: goes on battery already below threshold.
        let ev = d.observe(&status("a", PowerState::OnBattery, Some(15), Some(600)));
        assert!(matches!(
            ev.as_slice(),
            [AlertEvent::WentOnBattery { .. }, AlertEvent::LowBattery { .. }]
        ));
        // Recovery fires Recovered and re-arms low-battery.
        let ev = d.observe(&status("a", PowerState::Online, Some(60), None));
        assert_eq!(ev, vec![AlertEvent::Recovered { id: "a".into() }]);
        // Next outage below threshold fires LowBattery again.
        let ev = d.observe(&status("a", PowerState::OnBattery, Some(12), Some(500)));
        assert!(matches!(
            ev.as_slice(),
            [AlertEvent::WentOnBattery { .. }, AlertEvent::LowBattery { .. }]
        ));
    }

    #[test]
    fn unknown_status_does_not_fire_power_events() {
        let mut d = TransitionDetector::new(20);
        d.observe(&status("a", PowerState::Online, Some(100), None));
        assert!(d
            .observe(&status("a", PowerState::Unknown, None, None))
            .is_empty());
    }
}
