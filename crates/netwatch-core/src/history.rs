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
    fn peak_covers_both_directions() {
        let pts = [p(3.0), RatePoint { down: 1.0, up: 7.0 }];
        assert_eq!(History::peak(&pts), 7.0);
        assert_eq!(History::peak(&[]), 0.0);
    }
}
