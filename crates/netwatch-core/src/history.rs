//! Fixed-length rolling history of rate samples.

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RatePoint {
    /// Bytes per second received.
    pub down: f64,
    /// Bytes per second sent.
    pub up: f64,
}

/// Keeps the most recent `capacity` points, oldest first.
#[derive(Clone, Debug)]
pub struct History {
    points: VecDeque<RatePoint>,
    capacity: usize,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            points: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    pub fn push(&mut self, p: RatePoint) {
        if self.points.len() == self.capacity {
            self.points.pop_front();
        }
        self.points.push_back(p);
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &RatePoint> + ExactSizeIterator {
        self.points.iter()
    }

    /// The last `n` points (fewer if not yet collected), oldest first.
    pub fn tail(&self, n: usize) -> Vec<RatePoint> {
        let skip = self.points.len().saturating_sub(n);
        self.points.iter().skip(skip).copied().collect()
    }

    /// Largest rate in either direction across `points`.
    pub fn peak(points: &[RatePoint]) -> f64 {
        points.iter().fold(0.0, |m, p| m.max(p.down).max(p.up))
    }
}

/// Group a full history window of `capacity` samples into `n` bars, oldest
/// first, each the busiest sample (per direction) in its slot. Slots the
/// history hasn't reached yet are `None`, so a fresh graph fills in from the
/// right like a full one scrolls.
pub fn bars(points: &[RatePoint], capacity: usize, n: usize) -> Vec<Option<RatePoint>> {
    let n = n.max(1);
    let per = capacity.div_ceil(n).max(1);
    // Align to the newest sample: the last slot always holds the latest one.
    let missing = (n * per).saturating_sub(points.len());
    (0..n)
        .map(|i| {
            let start = (i * per).saturating_sub(missing);
            let end = ((i + 1) * per).saturating_sub(missing).min(points.len());
            (start < end).then(|| {
                points[start..end]
                    .iter()
                    .fold(RatePoint::default(), |m, p| RatePoint {
                        down: m.down.max(p.down),
                        up: m.up.max(p.up),
                    })
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(down: f64) -> RatePoint {
        RatePoint { down, up: 0.0 }
    }

    #[test]
    fn drops_the_oldest_point_when_full() {
        let mut h = History::new(3);
        for v in 1..=5 {
            h.push(p(v as f64));
        }
        let v: Vec<f64> = h.iter().map(|p| p.down).collect();
        assert_eq!(v, [3.0, 4.0, 5.0]);
        assert_eq!(h.tail(2), [p(4.0), p(5.0)]);
        assert_eq!(h.tail(10).len(), 3);
    }

    #[test]
    fn bars_take_the_busiest_sample_per_slot() {
        let pts: Vec<RatePoint> = (1..=6).map(|v| p(v as f64)).collect();
        let b = bars(&pts, 6, 3);
        let down: Vec<Option<f64>> = b.iter().map(|b| b.map(|p| p.down)).collect();
        assert_eq!(down, [Some(2.0), Some(4.0), Some(6.0)]);
    }

    #[test]
    fn bars_fill_in_from_the_right() {
        let pts = [p(1.0), p(2.0), p(3.0)];
        let b = bars(&pts, 6, 3);
        let down: Vec<Option<f64>> = b.iter().map(|b| b.map(|p| p.down)).collect();
        assert_eq!(down, [None, Some(1.0), Some(3.0)]);
        assert!(bars(&[], 6, 3).iter().all(Option::is_none));
    }

    #[test]
    fn peak_covers_both_directions() {
        let pts = [p(3.0), RatePoint { down: 1.0, up: 7.0 }];
        assert_eq!(History::peak(&pts), 7.0);
        assert_eq!(History::peak(&[]), 0.0);
    }
}
