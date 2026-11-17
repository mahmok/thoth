//! The number conversions that have no lossless form.

/// `part` over `whole`, as a float.
#[must_use]
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::float_arithmetic,
    reason = "token counts are estimates: precision lost above 2^24 does not matter"
)]
#[inline]
pub fn ratio(part: u32, whole: usize) -> f32 {
    part as f32 / whole as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ratio_is_the_part_over_the_whole() {
        assert!((ratio(150, 100) - 1.5).abs() < f32::EPSILON);
    }
}
