//! The four figures a column group holds of repeated measurements: the
//! least, the median, the 95th percentile and the greatest. No mean: one
//! slow repeat on a machine doing something else should show as the
//! greatest and move nothing else.

/// Figures over a set of samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Figures {
    pub samples: u32,
    pub min: f64,
    /// The middle sample, or the mean of the two middle ones.
    pub p50: f64,
    /// By nearest rank: the smallest sample with at least 95 in 100 of the
    /// samples at or below it.
    pub p95: f64,
    /// The same at 99 in 100.
    pub p99: f64,
    pub max: f64,
}

/// The sample at `percent` by nearest rank, of sorted samples.
fn rank(sorted: &[f64], percent: usize) -> f64 {
    let at = (sorted.len() * percent).div_ceil(100).max(1);
    sorted[at - 1]
}

/// The figures of `samples`; `None` of no samples, or of any that is not
/// a number.
pub fn figures(samples: &[f64]) -> Option<Figures> {
    if samples.is_empty() || samples.iter().any(|x| x.is_nan()) {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    let p50 = if sorted.len() % 2 == 1 { sorted[mid] } else { (sorted[mid - 1] + sorted[mid]) / 2.0 };
    Some(Figures {
        samples: sorted.len() as u32,
        min: sorted[0],
        p50,
        p95: rank(&sorted, 95),
        p99: rank(&sorted, 99),
        max: sorted[sorted.len() - 1],
    })
}

/// The samples kept of a series: all but the first `discard`, which are
/// taken to warm the device up.
pub fn kept(samples: &[Option<f64>], discard: usize) -> (Vec<f64>, u32) {
    let rest = samples.get(discard..).unwrap_or(&[]);
    let timed: Vec<f64> = rest.iter().flatten().copied().collect();
    let untimed = (rest.len() - timed.len()) as u32;
    (timed, untimed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_figures_of_a_series() {
        let one = figures(&[7.5]).unwrap();
        assert_eq!((one.samples, one.min, one.p50, one.p95, one.p99, one.max), (1, 7.5, 7.5, 7.5, 7.5, 7.5));
        let two = figures(&[4.0, 2.0]).unwrap();
        assert_eq!((two.min, two.p50, two.p95, two.max), (2.0, 3.0, 4.0, 4.0));
        // Thirty repeats, as a default run keeps: 1 to 30.
        let thirty: Vec<f64> = (1..=30).rev().map(f64::from).collect();
        let f = figures(&thirty).unwrap();
        assert_eq!((f.samples, f.min, f.p50, f.max), (30, 1.0, 15.5, 30.0));
        // 95 in 100 of 30 is 28.5: the 29th; 99 in 100 is 29.7: the 30th.
        assert_eq!((f.p95, f.p99), (29.0, 30.0));
        // A hundred: the 95th and the 99th themselves.
        let hundred: Vec<f64> = (1..=100).map(f64::from).collect();
        let f = figures(&hundred).unwrap();
        assert_eq!((f.p50, f.p95, f.p99), (50.5, 95.0, 99.0));
        // One slow repeat is the greatest and nothing else.
        let mut slow = vec![1.0; 29];
        slow.push(900.0);
        let f = figures(&slow).unwrap();
        assert_eq!((f.p50, f.p95, f.max), (1.0, 1.0, 900.0));
    }

    #[test]
    fn no_samples_and_samples_that_are_no_numbers_have_no_figures() {
        assert_eq!(figures(&[]), None);
        assert_eq!(figures(&[1.0, f64::NAN]), None);
    }

    #[test]
    fn the_first_repeats_are_discarded_and_untimed_ones_counted() {
        let series = [Some(9.0), None, Some(8.0), Some(1.0), None, Some(2.0)];
        assert_eq!(kept(&series, 3), (vec![1.0, 2.0], 1));
        assert_eq!(kept(&series, 0), (vec![9.0, 8.0, 1.0, 2.0], 2));
        assert_eq!(kept(&series, 10), (vec![], 0));
    }
}
