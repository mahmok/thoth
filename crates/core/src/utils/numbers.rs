//! The number conversions that have no lossless form.

/// `count` scaled by `factor`, rounded down and never below zero.
#[must_use]
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::float_arithmetic,
    reason = "token counts are estimates: precision lost above 2^24 does not matter, and `as` saturates"
)]
#[inline]
pub fn scaled(count: usize, factor: f32) -> usize {
    (count as f32 * factor) as usize
}

/// `part` over `whole`, as a float.
#[must_use]
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::float_arithmetic,
    reason = "token counts are estimates: precision lost above 2^24 does not matter"
)]
#[inline]
#[cfg_attr(not(test), expect(clippy::single_call_fn, reason = "kept beside `scaled`: both hold the float conversions"))]
pub fn ratio(part: u32, whole: usize) -> f32 {
    part as f32 / whole as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_rounds_down_and_stops_at_zero() {
        assert_eq!(scaled(10, 1.5), 15);
        assert_eq!(scaled(10, 0.19), 1);
        assert_eq!(scaled(10, -2.0), 0);
        assert_eq!(scaled(0, 3.0), 0);
    }

    #[test]
    fn a_ratio_is_the_part_over_the_whole() {
        assert!((ratio(150, 100) - 1.5).abs() < f32::EPSILON);
    }
}
