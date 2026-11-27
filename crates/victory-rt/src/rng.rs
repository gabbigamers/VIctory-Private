//! The generator's random numbers: SplitMix64 (Steele, Lea and Flood, 2014;
//! the constants are the published ones). Every value handed out is made
//! with integer arithmetic and exact conversions only, so a seed gives the
//! same scene on every platform.

#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// A number below `n` (`n` > 0). The bias of the multiply-shift is far
    /// below what a test scene can see.
    pub fn below(&mut self, n: u32) -> u32 {
        ((u64::from(self.next_u32()) * u64::from(n)) >> 32) as u32
    }

    /// An integer in `lo..=hi`.
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1) as u32) as i32
    }

    /// True `num` times in `den`.
    pub fn chance(&mut self, num: u32, den: u32) -> bool {
        self.below(den) < num
    }

    /// A multiple of 2^-24 in [0, 1): 24 random bits, exact in an `f32`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    /// An integer in `lo..=hi` times 2^-`shift`: a coordinate with few
    /// significant bits, so sums and differences of such numbers are exact.
    pub fn dyadic(&mut self, lo: i32, hi: i32, shift: u32) -> f32 {
        self.range(lo, hi) as f32 / (1u32 << shift) as f32
    }

    pub fn pick<'a, T>(&mut self, of: &'a [T]) -> &'a T {
        &of[self.below(of.len() as u32) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_gives_its_published_sequence() {
        // The first outputs for seed 1234567, as the reference
        // implementation prints them.
        let mut r = Rng::new(1_234_567);
        assert_eq!(r.next_u64(), 6_457_827_717_110_365_317);
        assert_eq!(r.next_u64(), 3_203_168_211_198_807_973);
        assert_eq!(r.next_u64(), 9_817_491_932_198_370_423);
    }

    #[test]
    fn values_stay_in_their_ranges() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            let u = r.unit();
            assert!((0.0..1.0).contains(&u));
            assert!((-5..=9).contains(&r.range(-5, 9)));
            let d = r.dyadic(-64, 64, 4);
            assert!((-4.0..=4.0).contains(&d) && (d * 16.0).fract() == 0.0);
            assert!(r.below(3) < 3);
        }
    }
}
