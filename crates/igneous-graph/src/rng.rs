//! A tiny seeded random number generator, so layouts are reproducible.

/// SplitMix64: fast, good enough for jitter, and identical on every platform.
#[derive(Debug, Clone)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `[0, 1)`.
    pub(crate) fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// A number in `[-0.5, 0.5) × 1e-6`: d3's "jiggle", used to push apart
    /// nodes that sit exactly on top of each other.
    pub(crate) fn jiggle(&mut self) -> f32 {
        (self.next_f32() - 0.5) * 1e-6
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let x = Rng::new(8).next_f32();
        assert!((0.0..1.0).contains(&x));
        assert_ne!(Rng::new(7).next_u64(), Rng::new(8).next_u64());
    }
}
