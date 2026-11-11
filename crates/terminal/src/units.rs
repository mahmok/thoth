/// `value` as a screen size: a count too big for the screen is as big as the screen can be.
pub fn to_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_become_screen_sizes_and_stop_at_the_largest() {
        assert_eq!(to_u16(0), 0);
        assert_eq!(to_u16(80), 80);
        assert_eq!(to_u16(usize::MAX), u16::MAX);
    }
}
