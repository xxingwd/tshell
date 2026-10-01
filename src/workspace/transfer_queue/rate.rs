use std::{collections::VecDeque, time::Duration};

#[derive(Default)]
pub(super) struct TransferRate {
    samples: VecDeque<(Duration, u64)>,
    pub bytes_per_second: u64,
}

impl TransferRate {
    pub fn sample(&mut self, elapsed: Duration, bytes: u64) -> bool {
        if self
            .samples
            .back()
            .is_some_and(|(time, _)| elapsed.saturating_sub(*time) < Duration::from_millis(250))
        {
            return false;
        }
        self.samples.push_back((elapsed, bytes));
        while self.samples.len() > 2
            && self
                .samples
                .get(1)
                .is_some_and(|(time, _)| elapsed.saturating_sub(*time) >= Duration::from_secs(3))
        {
            self.samples.pop_front();
        }
        let (time, previous) = self.samples.front().copied().unwrap_or((elapsed, bytes));
        let seconds = elapsed.saturating_sub(time).as_secs_f64();
        let rate = if seconds > 0. {
            (bytes.saturating_sub(previous) as f64 / seconds) as u64
        } else {
            0
        };
        let changed = self.bytes_per_second != rate;
        self.bytes_per_second = rate;
        changed
    }

    pub fn remaining(&self, transferred: u64, total: Option<u64>) -> Option<u64> {
        let total = total?;
        (self.bytes_per_second > 0).then(|| {
            total
                .saturating_sub(transferred)
                .div_ceil(self.bytes_per_second)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_uses_elapsed_time_and_expires_when_transfer_stalls() {
        let mut rate = TransferRate::default();
        rate.sample(Duration::ZERO, 0);
        rate.sample(Duration::from_secs(1), 1024);
        assert_eq!(rate.bytes_per_second, 1024);
        assert_eq!(rate.remaining(1024, Some(3073)), Some(3));
        assert_eq!(rate.remaining(1024, None), None);
        rate.sample(Duration::from_secs(2), 1024);
        rate.sample(Duration::from_secs(3), 1024);
        rate.sample(Duration::from_secs(4), 1024);
        assert_eq!(rate.bytes_per_second, 0);
        assert_eq!(rate.remaining(1024, Some(3073)), None);
    }
}
