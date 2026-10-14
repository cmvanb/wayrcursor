use crate::anim::Pose;
use crate::config::RingStyle;

/// One renderable ring in output-local logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingFrame {
    pub center: (f32, f32),
    pub radius: f32,
    pub alpha: f32,
}

/// Places the ring for `pose`, starting where even its inner edge clears every corner of the output.
pub fn place(style: &RingStyle, pose: Pose, center: (f32, f32), output: (f32, f32)) -> RingFrame {
    let far_x = center.0.max(output.0 - center.0);
    let far_y = center.1.max(output.1 - center.1);
    let start = far_x.hypot(far_y) + half_extent(style);
    RingFrame {
        center,
        radius: start + (style.radius - start) * pose.progress,
        alpha: pose.alpha,
    }
}

/// Distance from the ring's centerline to its outermost antialiased pixel.
pub fn half_extent(style: &RingStyle) -> f32 {
    style.width / 2.0 + style.outline_width + 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corner_distances(center: (f32, f32), output: (f32, f32)) -> [f32; 4] {
        [(0.0, 0.0), (output.0, 0.0), (0.0, output.1), output]
            .map(|(x, y)| ((x - center.0).powi(2) + (y - center.1).powi(2)).sqrt())
    }

    #[test]
    fn starts_fully_outside_the_output() {
        let style = RingStyle::default();
        let center = (300.0, 200.0);
        let output = (1920.0, 1080.0);
        let ring = place(
            &style,
            Pose {
                progress: 0.0,
                alpha: 1.0,
            },
            center,
            output,
        );
        let inner_edge = ring.radius - half_extent(&style);
        for d in corner_distances(center, output) {
            assert!(
                inner_edge >= d,
                "inner edge {inner_edge} inside corner at {d}"
            );
        }
    }

    #[test]
    fn lands_at_the_configured_radius() {
        let style = RingStyle::default();
        let ring = place(
            &style,
            Pose {
                progress: 1.0,
                alpha: 0.4,
            },
            (10.0, 20.0),
            (800.0, 600.0),
        );
        assert_eq!(
            ring,
            RingFrame {
                center: (10.0, 20.0),
                radius: 40.0,
                alpha: 0.4
            }
        );
    }
}
