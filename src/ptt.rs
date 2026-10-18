//! Push-to-talk on top of a terminal.
//!
//! Most terminals report key presses but not releases. While a key is held,
//! the OS sends auto-repeat presses, so we treat "repeats stopped arriving" as
//! the release. Terminals that speak the kitty keyboard protocol do report
//! releases, and `release()` handles those directly.

use std::time::{Duration, Instant};

/// Wait before the first auto-repeat. The Windows default delay is about
/// 500 ms, so a hold shorter than this one is still a single tap.
const FIRST_REPEAT_TIMEOUT: Duration = Duration::from_millis(700);
/// Gap between repeats is about 33 ms, so this leaves room for jitter.
const REPEAT_TIMEOUT: Duration = Duration::from_millis(150);

#[derive(Debug, PartialEq)]
pub enum Transition {
    Started,
    /// Recording stopped. `lag` is how long after the last key event the
    /// release was detected; it is zero when the terminal reported it.
    Stopped {
        lag: Duration,
    },
}

#[derive(Default)]
pub struct HoldDetector {
    holding: bool,
    saw_repeat: bool,
    last_event: Option<Instant>,
}

impl HoldDetector {
    pub fn is_holding(&self) -> bool {
        self.holding
    }

    pub fn press(&mut self, now: Instant) -> Option<Transition> {
        self.last_event = Some(now);
        if self.holding {
            self.saw_repeat = true;
            None
        } else {
            self.holding = true;
            self.saw_repeat = false;
            Some(Transition::Started)
        }
    }

    pub fn release(&mut self) -> Option<Transition> {
        if !self.holding {
            return None;
        }
        self.holding = false;
        Some(Transition::Stopped {
            lag: Duration::ZERO,
        })
    }

    /// Call often; detects a release from the missing auto-repeat.
    pub fn tick(&mut self, now: Instant) -> Option<Transition> {
        let last = self.last_event?;
        if !self.holding {
            return None;
        }
        let timeout = if self.saw_repeat {
            REPEAT_TIMEOUT
        } else {
            FIRST_REPEAT_TIMEOUT
        };
        let idle = now.duration_since(last);
        if idle >= timeout {
            self.holding = false;
            Some(Transition::Stopped { lag: idle })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn hold_with_repeats_stops_shortly_after_last_repeat() {
        let t0 = Instant::now();
        let mut d = HoldDetector::default();
        assert_eq!(d.press(t0), Some(Transition::Started));
        // Waiting for the first repeat must not end the recording.
        assert_eq!(d.tick(t0 + ms(500)), None);
        let mut t = t0 + ms(500);
        for _ in 0..30 {
            assert_eq!(d.press(t), None);
            t += ms(33);
            assert_eq!(d.tick(t), None);
        }
        assert!(matches!(
            d.tick(t + ms(200)),
            Some(Transition::Stopped { .. })
        ));
        assert!(!d.is_holding());
    }

    #[test]
    fn single_tap_stops_after_first_repeat_timeout() {
        let t0 = Instant::now();
        let mut d = HoldDetector::default();
        d.press(t0);
        assert_eq!(d.tick(t0 + ms(600)), None);
        assert!(matches!(
            d.tick(t0 + ms(700)),
            Some(Transition::Stopped { .. })
        ));
    }

    #[test]
    fn explicit_release_stops_immediately() {
        let mut d = HoldDetector::default();
        d.press(Instant::now());
        assert_eq!(
            d.release(),
            Some(Transition::Stopped {
                lag: Duration::ZERO
            })
        );
        assert_eq!(d.release(), None);
    }
}
