//! Card animation. Each card follows one runner (keyed by gate) and eases
//! towards its target slot, so a swap shows the two cards sliding past each
//! other. Gaining a place gives a short scale-up "pop", losing one a small dip.

use std::collections::HashMap;

/// Higher = snappier. ~0.3 s to settle.
const MOVE_RATE: f32 = 10.0;
const FADE_RATE: f32 = 6.0;
const POP_TIME: f64 = 0.45;
const POP_UP: f32 = 0.16;
const POP_DOWN: f32 = 0.08;

/// Where a card should end up (from `layout::place`).
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub gate: i32,
    pub order: i32,
    pub center_x: f32,
    pub bottom: f32,
    pub width: f32,
}

/// What to draw this frame.
#[derive(Clone, Copy, Debug)]
pub struct CardState {
    pub gate: i32,
    pub center_x: f32,
    /// Cards scale around their bottom-centre.
    pub bottom: f32,
    pub width: f32,
    pub opacity: f32,
    /// Draw order: higher on top.
    pub z: f32,
}

#[derive(Clone, Copy, Debug)]
struct Card {
    x: f32,
    width: f32,
    bottom: f32,
    opacity: f32,
    order: i32,
    /// (start time, gained a place?)
    pop: Option<(f64, bool)>,
}

#[derive(Default)]
pub struct Animator {
    cards: HashMap<i32, Card>,
    last_time: Option<f64>,
}

impl Animator {
    /// Cards of the first update start settled in their slots (the board's
    /// own entrance animates them); later newcomers fade in on their own.
    fn first_update(&self) -> bool {
        self.last_time.is_none()
    }
}

pub fn approach(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    current + (target - current) * (1.0 - (-rate * dt).exp())
}

impl Animator {
    pub fn update<T: Into<Target> + Copy>(&mut self, targets: &[T], time: f64) -> Vec<CardState> {
        let dt = self.last_time.map(|t| (time - t).clamp(0.0, 0.1) as f32).unwrap_or(0.0);
        let settled = self.first_update();
        self.last_time = Some(time);

        let targets: Vec<Target> = targets.iter().map(|&t| t.into()).collect();
        // Runners that left the field (new race) lose their cards.
        self.cards.retain(|gate, _| targets.iter().any(|t| t.gate == *gate));

        targets
            .iter()
            .map(|t| {
                let card = self.cards.entry(t.gate).or_insert(Card {
                    // New cards fade in at their slot, from slightly lower.
                    x: t.center_x,
                    width: t.width,
                    bottom: if settled { t.bottom } else { t.bottom + t.width * 0.25 },
                    opacity: if settled { 1.0 } else { 0.0 },
                    order: t.order,
                    pop: None,
                });

                if t.order != card.order {
                    card.pop = Some((time, t.order < card.order));
                    card.order = t.order;
                }

                card.x = approach(card.x, t.center_x, MOVE_RATE, dt);
                card.width = approach(card.width, t.width, MOVE_RATE, dt);
                card.bottom = approach(card.bottom, t.bottom, MOVE_RATE, dt);
                card.opacity = approach(card.opacity, 1.0, FADE_RATE, dt);

                let mut scale = 1.0;
                let mut rising = false;
                if let Some((start, gained)) = card.pop {
                    let p = ((time - start) / POP_TIME) as f32;
                    if (0.0..1.0).contains(&p) {
                        let bump = (p * std::f32::consts::PI).sin();
                        scale = if gained { 1.0 + POP_UP * bump } else { 1.0 - POP_DOWN * bump };
                        rising = gained;
                    } else {
                        card.pop = None;
                    }
                }

                CardState {
                    gate: t.gate,
                    center_x: card.x,
                    bottom: card.bottom,
                    width: card.width * scale,
                    opacity: card.opacity.clamp(0.0, 1.0),
                    // Rising cards pass over the others; otherwise higher places on top.
                    z: if rising { 1000.0 } else { -(t.order as f32) },
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(gate: i32, order: i32, x: f32) -> Target {
        Target { gate, order, center_x: x, bottom: 100.0, width: 80.0 }
    }

    #[test]
    fn swap_moves_cards_smoothly_and_pops_the_riser() {
        let mut a = Animator::default();
        a.update(&[t(1, 0, 500.0), t(2, 1, 400.0)], 0.0);
        // Settle.
        let mut time = 0.0;
        for _ in 0..60 {
            time += 1.0 / 60.0;
            a.update(&[t(1, 0, 500.0), t(2, 1, 400.0)], time);
        }
        // Swap: gate 2 takes the lead.
        time += 1.0 / 60.0;
        let s = a.update(&[t(2, 0, 500.0), t(1, 1, 400.0)], time);
        let g2 = s.iter().find(|c| c.gate == 2).unwrap();
        let g1 = s.iter().find(|c| c.gate == 1).unwrap();
        // One frame in: moving, not jumped.
        assert!(g2.center_x > 400.0 && g2.center_x < 450.0, "{}", g2.center_x);
        assert!(g1.center_x < 500.0 && g1.center_x > 450.0, "{}", g1.center_x);
        assert!(g2.z > g1.z, "riser drawn on top");

        // Mid-pop the riser is bigger, the loser smaller.
        let mut mid = Vec::new();
        for _ in 0..13 {
            time += 1.0 / 60.0;
            mid = a.update(&[t(2, 0, 500.0), t(1, 1, 400.0)], time);
        }
        let g2 = mid.iter().find(|c| c.gate == 2).unwrap();
        let g1 = mid.iter().find(|c| c.gate == 1).unwrap();
        assert!(g2.width > 80.0 && g1.width < 80.0, "{} {}", g2.width, g1.width);

        // After a second everything has settled.
        let mut end = Vec::new();
        for _ in 0..60 {
            time += 1.0 / 60.0;
            end = a.update(&[t(2, 0, 500.0), t(1, 1, 400.0)], time);
        }
        let g2 = end.iter().find(|c| c.gate == 2).unwrap();
        assert!((g2.center_x - 500.0).abs() < 0.5 && (g2.width - 80.0).abs() < 0.01);
    }

    #[test]
    fn first_cards_start_settled_later_ones_fade_in() {
        let mut a = Animator::default();
        let s = a.update(&[t(1, 0, 500.0)], 0.0);
        assert_eq!((s[0].opacity, s[0].bottom), (1.0, 100.0));
        let s = a.update(&[t(1, 0, 500.0), t(2, 1, 400.0)], 0.016);
        let newcomer = s.iter().find(|c| c.gate == 2).unwrap();
        assert!(newcomer.opacity < 0.2 && newcomer.bottom > 100.0);
    }

    #[test]
    fn departed_runners_are_dropped() {
        let mut a = Animator::default();
        a.update(&[t(1, 0, 500.0), t(2, 1, 400.0)], 0.0);
        let s = a.update(&[t(1, 0, 500.0)], 0.1);
        assert_eq!(s.len(), 1);
        assert_eq!(a.cards.len(), 1);
    }
}
