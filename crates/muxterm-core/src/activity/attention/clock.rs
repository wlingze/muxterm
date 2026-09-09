//! 可注入时钟：状态机聚合的时间来源。

use std::time::{Duration, Instant};

/// 时钟抽象。
pub trait Clock {
    fn now(&self) -> Instant;
}

/// 真实时钟。
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// 测试用假时钟。
#[derive(Clone)]
pub struct FakeClock {
    now: Instant,
}

impl FakeClock {
    pub fn new(start: Instant) -> Self {
        Self { now: start }
    }

    pub fn advance(&mut self, d: Duration) {
        self.now += d;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_advances() {
        let start = Instant::now();
        let mut c = FakeClock::new(start);
        assert_eq!(c.now(), start);
        c.advance(Duration::from_millis(50));
        assert_eq!(c.now(), start + Duration::from_millis(50));
    }
}
