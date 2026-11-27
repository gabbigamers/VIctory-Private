//! Exact arithmetic on binary floating-point inputs. Every `f32` and `f64`
//! is an integer times a power of two, and sums, differences and products
//! of such numbers are again; [`Dy`] holds one exactly, as a sign, a
//! magnitude of 32-bit limbs and an exponent counted in limbs. There is no
//! division: the reference compares ratios by cross-multiplying, so an edge
//! function's sign, an equal-`t` tie and a hit exactly on `tmin` are decided
//! without a tolerance.

use std::cmp::Ordering;
use std::ops::{Add, Mul, Neg, Sub};

/// `(-1)^neg * mag * 2^(32 * exp)`; `mag` least significant limb first, no
/// zero limb at either end, empty for zero (then `neg` is false and `exp`
/// 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dy {
    neg: bool,
    mag: Vec<u32>,
    exp: i32,
}

impl Dy {
    pub fn zero() -> Self {
        Dy {
            neg: false,
            mag: Vec::new(),
            exp: 0,
        }
    }

    fn normalised(neg: bool, mut mag: Vec<u32>, mut exp: i32) -> Self {
        while mag.last() == Some(&0) {
            mag.pop();
        }
        let low = mag.iter().take_while(|&&l| l == 0).count();
        if low > 0 {
            mag.drain(..low);
            exp += low as i32;
        }
        if mag.is_empty() {
            return Dy::zero();
        }
        Dy { neg, mag, exp }
    }

    /// Exactly `x`, which must be finite.
    pub fn from_f64(x: f64) -> Self {
        assert!(x.is_finite(), "exact arithmetic on {x}");
        let bits = x.to_bits();
        let (e, frac) = (((bits >> 52) & 0x7FF) as i32, bits & ((1 << 52) - 1));
        // x = mant * 2^k.
        let (mant, k) = if e == 0 {
            (frac, -1074)
        } else {
            (frac | (1 << 52), e - 1075)
        };
        let (q, r) = (k.div_euclid(32), k.rem_euclid(32));
        let wide = u128::from(mant) << r;
        let mag = vec![wide as u32, (wide >> 32) as u32, (wide >> 64) as u32];
        Dy::normalised(bits >> 63 != 0, mag, q)
    }

    pub fn from_f32(x: f32) -> Self {
        Dy::from_f64(f64::from(x))
    }

    pub fn from_i64(x: i64) -> Self {
        let m = x.unsigned_abs();
        Dy::normalised(x < 0, vec![m as u32, (m >> 32) as u32], 0)
    }

    pub fn is_zero(&self) -> bool {
        self.mag.is_empty()
    }

    /// -1, 0 or 1.
    pub fn sign(&self) -> i32 {
        match (self.is_zero(), self.neg) {
            (true, _) => 0,
            (false, true) => -1,
            (false, false) => 1,
        }
    }

    /// The magnitude with its exponent lowered to `exp`.
    fn aligned(&self, exp: i32) -> Vec<u32> {
        let mut out = vec![0u32; (self.exp - exp) as usize];
        out.extend_from_slice(&self.mag);
        out
    }

    /// Magnitudes with no zero limb at the top.
    fn cmp_mag(a: &[u32], b: &[u32]) -> Ordering {
        a.len()
            .cmp(&b.len())
            .then_with(|| a.iter().rev().cmp(b.iter().rev()))
    }

    fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
        let mut carry = 0u64;
        for k in 0..a.len().max(b.len()) {
            let s = u64::from(a.get(k).copied().unwrap_or(0))
                + u64::from(b.get(k).copied().unwrap_or(0))
                + carry;
            out.push(s as u32);
            carry = s >> 32;
        }
        out.push(carry as u32);
        out
    }

    /// `a - b`, where `a >= b`.
    fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::with_capacity(a.len());
        let mut borrow = 0i64;
        for (k, &x) in a.iter().enumerate() {
            let d = i64::from(x) - i64::from(b.get(k).copied().unwrap_or(0)) - borrow;
            out.push(d as u32);
            borrow = i64::from(d < 0);
        }
        debug_assert_eq!(borrow, 0);
        out
    }

    fn sum(&self, other: &Dy, negate_other: bool) -> Dy {
        let other_neg = other.neg != negate_other;
        if self.is_zero() {
            return Dy::normalised(other_neg, other.mag.clone(), other.exp);
        }
        if other.is_zero() {
            return self.clone();
        }
        let exp = self.exp.min(other.exp);
        let (a, b) = (self.aligned(exp), other.aligned(exp));
        if self.neg == other_neg {
            return Dy::normalised(self.neg, Dy::add_mag(&a, &b), exp);
        }
        match Dy::cmp_mag(&a, &b) {
            Ordering::Equal => Dy::zero(),
            Ordering::Greater => Dy::normalised(self.neg, Dy::sub_mag(&a, &b), exp),
            Ordering::Less => Dy::normalised(other_neg, Dy::sub_mag(&b, &a), exp),
        }
    }

    fn product(&self, other: &Dy) -> Dy {
        if self.is_zero() || other.is_zero() {
            return Dy::zero();
        }
        let mut out = vec![0u32; self.mag.len() + other.mag.len()];
        for (i, &x) in self.mag.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &y) in other.mag.iter().enumerate() {
                let p = u64::from(x) * u64::from(y) + u64::from(out[i + j]) + carry;
                out[i + j] = p as u32;
                carry = p >> 32;
            }
            out[i + other.mag.len()] = carry as u32;
        }
        Dy::normalised(self.neg != other.neg, out, self.exp + other.exp)
    }

    /// The value as a fraction in (-2^96, 2^96) and a power of two: the top
    /// three limbs, so the fraction is right to a relative 2^-63.
    fn parts(&self) -> (f64, i64) {
        let n = self.mag.len();
        let mut m = 0f64;
        for k in (n.saturating_sub(3)..n).rev() {
            m = m * 4_294_967_296.0 + f64::from(self.mag[k]);
        }
        let m = if self.neg { -m } else { m };
        (m, 32 * (i64::from(self.exp) + n.saturating_sub(3) as i64))
    }

    /// The nearest `f64` to within a few units in the last place; infinite
    /// or zero past the type's range.
    pub fn to_f64(&self) -> f64 {
        let (m, e) = self.parts();
        scale2(m, e)
    }

    /// `num / den` as an `f64`, to within a few units in the last place.
    /// `den` must not be zero. A zero numerator gives +0.0 whatever the
    /// denominator's sign: the result is a number, and the records made of
    /// it are compared word for word.
    pub fn ratio(num: &Dy, den: &Dy) -> f64 {
        assert!(!den.is_zero(), "a ratio over zero");
        if num.is_zero() {
            return 0.0;
        }
        let ((mn, en), (md, ed)) = (num.parts(), den.parts());
        scale2(mn / md, en - ed)
    }
}

/// `x * 2^e` without an intermediate overflow.
fn scale2(mut x: f64, mut e: i64) -> f64 {
    // 2^512 and 2^-512.
    let (up, down) = (
        f64::from_bits(0x5FF0_0000_0000_0000),
        f64::from_bits(0x1FF0_0000_0000_0000),
    );
    while e >= 512 && x.is_finite() && x != 0.0 {
        x *= up;
        e -= 512;
    }
    while e <= -512 && x != 0.0 {
        x *= down;
        e += 512;
    }
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    // |e| < 512: 2^e is a normal number.
    x * f64::from_bits(((e + 1023) as u64) << 52)
}

impl Ord for Dy {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sum(other, true).sign().cmp(&0)
    }
}

impl PartialOrd for Dy {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Add for &Dy {
    type Output = Dy;
    fn add(self, other: &Dy) -> Dy {
        self.sum(other, false)
    }
}

impl Sub for &Dy {
    type Output = Dy;
    fn sub(self, other: &Dy) -> Dy {
        self.sum(other, true)
    }
}

impl Mul for &Dy {
    type Output = Dy;
    fn mul(self, other: &Dy) -> Dy {
        self.product(other)
    }
}

impl Neg for &Dy {
    type Output = Dy;
    fn neg(self) -> Dy {
        Dy::normalised(!self.neg, self.mag.clone(), self.exp)
    }
}

/// Three exact numbers.
pub type V3 = [Dy; 3];

pub fn v3(x: [f32; 3]) -> V3 {
    x.map(Dy::from_f32)
}

pub fn sub3(a: &V3, b: &V3) -> V3 {
    std::array::from_fn(|k| &a[k] - &b[k])
}

pub fn dot3(a: &V3, b: &V3) -> Dy {
    &(&(&a[0] * &b[0]) + &(&a[1] * &b[1])) + &(&a[2] * &b[2])
}

pub fn cross3(a: &V3, b: &V3) -> V3 {
    std::array::from_fn(|k| {
        let (i, j) = ((k + 1) % 3, (k + 2) % 3);
        &(&a[i] * &b[j]) - &(&a[j] * &b[i])
    })
}

/// `L p + T` for a 3x4 transform of `f32`.
pub fn transform3(m: &[f32; 12], p: &V3) -> V3 {
    std::array::from_fn(|r| {
        let row: V3 = std::array::from_fn(|c| Dy::from_f32(m[4 * r + c]));
        &dot3(&row, p) + &Dy::from_f32(m[4 * r + 3])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    fn of(x: i128) -> Dy {
        let m = x.unsigned_abs();
        Dy::normalised(
            x < 0,
            vec![
                m as u32,
                (m >> 32) as u32,
                (m >> 64) as u32,
                (m >> 96) as u32,
            ],
            0,
        )
    }

    #[test]
    fn integers_agree_with_i128() {
        let mut r = Rng::new(11);
        for _ in 0..20_000 {
            // Up to 62 bits each, with small and zero values among them.
            let pick = |r: &mut Rng| -> i128 {
                let x = ((r.next_u64() >> r.below(64)) >> 2) as i64;
                i128::from(if r.chance(1, 2) { -x } else { x })
            };
            let (a, b) = (pick(&mut r), pick(&mut r));
            let (da, db) = (Dy::from_i64(a as i64), Dy::from_i64(b as i64));
            assert_eq!(&da + &db, of(a + b));
            assert_eq!(&da - &db, of(a - b));
            assert_eq!(&da * &db, of(a * b));
            assert_eq!(da.cmp(&db), a.cmp(&b));
            assert_eq!((-&da).sign(), (-a).signum() as i32);
        }
    }

    #[test]
    fn floats_convert_exactly_and_scale() {
        for x in [
            0.0,
            1.0,
            -1.5,
            3.0e-320,
            f64::MIN_POSITIVE,
            f64::MAX,
            0.1,
            -123_456.789e40,
            f64::EPSILON,
        ] {
            assert_eq!(Dy::from_f64(x).to_f64(), x, "{x:e}");
        }
        // 2^-1074 squared is not an f64, but it is a Dy; times 2^1000 twice
        // it is an f64 again.
        let tiny = Dy::from_f64(f64::from_bits(1));
        let sq = &tiny * &tiny;
        assert_eq!(sq.to_f64(), 0.0);
        assert_eq!(sq.sign(), 1);
        let p = Dy::from_f64(f64::from_bits((1023u64 + 1000) << 52));
        let back = &(&sq * &p) * &p;
        assert_eq!(back.to_f64(), f64::from_bits((1023u64 - 148) << 52));
    }

    #[test]
    fn sums_that_an_f64_rounds_stay_exact() {
        let (a, b) = (Dy::from_f64(1.0), Dy::from_f64(1e-30));
        let s = &a + &b;
        assert_ne!(s, a);
        assert_eq!(&s - &a, b);
        assert_eq!(s.cmp(&a), Ordering::Greater);
        // 0.1 + 0.2 is not 0.3 in f64, and exactly so here.
        let lhs = &Dy::from_f64(0.1) + &Dy::from_f64(0.2);
        assert_eq!(lhs.cmp(&Dy::from_f64(0.3)), Ordering::Greater);
        assert_eq!(Dy::from_f64(0.1 + 0.2).cmp(&lhs), Ordering::Greater);
    }

    #[test]
    fn products_of_floats_agree_with_wide_integers() {
        let mut r = Rng::new(5);
        for _ in 0..5_000 {
            // Integers below 2^53 times small powers of two.
            let pick = |r: &mut Rng| -> (i64, i32) {
                let m = (r.next_u64() >> 11) as i64;
                (if r.chance(1, 2) { -m } else { m }, r.range(-40, 40))
            };
            let ((ma, ea), (mb, eb)) = (pick(&mut r), pick(&mut r));
            let f = |m: i64, e: i32| m as f64 * f64::from_bits(((e + 1023) as u64) << 52);
            let (a, b) = (Dy::from_f64(f(ma, ea)), Dy::from_f64(f(mb, eb)));
            // a * b * 2^80 against the integer product.
            let shift = Dy::from_f64(f(1, 80 - ea - eb));
            assert_eq!(
                &(&a * &b) * &shift,
                &of(i128::from(ma) * i128::from(mb)) * &Dy::from_f64(f(1, 80))
            );
        }
    }

    #[test]
    fn ratios_come_out_to_the_last_places() {
        let mut r = Rng::new(9);
        for _ in 0..5_000 {
            let (a, b) = (f64::from(r.unit()) - 0.5, f64::from(r.unit()) + 0.25);
            let got = Dy::ratio(&Dy::from_f64(a), &Dy::from_f64(b));
            assert!(
                (got - a / b).abs() <= 4.0 * f64::EPSILON * (a / b).abs(),
                "{a} / {b}"
            );
        }
        // Far past an f64's range on both sides, the ratio is fine.
        let huge = Dy::from_f64(f64::MAX);
        let (n, d) = (
            &(&huge * &huge) * &Dy::from_f64(3.0),
            &(&huge * &huge) * &Dy::from_f64(4.0),
        );
        assert!((Dy::ratio(&n, &d) - 0.75).abs() <= 4.0 * f64::EPSILON);
    }

    #[test]
    fn a_zero_numerator_gives_positive_zero_over_either_sign() {
        // 0.0 / -3.0 is -0.0 in floating point; the ratio of exact numbers
        // has no negative zero.
        for den in [3.0, -3.0, f64::MIN_POSITIVE, -f64::MAX] {
            let got = Dy::ratio(&Dy::zero(), &Dy::from_f64(den));
            assert_eq!(got.to_bits(), 0, "0 / {den:e}");
        }
        // A difference that cancels exactly is the same zero.
        let (a, b) = (Dy::from_f64(0.375), Dy::from_f64(-0.375));
        assert_eq!(Dy::ratio(&(&a + &b), &Dy::from_f64(-1.0)).to_bits(), 0);
        // A negative ratio keeps its sign.
        assert_eq!(Dy::ratio(&a, &Dy::from_f64(-3.0)), -0.125);
        assert_eq!(Dy::zero().to_f64().to_bits(), 0);
    }

    #[test]
    fn vectors_do_what_their_names_say() {
        let (a, b) = (v3([1.0, 2.0, 3.0]), v3([-4.0, 0.5, 6.0]));
        assert_eq!(dot3(&a, &b).to_f64(), 15.0);
        let c = cross3(&a, &b);
        assert_eq!(c.each_ref().map(Dy::to_f64), [10.5, -18.0, 8.5]);
        assert!(dot3(&a, &c).is_zero() && dot3(&b, &c).is_zero());
        let m = [0.0, 2.0, 0.0, 1.0, 0.0, 0.0, 1.0, 2.0, 1.0, 0.0, 0.0, 3.0];
        let p = transform3(&m, &a);
        assert_eq!(p.each_ref().map(Dy::to_f64), [5.0, 5.0, 4.0]);
    }
}
