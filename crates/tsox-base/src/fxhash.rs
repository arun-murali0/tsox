use std::hash::{BuildHasher, Hasher};

// A small reimplementation of rustc's FxHash (the multiply-rotate-xor scheme the Rust
// compiler itself uses internally), kept in-tree instead of adding the `rustc-hash`
// crate as a dependency for one hasher. Not DoS-resistant, so it must never be used
// for anything hashing attacker-controlled input. It exists only for the checker's
// own internal maps (see TypeNamespace), where the keys are type and parameter names
// from source the checker itself is compiling, and speed matters more than
// hash-flooding resistance.
const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default)]
pub struct FxHasher {
    hash: u64,
}

// The normal compiler build always starts each hash at zero, preserving the original
// FxHash behavior. Unit tests can choose a different initial state before constructing
// maps, which changes bucket/iteration order without changing map semantics. The seed is
// also compiled in under the `test-hash-seed` feature, because `cfg(test)` is true only for
// this crate's own tests: a crate that depends on tsox-base and wants to vary hash order in
// its tests (the project driver's determinism tests) turns the feature on in its
// dev-dependencies.
#[cfg(any(test, feature = "test-hash-seed"))]
pub static TEST_HASH_SEED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
pub struct FxBuildHasher {
    initial_hash: u64,
}

impl Default for FxBuildHasher {
    fn default() -> Self {
        #[cfg(any(test, feature = "test-hash-seed"))]
        let initial_hash = TEST_HASH_SEED.load(std::sync::atomic::Ordering::Relaxed);
        #[cfg(not(any(test, feature = "test-hash-seed")))]
        let initial_hash = 0;
        Self { initial_hash }
    }
}

impl BuildHasher for FxBuildHasher {
    type Hasher = FxHasher;

    #[inline]
    fn build_hasher(&self) -> Self::Hasher {
        FxHasher {
            hash: self.initial_hash,
        }
    }
}

impl FxHasher {
    #[inline]
    fn add_to_hash(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, mut bytes: &[u8]) {
        while bytes.len() >= 8 {
            let mut word = [0u8; 8];
            word.copy_from_slice(&bytes[..8]);
            self.add_to_hash(u64::from_ne_bytes(word));
            bytes = &bytes[8..];
        }
        if bytes.len() >= 4 {
            let mut word = [0u8; 4];
            word.copy_from_slice(&bytes[..4]);
            self.add_to_hash(u32::from_ne_bytes(word) as u64);
            bytes = &bytes[4..];
        }
        if bytes.len() >= 2 {
            let mut word = [0u8; 2];
            word.copy_from_slice(&bytes[..2]);
            self.add_to_hash(u16::from_ne_bytes(word) as u64);
            bytes = &bytes[2..];
        }
        if let Some(&byte) = bytes.first() {
            self.add_to_hash(byte as u64);
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add_to_hash(i as u64);
    }
    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add_to_hash(i as u64);
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add_to_hash(i as u64);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add_to_hash(i);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add_to_hash(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

pub type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuildHasher>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::Hash;

    #[test]
    fn same_value_hashes_identically_every_time() {
        let mut a = FxHasher::default();
        let mut b = FxHasher::default();
        "some-type-name".hash(&mut a);
        "some-type-name".hash(&mut b);
        assert_eq!(a.finish(), b.finish());
    }

    #[test]
    fn different_values_usually_hash_differently() {
        let mut a = FxHasher::default();
        let mut b = FxHasher::default();
        "Foo".hash(&mut a);
        "Bar".hash(&mut b);
        assert_ne!(a.finish(), b.finish());
    }

    #[test]
    fn works_as_an_actual_hashmap_hasher() {
        let mut map: FxHashMap<String, i32> = FxHashMap::default();
        map.insert("a".to_string(), 1);
        map.insert("b".to_string(), 2);
        assert_eq!(map.get("a"), Some(&1));
        assert_eq!(map.get("b"), Some(&2));
        assert_eq!(map.get("c"), None);
    }
}
