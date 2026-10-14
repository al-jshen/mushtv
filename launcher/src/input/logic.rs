//! Pure input state machine: turns raw device events into launcher actions.
//!
//! Handles Guide-hold timing, Home debounce, navigation auto-repeat, the
//! cooldown after an app exits, and ignoring navigation while an app runs.
//! Time is passed in, so everything here is testable without devices.

use std::collections::HashMap;
use std::time::{Duration, Instant};

pub type DevId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawEvent {
    /// A configured Home key was pressed on a keyboard-like device.
    HomeKey,
    GuideDown(DevId),
    GuideUp(DevId),
    /// D-pad or stick direction after the deadzone: -1, 0 or 1.
    Dir {
        dev: DevId,
        axis: Axis,
        value: i8,
    },
    /// The gamepad's bottom face button (A on Xbox layouts).
    South(DevId),
    /// A remote's OK/Select key (KEY_OK, KEY_SELECT). These have no keysym,
    /// so they never arrive through Wayland.
    SelectKey,
    /// Back: the gamepad's right face button (B), or a remote's Back/Exit key.
    Back,
    DeviceGone(DevId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Left,
    Right,
    Up,
    Down,
    Activate,
    Back,
    Home,
}

pub const HOME_DEBOUNCE: Duration = Duration::from_millis(1000);
pub const RETURN_COOLDOWN: Duration = Duration::from_millis(500);
pub const REPEAT_DELAY: Duration = Duration::from_millis(400);
pub const REPEAT_INTERVAL: Duration = Duration::from_millis(150);

pub struct Logic {
    guide_hold: Duration,
    app_active: bool,
    cooldown_until: Option<Instant>,
    last_home: Option<Instant>,
    /// Guide buttons currently down: when pressed, and whether Home already fired.
    guide_down: HashMap<DevId, (Instant, bool)>,
    /// Directions currently held: the value and when it next repeats.
    held: HashMap<(DevId, Axis), (i8, Instant)>,
}

impl Logic {
    pub fn new(guide_hold: Duration) -> Self {
        Self {
            guide_hold,
            app_active: false,
            cooldown_until: None,
            last_home: None,
            guide_down: HashMap::new(),
            held: HashMap::new(),
        }
    }

    /// `active` is true while an app is opening, running or stopping.
    pub fn set_app_active(&mut self, active: bool, now: Instant) {
        if active {
            self.held.clear();
        } else if self.app_active {
            self.cooldown_until = Some(now + RETURN_COOLDOWN);
        }
        self.app_active = active;
    }

    pub fn on_event(&mut self, ev: RawEvent, now: Instant) -> Vec<Action> {
        match ev {
            RawEvent::HomeKey => self.home(now),
            RawEvent::GuideDown(dev) => {
                self.guide_down.insert(dev, (now, false));
                vec![]
            }
            RawEvent::GuideUp(dev) => {
                self.guide_down.remove(&dev);
                vec![]
            }
            RawEvent::Dir { dev, axis, value } => {
                if value == 0 || !self.navigation_allowed(now) {
                    self.held.remove(&(dev, axis));
                    return vec![];
                }
                if self
                    .held
                    .get(&(dev, axis))
                    .is_some_and(|(v, _)| *v == value)
                {
                    return vec![];
                }
                self.held.insert((dev, axis), (value, now + REPEAT_DELAY));
                vec![direction(axis, value)]
            }
            RawEvent::South(_) | RawEvent::SelectKey => {
                if self.navigation_allowed(now) {
                    vec![Action::Activate]
                } else {
                    vec![]
                }
            }
            RawEvent::Back => {
                if self.navigation_allowed(now) {
                    vec![Action::Back]
                } else {
                    vec![]
                }
            }
            RawEvent::DeviceGone(dev) => {
                self.guide_down.remove(&dev);
                self.held.retain(|(d, _), _| *d != dev);
                vec![]
            }
        }
    }

    /// Fires Guide holds and navigation repeats that are due at `now`.
    pub fn on_tick(&mut self, now: Instant) -> Vec<Action> {
        let mut actions = vec![];
        let mut hold_fired = false;
        for (pressed, fired) in self.guide_down.values_mut() {
            if !*fired && now >= *pressed + self.guide_hold {
                *fired = true;
                hold_fired = true;
            }
        }
        if hold_fired {
            actions.extend(self.home(now));
        }
        for ((_, axis), (value, next)) in self.held.iter_mut() {
            if now >= *next {
                actions.push(direction(*axis, *value));
                // Schedule from `now`, so a stalled loop can't fire a burst.
                *next = now + REPEAT_INTERVAL;
            }
        }
        actions
    }

    /// When `on_tick` next needs to run, if anything is pending.
    pub fn next_deadline(&self) -> Option<Instant> {
        let holds = self
            .guide_down
            .values()
            .filter(|(_, fired)| !fired)
            .map(|(pressed, _)| *pressed + self.guide_hold);
        let repeats = self.held.values().map(|(_, next)| *next);
        holds.chain(repeats).min()
    }

    fn home(&mut self, now: Instant) -> Vec<Action> {
        if self
            .last_home
            .is_some_and(|last| now < last + HOME_DEBOUNCE)
        {
            return vec![];
        }
        self.last_home = Some(now);
        vec![Action::Home]
    }

    fn navigation_allowed(&self, now: Instant) -> bool {
        !self.app_active && self.cooldown_until.is_none_or(|until| now >= until)
    }
}

fn direction(axis: Axis, value: i8) -> Action {
    match (axis, value < 0) {
        (Axis::X, true) => Action::Left,
        (Axis::X, false) => Action::Right,
        (Axis::Y, true) => Action::Up,
        (Axis::Y, false) => Action::Down,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;

    const HOLD: Duration = Duration::from_millis(1000);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn setup() -> (Logic, Instant) {
        (Logic::new(HOLD), Instant::now())
    }

    fn x(dev: DevId, value: i8) -> RawEvent {
        RawEvent::Dir {
            dev,
            axis: Axis::X,
            value,
        }
    }

    #[test]
    fn guide_hold_fires_home_once() {
        let (mut l, t) = setup();
        assert_eq!(l.on_event(RawEvent::GuideDown(1), t), vec![]);
        assert_eq!(l.next_deadline(), Some(t + HOLD));
        assert_eq!(l.on_tick(t + ms(999)), vec![]);
        assert_eq!(l.on_tick(t + HOLD), vec![Home]);
        assert_eq!(l.on_tick(t + ms(5000)), vec![]);
        assert_eq!(l.next_deadline(), None);
    }

    #[test]
    fn short_guide_press_does_nothing() {
        let (mut l, t) = setup();
        l.on_event(RawEvent::GuideDown(1), t);
        assert_eq!(l.next_deadline(), Some(t + HOLD));
        assert_eq!(l.on_event(RawEvent::GuideUp(1), t + ms(300)), vec![]);
        assert_eq!(l.on_tick(t + ms(2000)), vec![]);
        assert_eq!(l.next_deadline(), None);
    }

    #[test]
    fn guide_hold_works_while_app_active() {
        let (mut l, t) = setup();
        l.set_app_active(true, t);
        l.on_event(RawEvent::GuideDown(1), t);
        assert_eq!(l.on_tick(t + HOLD), vec![Home]);
    }

    #[test]
    fn home_key_fires_immediately_and_is_debounced() {
        let (mut l, t) = setup();
        assert_eq!(l.on_event(RawEvent::HomeKey, t), vec![Home]);
        assert_eq!(l.on_event(RawEvent::HomeKey, t + ms(500)), vec![]);
        assert_eq!(l.on_event(RawEvent::HomeKey, t + ms(1000)), vec![Home]);
    }

    #[test]
    fn guide_hold_is_debounced_against_home_key() {
        let (mut l, t) = setup();
        l.on_event(RawEvent::GuideDown(1), t);
        assert_eq!(l.on_event(RawEvent::HomeKey, t + ms(500)), vec![Home]);
        assert_eq!(l.on_tick(t + HOLD), vec![]);
    }

    #[test]
    fn home_key_works_while_app_active() {
        let (mut l, t) = setup();
        l.set_app_active(true, t);
        assert_eq!(l.on_event(RawEvent::HomeKey, t), vec![Home]);
    }

    #[test]
    fn navigation_is_ignored_while_app_active() {
        let (mut l, t) = setup();
        l.set_app_active(true, t);
        assert_eq!(l.on_event(x(1, -1), t), vec![]);
        assert_eq!(l.on_event(RawEvent::South(1), t), vec![]);
        assert_eq!(l.on_tick(t + ms(1000)), vec![]);
        assert_eq!(l.next_deadline(), None);
        l.set_app_active(false, t + ms(2000));
        assert_eq!(l.on_event(RawEvent::South(1), t + ms(3000)), vec![Activate]);
    }

    #[test]
    fn directions_map_to_actions() {
        let (mut l, t) = setup();
        assert_eq!(l.on_event(x(1, -1), t), vec![Left]);
        l.on_event(x(1, 0), t);
        assert_eq!(l.on_event(x(1, 1), t), vec![Right]);
        let y = |value| RawEvent::Dir {
            dev: 1,
            axis: Axis::Y,
            value,
        };
        assert_eq!(l.on_event(y(-1), t), vec![Up]);
        l.on_event(y(0), t);
        assert_eq!(l.on_event(y(1), t), vec![Down]);
    }

    #[test]
    fn held_direction_repeats_then_stops_on_release() {
        let (mut l, t) = setup();
        assert_eq!(l.on_event(x(1, 1), t), vec![Right]);
        assert_eq!(l.next_deadline(), Some(t + REPEAT_DELAY));
        assert_eq!(l.on_tick(t + ms(399)), vec![]);
        assert_eq!(l.on_tick(t + ms(400)), vec![Right]);
        assert_eq!(l.on_tick(t + ms(549)), vec![]);
        assert_eq!(l.on_tick(t + ms(550)), vec![Right]);
        assert_eq!(l.on_event(x(1, 0), t + ms(600)), vec![]);
        assert_eq!(l.on_tick(t + ms(2000)), vec![]);
        assert_eq!(l.next_deadline(), None);
    }

    #[test]
    fn repeated_same_direction_event_does_not_retrigger() {
        let (mut l, t) = setup();
        assert_eq!(l.on_event(x(1, 1), t), vec![Right]);
        assert_eq!(l.on_event(x(1, 1), t + ms(50)), vec![]);
        assert_eq!(l.on_tick(t + ms(400)), vec![Right]);
    }

    #[test]
    fn long_stall_does_not_burst_repeats() {
        let (mut l, t) = setup();
        l.on_event(x(1, 1), t);
        assert_eq!(l.on_tick(t + ms(3000)), vec![Right]);
        assert_eq!(l.next_deadline(), Some(t + ms(3000) + REPEAT_INTERVAL));
    }

    #[test]
    fn south_activates_when_idle() {
        let (mut l, t) = setup();
        assert_eq!(l.on_event(RawEvent::South(1), t), vec![Activate]);
    }

    #[test]
    fn select_key_activates_when_idle() {
        let (mut l, t) = setup();
        assert_eq!(l.on_event(RawEvent::SelectKey, t), vec![Activate]);
    }

    #[test]
    fn select_key_is_gated_like_south() {
        let (mut l, t) = setup();
        l.set_app_active(true, t);
        assert_eq!(l.on_event(RawEvent::SelectKey, t), vec![]);
        l.set_app_active(false, t + ms(1000));
        assert_eq!(l.on_event(RawEvent::SelectKey, t + ms(1200)), vec![]);
        assert_eq!(
            l.on_event(RawEvent::SelectKey, t + ms(1600)),
            vec![Activate]
        );
    }

    #[test]
    fn back_acts_when_idle() {
        let (mut l, t) = setup();
        assert_eq!(l.on_event(RawEvent::Back, t), vec![Back]);
    }

    #[test]
    fn back_is_gated_like_south() {
        let (mut l, t) = setup();
        l.set_app_active(true, t);
        assert_eq!(l.on_event(RawEvent::Back, t), vec![]);
        l.set_app_active(false, t + ms(1000));
        assert_eq!(l.on_event(RawEvent::Back, t + ms(1200)), vec![]);
        assert_eq!(l.on_event(RawEvent::Back, t + ms(1600)), vec![Back]);
    }

    #[test]
    fn cooldown_after_app_exit_ignores_navigation() {
        let (mut l, t) = setup();
        l.set_app_active(true, t);
        l.set_app_active(false, t + ms(1000));
        assert_eq!(l.on_event(RawEvent::South(1), t + ms(1300)), vec![]);
        assert_eq!(l.on_event(x(1, 1), t + ms(1300)), vec![]);
        assert_eq!(l.on_event(RawEvent::South(1), t + ms(1600)), vec![Activate]);
    }

    #[test]
    fn app_start_cancels_held_repeat() {
        let (mut l, t) = setup();
        assert_eq!(l.on_event(x(1, 1), t), vec![Right]);
        l.set_app_active(true, t + ms(100));
        assert_eq!(l.on_tick(t + ms(1000)), vec![]);
        assert_eq!(l.next_deadline(), None);
    }

    #[test]
    fn device_gone_clears_its_holds() {
        let (mut l, t) = setup();
        l.on_event(RawEvent::GuideDown(1), t);
        l.on_event(x(1, 1), t);
        assert_eq!(l.next_deadline(), Some(t + REPEAT_DELAY));
        l.on_event(RawEvent::DeviceGone(1), t + ms(100));
        assert_eq!(l.on_tick(t + ms(2000)), vec![]);
        assert_eq!(l.next_deadline(), None);
    }

    #[test]
    fn devices_are_tracked_separately() {
        let (mut l, t) = setup();
        l.on_event(RawEvent::GuideDown(1), t);
        l.on_event(RawEvent::GuideDown(2), t + ms(500));
        l.on_event(RawEvent::GuideUp(1), t + ms(600));
        assert_eq!(l.on_tick(t + ms(1500)), vec![Home]);
    }
}
