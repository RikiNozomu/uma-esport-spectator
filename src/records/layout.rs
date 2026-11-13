//! Card geometry (pure, tested): 1080p points, origin bottom-left, y up (as
//! Unity UI anchored to the bottom-left corner).
//!
//! A card group is `GROUP_W` wide. With a picture: the card is `CARD_W` wide
//! and the picture stands on the bottom of the screen, overlapping the card's
//! right edge. Without one: the card stretches over the whole group.
//! Groups are centred in their slice of the screen (one card: the whole
//! screen; two: halves); `fit_scale` shrinks everything when they would not
//! fit (two cards on a 4:3 screen).

pub const BOTTOM: f32 = 48.0;
pub const CARD_W: f32 = 570.0;
pub const STRIP_H: f32 = 46.0;
pub const BODY_H: f32 = 176.0;
pub const PAD_X: f32 = 12.0;
/// The picture's square box: its left edge this far into the group.
pub const PICTURE_X: f32 = 470.0;
pub const PICTURE_H: f32 = 420.0;
pub const GROUP_W: f32 = PICTURE_X + PICTURE_H;
/// Outer margin kept when fitting.
pub const MARGIN_X: f32 = 72.0;
/// Height of the dark gradient rising from the bottom of the screen.
pub const SHADE_H: f32 = 520.0;

pub const HEADING_SIZE: f32 = 26.0;
pub const HOLDER_SIZE: f32 = 74.0;
pub const TIME_SIZE: f32 = 78.0;
pub const DATE_SIZE: f32 = 26.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box2 {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Box2 {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Box2 {
        Box2 { x, y, w, h }
    }

    fn inset_x(self, d: f32) -> Box2 {
        Box2::new(self.x + d, self.y, (self.w - 2.0 * d).max(1.0), self.h)
    }
}

/// Everything of one card, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardRects {
    pub head: Box2,
    pub body: Box2,
    pub date: Box2,
    pub heading_text: Box2,
    pub holder_text: Box2,
    pub time_text: Box2,
    pub date_text: Box2,
    /// The picture's box (fit the picture inside, standing on its bottom).
    pub picture: Option<Box2>,
    /// The holder's name centred (no picture: the whole card is free);
    /// left-aligned next to a picture.
    pub holder_centred: bool,
}

pub fn card_rects(left: f32, has_picture: bool) -> CardRects {
    let w = if has_picture { CARD_W } else { GROUP_W };
    let date = Box2::new(left, BOTTOM, w, STRIP_H);
    let body = Box2::new(left, BOTTOM + STRIP_H, w, BODY_H);
    let head = Box2::new(left, BOTTOM + STRIP_H + BODY_H, w, STRIP_H);
    // Texts stop where the picture starts.
    let text_w = if has_picture { PICTURE_X - PAD_X } else { w - 2.0 * PAD_X };
    let text = |b: Box2| Box2::new(b.x + PAD_X, b.y, text_w, b.h);
    let half = BODY_H / 2.0;
    CardRects {
        head,
        body,
        date,
        heading_text: head.inset_x(PAD_X),
        holder_text: text(Box2::new(left, body.y + half, w, half)),
        time_text: text(Box2::new(left, body.y, w, half)),
        date_text: text(date),
        picture: has_picture.then(|| Box2::new(left + PICTURE_X, 0.0, PICTURE_H, PICTURE_H)),
        holder_centred: !has_picture,
    }
}

/// A `pw` x `ph` picture fitted in `area`, standing on its bottom, centred.
pub fn fit_picture(area: Box2, pw: f32, ph: f32) -> Box2 {
    if !(pw > 0.0 && ph > 0.0) {
        return area;
    }
    let s = (area.w / pw).min(area.h / ph);
    let (w, h) = (pw * s, ph * s);
    Box2::new(area.x + (area.w - w) / 2.0, area.y, w, h)
}

/// Scale (<= 1) so `count` groups fit across a screen `width` points wide.
pub fn fit_scale(width: f32, count: usize) -> f32 {
    let needed = count.max(1) as f32 * GROUP_W + MARGIN_X;
    (width / needed).min(1.0).max(0.1)
}

/// Left x of each group on a screen `width` points wide (already scaled).
pub fn group_lefts(width: f32, count: usize) -> Vec<f32> {
    if count == 0 {
        return Vec::new();
    }
    let slice = width / count as f32;
    (0..count).map(|i| slice * i as f32 + ((slice - GROUP_W) / 2.0).max(0.0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_stack_from_the_bottom() {
        let r = card_rects(100.0, true);
        assert_eq!(r.date.y, BOTTOM);
        assert_eq!(r.body.y, r.date.y + STRIP_H);
        assert_eq!(r.head.y, r.body.y + BODY_H);
        assert_eq!(r.head.w, CARD_W);
        assert_eq!(r.picture, Some(Box2::new(100.0 + PICTURE_X, 0.0, PICTURE_H, PICTURE_H)));
        // Holder over time, both left of the picture.
        assert!(r.holder_text.y > r.time_text.y);
        assert!(r.holder_text.x + r.holder_text.w <= 100.0 + PICTURE_X);
    }

    #[test]
    fn no_picture_stretches_the_card() {
        let r = card_rects(0.0, false);
        assert!(r.holder_centred);
        assert!(!card_rects(0.0, true).holder_centred);
        assert_eq!(r.head.w, GROUP_W);
        assert_eq!(r.time_text.w, GROUP_W - 2.0 * PAD_X);
        assert_eq!(r.picture, None);
    }

    #[test]
    fn pictures_stand_on_the_bottom() {
        let area = Box2::new(0.0, 0.0, 420.0, 420.0);
        assert_eq!(fit_picture(area, 512.0, 512.0), area);
        // A tall picture: full height, centred.
        assert_eq!(fit_picture(area, 210.0, 420.0), Box2::new(105.0, 0.0, 210.0, 420.0));
        // A wide one: full width, on the bottom.
        assert_eq!(fit_picture(area, 840.0, 420.0), Box2::new(0.0, 0.0, 420.0, 210.0));
    }

    #[test]
    fn groups_fit_any_screen() {
        assert_eq!(group_lefts(1920.0, 1), vec![515.0]);
        assert_eq!(group_lefts(1920.0, 2), vec![35.0, 995.0]);
        assert!(group_lefts(1920.0, 0).is_empty());
        assert_eq!(fit_scale(1920.0, 2), 1.0);
        for width in [1920.0, 1440.0, 1280.0, 2560.0] {
            let s = fit_scale(width, 2);
            let lefts = group_lefts(width / s, 2);
            assert!(lefts[1] + GROUP_W <= width / s + 0.01, "{width}");
            assert!(lefts[0] + GROUP_W <= lefts[1] + 0.01, "{width}");
        }
    }
}
