//! The only source of variation in any generated input.

/// splitmix64. Small, fixed, and specified by its code, so the corpora are a
/// function of this file alone.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// A stream from `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`. The modulo bias is irrelevant to a workload.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    /// A finite `f64` drawn uniformly over bit patterns, so every exponent
    /// and subnormals are represented.
    pub fn finite_f64(&mut self) -> f64 {
        loop {
            let value = f64::from_bits(self.next_u64());
            if value.is_finite() {
                return value;
            }
        }
    }

    /// One element of `items`.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}
