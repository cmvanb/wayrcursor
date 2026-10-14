use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    pub inbound: Duration,
    pub hold: Duration,
    pub fade: Duration,
}

/// Where the ring is in its life: `progress` runs 0 (offscreen) to 1 (landed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub progress: f32,
    pub alpha: f32,
}

#[derive(Debug, Clone, Copy)]
enum Phase {
    Inbound { start: Instant },
    Hold { since: Instant, from_alpha: f32 },
    Fade { start: Instant },
}

#[derive(Debug, Clone)]
pub struct Animation {
    timing: Timing,
    phase: Phase,
}

/// Opacity recovery time when a retrigger interrupts the fade.
const RECOVER: Duration = Duration::from_millis(80);

impl Animation {
    pub fn new(timing: Timing, now: Instant) -> Self {
        Self {
            timing,
            phase: Phase::Inbound { start: now },
        }
    }

    /// Keeps a landed ring on screen; an inbound ring is already on its way.
    pub fn retrigger(&mut self, now: Instant) {
        if let Phase::Inbound { .. } = self.phase {
            return;
        }
        let from_alpha = self.pose(now).map_or(0.0, |p| p.alpha);
        self.phase = Phase::Hold {
            since: now,
            from_alpha,
        };
    }

    /// Advances to the phase containing `now`; `None` once the ring has faded out.
    pub fn pose(&mut self, now: Instant) -> Option<Pose> {
        loop {
            match self.phase {
                Phase::Inbound { start } => {
                    let t = fraction(now.saturating_duration_since(start), self.timing.inbound);
                    if t < 1.0 {
                        return Some(Pose {
                            progress: ease_out_cubic(t),
                            alpha: 1.0,
                        });
                    }
                    let since = start + self.timing.inbound;
                    self.phase = Phase::Hold {
                        since,
                        from_alpha: 1.0,
                    };
                }
                Phase::Hold { since, from_alpha } => {
                    let elapsed = now.saturating_duration_since(since);
                    if elapsed < self.timing.hold {
                        let t = fraction(elapsed, RECOVER).min(1.0);
                        return Some(Pose {
                            progress: 1.0,
                            alpha: from_alpha + (1.0 - from_alpha) * t,
                        });
                    }
                    self.phase = Phase::Fade {
                        start: since + self.timing.hold,
                    };
                }
                Phase::Fade { start } => {
                    let t = fraction(now.saturating_duration_since(start), self.timing.fade);
                    return (t < 1.0).then_some(Pose {
                        progress: 1.0,
                        alpha: 1.0 - t,
                    });
                }
            }
        }
    }
}

fn fraction(elapsed: Duration, total: Duration) -> f32 {
    if total.is_zero() {
        1.0
    } else {
        elapsed.as_secs_f32() / total.as_secs_f32()
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn timing() -> Timing {
        Timing {
            inbound: 350 * MS,
            hold: 150 * MS,
            fade: 250 * MS,
        }
    }

    #[test]
    fn starts_offscreen_and_opaque() {
        let t0 = Instant::now();
        let mut a = Animation::new(timing(), t0);
        assert_eq!(
            a.pose(t0),
            Some(Pose {
                progress: 0.0,
                alpha: 1.0
            })
        );
    }

    #[test]
    fn decelerates_into_the_cursor() {
        let t0 = Instant::now();
        let mut a = Animation::new(timing(), t0);
        let half = a.pose(t0 + 175 * MS).unwrap();
        assert!(
            half.progress > 0.8,
            "ease-out covers most distance early: {half:?}"
        );
        assert_eq!(
            a.pose(t0 + 350 * MS),
            Some(Pose {
                progress: 1.0,
                alpha: 1.0
            })
        );
    }

    #[test]
    fn holds_then_fades_then_ends() {
        let t0 = Instant::now();
        let mut a = Animation::new(timing(), t0);
        assert_eq!(
            a.pose(t0 + 499 * MS),
            Some(Pose {
                progress: 1.0,
                alpha: 1.0
            })
        );
        let fading = a.pose(t0 + 625 * MS).unwrap();
        assert_eq!(fading.progress, 1.0);
        assert!(
            (fading.alpha - 0.5).abs() < 0.01,
            "linear fade midpoint: {fading:?}"
        );
        assert_eq!(a.pose(t0 + 750 * MS), None);
    }

    #[test]
    fn retrigger_while_holding_extends_the_hold() {
        let t0 = Instant::now();
        let mut a = Animation::new(timing(), t0);
        a.pose(t0 + 400 * MS);
        a.retrigger(t0 + 400 * MS);
        assert_eq!(
            a.pose(t0 + 549 * MS),
            Some(Pose {
                progress: 1.0,
                alpha: 1.0
            })
        );
        assert_eq!(a.pose(t0 + 800 * MS), None);
    }

    #[test]
    fn retrigger_while_fading_restores_opacity_without_restarting() {
        let t0 = Instant::now();
        let mut a = Animation::new(timing(), t0);
        a.pose(t0 + 625 * MS);
        a.retrigger(t0 + 625 * MS);
        let recovering = a.pose(t0 + 625 * MS).unwrap();
        assert_eq!(recovering.progress, 1.0);
        assert!(
            (recovering.alpha - 0.5).abs() < 0.01,
            "no opacity pop: {recovering:?}"
        );
        assert_eq!(
            a.pose(t0 + 760 * MS),
            Some(Pose {
                progress: 1.0,
                alpha: 1.0
            })
        );
    }

    #[test]
    fn retrigger_while_inbound_changes_nothing() {
        let t0 = Instant::now();
        let mut a = Animation::new(timing(), t0);
        let before = Animation::new(timing(), t0).pose(t0 + 100 * MS);
        a.retrigger(t0 + 50 * MS);
        assert_eq!(a.pose(t0 + 100 * MS), before);
    }
}
