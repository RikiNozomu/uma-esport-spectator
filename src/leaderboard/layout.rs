//! Where each card goes. Pure functions, no egui state, unit-tested.
//!
//! Units are egui points; the screen is 1080 points tall at scale 1.0.

use egui::Vec2;

use crate::race::Runner;

/// Card width for positions 4 and below.
pub const CARD_WIDTH: f32 = 80.0;
/// Top 3 cards are this much bigger.
pub const BIG_FACTOR: f32 = 1.32;
pub const BIG_COUNT: i32 = 3;
pub const GAP: f32 = 7.0;
pub const MARGIN: f32 = 24.0;

/// Sizes inside a card, all derived from its width.
#[derive(Clone, Copy, Debug)]
pub struct CardGeometry {
    pub width: f32,
    pub height: f32,
    pub pad: f32,
    pub image: f32,
    pub band: f32,
    pub triangle: f32,
    pub number_font: f32,
    pub trainer_font: f32,
}

impl CardGeometry {
    pub fn for_width(width: f32) -> CardGeometry {
        let pad = width * 0.05;
        let image = width - 2.0 * pad;
        let band = width * 0.22;
        CardGeometry {
            width,
            height: pad + image + band,
            pad,
            image,
            band,
            triangle: width * 0.52,
            number_font: width * 0.23,
            trainer_font: width * 0.15,
        }
    }
}

/// Target position of one card.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub gate: i32,
    /// Display position, 0 = leader.
    pub order: i32,
    pub center_x: f32,
    pub bottom: f32,
    pub width: f32,
}

/// Display order: by position, unknown positions last (by gate).
pub fn sorted(runners: &[Runner]) -> Vec<&Runner> {
    let mut v: Vec<&Runner> = runners.iter().collect();
    v.sort_by_key(|r| (r.order < 0, r.order, r.gate));
    v
}

/// Right-aligned row, leader rightmost, bottoms on `y_frac` of the screen.
/// Shrinks everything evenly if the field does not fit the screen width.
pub fn place(runners: &[Runner], screen: Vec2, y_frac: f32) -> Vec<Placement> {
    let ordered = sorted(runners);
    let base_width = |pos: usize| if (pos as i32) < BIG_COUNT { CARD_WIDTH * BIG_FACTOR } else { CARD_WIDTH };

    let total: f32 = (0..ordered.len()).map(base_width).sum::<f32>() + GAP * ordered.len().saturating_sub(1) as f32;
    let available = (screen.x - 2.0 * MARGIN).max(1.0);
    let fit = (available / total).min(1.0);

    let bottom = screen.y * y_frac;
    let mut right = screen.x - MARGIN;
    ordered
        .iter()
        .enumerate()
        .map(|(pos, r)| {
            let width = base_width(pos) * fit;
            let p = Placement { gate: r.gate, order: pos as i32, center_x: right - width / 2.0, bottom, width };
            right -= width + GAP * fit;
            p
        })
        .collect()
}

/// JRA bracket (waku) 1..=8 for a gate in a field of `field` runners.
/// With more than 8 runners the extra runners go to the highest brackets.
pub fn bracket(gate: i32, field: usize) -> i32 {
    let gate = gate.max(1) as usize;
    let field = field.max(gate);
    if field <= 8 {
        return gate.min(8) as i32;
    }
    let base = field / 8;
    let extra = field % 8; // brackets 9-extra..=8 hold one more runner
    let mut first = 1;
    for b in 1..=8 {
        let size = base + usize::from(b > 8 - extra);
        if gate < first + size {
            return b as i32;
        }
        first += size;
    }
    8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner(gate: i32, order: i32) -> Runner {
        Runner { gate, order, ..Default::default() }
    }

    #[test]
    fn brackets_match_jra_rules() {
        // 17 runners (as in the reference screenshot): 13-14 orange, 15-17 pink.
        let b: Vec<i32> = (1..=17).map(|g| bracket(g, 17)).collect();
        assert_eq!(b, vec![1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 8]);
        let b: Vec<i32> = (1..=18).map(|g| bracket(g, 18)).collect();
        assert_eq!(b, vec![1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 7, 8, 8, 8]);
        let b: Vec<i32> = (1..=12).map(|g| bracket(g, 12)).collect();
        assert_eq!(b, vec![1, 2, 3, 4, 5, 5, 6, 6, 7, 7, 8, 8]);
        let b: Vec<i32> = (1..=9).map(|g| bracket(g, 9)).collect();
        assert_eq!(b, vec![1, 2, 3, 4, 5, 6, 7, 8, 8]);
        assert_eq!((1..=5).map(|g| bracket(g, 5)).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn leader_is_rightmost_and_row_is_right_aligned() {
        let rs = vec![runner(1, 2), runner(2, 0), runner(3, 1), runner(4, 3)];
        let p = place(&rs, Vec2::new(1920.0, 1080.0), 0.9);
        assert_eq!(p.iter().map(|p| p.gate).collect::<Vec<_>>(), vec![2, 3, 1, 4]);
        // Leader's right edge sits on the margin.
        assert!((p[0].center_x + p[0].width / 2.0 - (1920.0 - MARGIN)).abs() < 0.01);
        // Strictly right-to-left, no overlap.
        for w in p.windows(2) {
            assert!(w[1].center_x + w[1].width / 2.0 <= w[0].center_x - w[0].width / 2.0);
        }
        // Top 3 are bigger.
        assert!(p[2].width > p[3].width);
        assert!(p.iter().all(|p| (p.bottom - 972.0).abs() < 0.01));
    }

    #[test]
    fn eighteen_runners_fit_a_narrow_screen() {
        let rs: Vec<Runner> = (0..18).map(|i| runner(i + 1, i)).collect();
        let p = place(&rs, Vec2::new(1280.0, 1080.0), 0.9);
        let left = p.last().map(|p| p.center_x - p.width / 2.0).unwrap();
        assert!(left >= MARGIN - 0.01, "left edge {left}");
    }

    #[test]
    fn unknown_positions_go_last() {
        let rs = vec![runner(5, -1), runner(1, 1), runner(2, 0)];
        assert_eq!(sorted(&rs).iter().map(|r| r.gate).collect::<Vec<_>>(), vec![2, 1, 5]);
    }
}
