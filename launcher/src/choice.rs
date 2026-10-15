//! Choosing how to start a tile that offers choices (e.g. which GPU):
//! where focus starts and how it moves. Pure; the UI glue is `choice_ui`.

/// The remembered choice if it still exists, else the first.
pub fn initial_focus(names: &[&str], remembered: Option<&str>) -> usize {
    remembered
        .and_then(|r| names.iter().position(|n| *n == r))
        .unwrap_or(0)
}

/// Moves focus by `delta`, staying on the list.
pub fn step(focused: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let last = i64::try_from(len - 1).unwrap_or(i64::MAX);
    let moved = i64::try_from(focused).unwrap_or(last) + i64::from(delta);
    usize::try_from(moved.clamp(0, last)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 2] = ["Graphics card (RTX 4060)", "Integrated graphics card"];

    #[test]
    fn initial_focus_uses_the_remembered_choice() {
        assert_eq!(initial_focus(&NAMES, Some("Integrated graphics card")), 1);
    }

    #[test]
    fn initial_focus_falls_back_to_first() {
        assert_eq!(initial_focus(&NAMES, None), 0);
        assert_eq!(initial_focus(&NAMES, Some("Renamed since")), 0);
    }

    #[test]
    fn step_stays_in_range() {
        assert_eq!(step(0, -1, 2), 0);
        assert_eq!(step(0, 1, 2), 1);
        assert_eq!(step(1, 1, 2), 1);
        assert_eq!(step(1, -1, 2), 0);
        assert_eq!(step(0, 1, 0), 0);
    }
}
