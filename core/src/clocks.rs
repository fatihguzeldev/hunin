use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeBounds {
    earliest: i128,
    latest: i128,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeError {
    InvalidInterval,
    InconsistentExchange,
    Overflow,
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidInterval => "invalid time interval",
            Self::InconsistentExchange => "clock exchange violates equal-rate assumptions",
            Self::Overflow => "time arithmetic overflow",
        })
    }
}

impl std::error::Error for TimeError {}

impl TimeBounds {
    pub fn earliest(self) -> i128 {
        self.earliest
    }

    pub fn latest(self) -> i128 {
        self.latest
    }

    pub fn new(earliest: i128, latest: i128) -> Result<Self, TimeError> {
        if earliest > latest {
            return Err(TimeError::InvalidInterval);
        }
        Ok(Self { earliest, latest })
    }

    pub fn shifted(self, offset: Self) -> Result<Self, TimeError> {
        Self::new(
            self.earliest
                .checked_add(offset.earliest)
                .ok_or(TimeError::Overflow)?,
            self.latest
                .checked_add(offset.latest)
                .ok_or(TimeError::Overflow)?,
        )
    }

    pub fn duration_until(self, end: Self) -> Result<Self, TimeError> {
        Self::new(
            end.earliest
                .checked_sub(self.latest)
                .ok_or(TimeError::Overflow)?,
            end.latest
                .checked_sub(self.earliest)
                .ok_or(TimeError::Overflow)?,
        )
    }

    pub fn certainly_before(self, other: Self) -> bool {
        self.latest < other.earliest
    }

    pub fn intersect(self, other: Self) -> Result<Self, TimeError> {
        Self::new(
            self.earliest.max(other.earliest),
            self.latest.min(other.latest),
        )
    }
}

// Offsets use equal-rate clocks over this exchange, with nonnegative transit delays.
pub fn exchange_offset(
    core_send: i128,
    producer_receive: i128,
    producer_send: i128,
    core_receive: i128,
    timestamp_error: i128,
) -> Result<TimeBounds, TimeError> {
    if timestamp_error < 0 || core_send > core_receive || producer_receive > producer_send {
        return Err(TimeError::InvalidInterval);
    }
    let low = core_send
        .checked_sub(producer_receive)
        .and_then(|x| x.checked_sub(timestamp_error))
        .ok_or(TimeError::Overflow)?;
    let high = core_receive
        .checked_sub(producer_send)
        .and_then(|x| x.checked_add(timestamp_error))
        .ok_or(TimeError::Overflow)?;
    TimeBounds::new(low, high).map_err(|_| TimeError::InconsistentExchange)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asymmetric_transit_contains_independently_known_offset() {
        // Core is 800 ns ahead; transit delays are 30 and 70 ns, processing is 12 ns.
        let offset = exchange_offset(1000, 230, 242, 1112, 0).unwrap();
        assert_eq!(
            offset,
            TimeBounds {
                earliest: 770,
                latest: 870
            }
        );
        assert!(offset.earliest <= 800 && offset.latest >= 800);
        let mapped = TimeBounds::new(300, 300).unwrap().shifted(offset).unwrap();
        assert_eq!(
            mapped,
            TimeBounds {
                earliest: 1070,
                latest: 1170
            }
        );
    }

    #[test]
    fn uncertainty_survives_duration_and_ordering() {
        let a = TimeBounds::new(10, 30).unwrap();
        let b = TimeBounds::new(20, 50).unwrap();
        assert!(!a.certainly_before(b));
        assert_eq!(
            a.duration_until(b).unwrap(),
            TimeBounds {
                earliest: -10,
                latest: 40
            }
        );
        assert_eq!(
            a.intersect(b).unwrap(),
            TimeBounds {
                earliest: 20,
                latest: 30
            }
        );
    }

    #[test]
    fn invalid_assumptions_and_overflow_are_explicit() {
        assert_eq!(
            exchange_offset(0, 0, 100, 10, 0),
            Err(TimeError::InconsistentExchange)
        );
        assert_eq!(
            exchange_offset(0, 0, 0, 0, -1),
            Err(TimeError::InvalidInterval)
        );
        assert_eq!(
            exchange_offset(i128::MIN, 1, 1, 0, 0),
            Err(TimeError::Overflow)
        );
        assert_eq!(
            TimeBounds::new(0, 1)
                .unwrap()
                .shifted(TimeBounds::new(i128::MAX, i128::MAX).unwrap()),
            Err(TimeError::Overflow)
        );
    }
}
