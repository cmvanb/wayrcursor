use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::config::ShakeParams;

/// Pointer movement in millimetres of physical device travel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    pub dx: f32,
    pub dy: f32,
    pub at: Instant,
}

/// Recognises rapid back-and-forth strokes on either axis.
pub struct ShakeDetector {
    params: ShakeParams,
    axes: [Axis; 2],
}

#[derive(Default)]
struct Axis {
    direction: f32,
    travel: f32,
    against: f32,
    reversals: VecDeque<Instant>,
}

impl ShakeDetector {
    pub fn new(params: ShakeParams) -> Self {
        Self {
            params,
            axes: Default::default(),
        }
    }

    pub fn set_params(&mut self, params: ShakeParams) {
        self.params = params;
    }

    /// Returns true on each stroke reversal that completes or continues a shake.
    pub fn feed(&mut self, motion: Motion) -> bool {
        let [x, y] = &mut self.axes;
        let fired_x = x.push(motion.dx, motion.at, &self.params);
        let fired_y = y.push(motion.dy, motion.at, &self.params);
        fired_x || fired_y
    }
}

impl Axis {
    /// Returns true when this delta completes a qualifying reversal within a shake.
    fn push(&mut self, delta: f32, at: Instant, params: &ShakeParams) -> bool {
        if delta == 0.0 {
            return false;
        }
        let direction = delta.signum();
        if self.direction == 0.0 || direction == self.direction {
            self.direction = direction;
            self.travel += delta.abs();
            self.against = 0.0;
            return false;
        }
        // Hysteresis: a sensor wobble against the stroke is not a reversal.
        self.against += delta.abs();
        if self.against < params.min_travel_mm / 4.0 {
            return false;
        }
        let counted = self.travel >= params.min_travel_mm;
        self.direction = direction;
        self.travel = self.against;
        self.against = 0.0;
        if !counted {
            return false;
        }
        let window = Duration::from_millis(params.window_ms);
        self.reversals.push_back(at);
        while self
            .reversals
            .front()
            .is_some_and(|&t| at.duration_since(t) > window)
        {
            self.reversals.pop_front();
        }
        self.reversals.len() >= params.reversals as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: Duration = Duration::from_millis(8);

    fn params() -> ShakeParams {
        ShakeParams {
            reversals: 4,
            window_ms: 600,
            min_travel_mm: 8.0,
        }
    }

    /// Feeds strokes along `axis` (x = 0, y = 1), each `mm` long over `per_stroke`; counts how often it fired.
    fn strokes(
        detector: &mut ShakeDetector,
        t: &mut Instant,
        axis: usize,
        mm: f32,
        per_stroke: Duration,
        count: usize,
    ) -> usize {
        let steps = (per_stroke.as_millis() / STEP.as_millis()) as usize;
        let mut fired = 0;
        for stroke in 0..count {
            let sign = if stroke % 2 == 0 { 1.0 } else { -1.0 };
            for _ in 0..steps {
                *t += STEP;
                let d = sign * mm / steps as f32;
                let motion = if axis == 0 {
                    Motion {
                        dx: d,
                        dy: 0.0,
                        at: *t,
                    }
                } else {
                    Motion {
                        dx: 0.0,
                        dy: d,
                        at: *t,
                    }
                };
                fired += detector.feed(motion) as usize;
            }
        }
        fired
    }

    #[test]
    fn brisk_horizontal_shake_fires() {
        let mut d = ShakeDetector::new(params());
        let mut t = Instant::now();
        assert!(strokes(&mut d, &mut t, 0, 25.0, Duration::from_millis(80), 6) > 0);
    }

    #[test]
    fn brisk_vertical_shake_fires() {
        let mut d = ShakeDetector::new(params());
        let mut t = Instant::now();
        assert!(strokes(&mut d, &mut t, 1, 25.0, Duration::from_millis(80), 6) > 0);
    }

    #[test]
    fn slow_back_and_forth_is_not_a_shake() {
        let mut d = ShakeDetector::new(params());
        let mut t = Instant::now();
        assert_eq!(
            strokes(&mut d, &mut t, 0, 25.0, Duration::from_millis(400), 8),
            0
        );
    }

    #[test]
    fn small_jitter_is_not_a_shake() {
        let mut d = ShakeDetector::new(params());
        let mut t = Instant::now();
        assert_eq!(
            strokes(&mut d, &mut t, 0, 3.0, Duration::from_millis(40), 20),
            0
        );
    }

    #[test]
    fn long_straight_motion_is_not_a_shake() {
        let mut d = ShakeDetector::new(params());
        let mut t = Instant::now();
        assert_eq!(
            strokes(&mut d, &mut t, 0, 400.0, Duration::from_millis(800), 1),
            0
        );
    }

    #[test]
    fn counter_jitter_inside_a_stroke_is_not_a_reversal() {
        let mut d = ShakeDetector::new(params());
        let mut t = Instant::now();
        let mut fired = 0;
        for _ in 0..40 {
            for dx in [2.0, -0.3] {
                t += STEP;
                fired += d.feed(Motion { dx, dy: 0.0, at: t }) as usize;
            }
        }
        assert_eq!(fired, 0);
    }

    #[test]
    fn sustained_shaking_keeps_firing() {
        let mut d = ShakeDetector::new(params());
        let mut t = Instant::now();
        strokes(&mut d, &mut t, 0, 25.0, Duration::from_millis(80), 6);
        assert!(strokes(&mut d, &mut t, 0, 25.0, Duration::from_millis(80), 4) >= 3);
    }

    #[test]
    fn default_params_ignore_slow_drift() {
        let mut d = ShakeDetector::new(crate::config::Config::default().shake);
        let mut t = Instant::now();
        let mut fired = 0;
        for stroke in 0..8 {
            let sign = if stroke % 2 == 0 { 1.0 } else { -1.0 };
            for _ in 0..10 {
                t += Duration::from_millis(60);
                fired += d.feed(Motion {
                    dx: sign * 2.54,
                    dy: 0.0,
                    at: t,
                }) as usize;
            }
        }
        assert_eq!(fired, 0);
    }
}
