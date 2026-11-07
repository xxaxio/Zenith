//! Deterministic packet and frame mutator for fuzzing and adversary resilience testing.

/// Kinds of mutations that can be applied to valid Minecraft frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationKind {
    /// Inverts one bit at a pseudorandom position.
    BitFlip,
    /// Truncates the packet payload to test partial/broken frames.
    Truncate,
    /// Appends trailing garbage bytes to test stream framing boundaries.
    TrailingGarbage,
    /// Overwrites internal payload bytes with zeros.
    ZeroOut,
    /// Injects an oversized VarInt length header into the frame.
    OversizedHeader(i32),
}

/// Simple, zero-dependency deterministic Xorshift64Star PRNG.
pub struct MutatorPrng {
    state: u64,
}

impl MutatorPrng {
    pub const fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0xdead_beef_cafe_babe
            } else {
                seed
            },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn next_range(&mut self, min: usize, max: usize) -> usize {
        if min >= max {
            return min;
        }
        let range = (max - min) as u64;
        min + (self.next_u64() % range) as usize
    }
}

/// Mutates a valid packet payload according to a specific mutation strategy.
#[must_use]
pub fn mutate_packet(data: &[u8], kind: MutationKind, seed: u64) -> Vec<u8> {
    let mut prng = MutatorPrng::new(seed);
    let mut mutated = data.to_vec();

    match kind {
        MutationKind::BitFlip => {
            if !mutated.is_empty() {
                let idx = prng.next_range(0, mutated.len());
                let bit = (prng.next_u64() % 8) as u8;
                mutated[idx] ^= 1 << bit;
            }
        }
        MutationKind::Truncate => {
            if mutated.len() > 1 {
                let keep = prng.next_range(1, mutated.len());
                mutated.truncate(keep);
            }
        }
        MutationKind::TrailingGarbage => {
            let garbage_len = prng.next_range(4, 64);
            for _ in 0..garbage_len {
                mutated.push((prng.next_u64() & 0xFF) as u8);
            }
        }
        MutationKind::ZeroOut => {
            if mutated.len() > 2 {
                let start = prng.next_range(1, mutated.len() - 1);
                let end = prng.next_range(start + 1, mutated.len());
                for b in &mut mutated[start..end] {
                    *b = 0;
                }
            }
        }
        MutationKind::OversizedHeader(declared_len) => {
            let mut out = Vec::with_capacity(mutated.len() + 5);
            zenith_wire::write_varint(declared_len, &mut out);
            out.extend_from_slice(&mutated);
            return out;
        }
    }

    mutated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mutation_bit_flip() {
        let original = vec![1, 2, 3, 4, 5];
        let mutated = mutate_packet(&original, MutationKind::BitFlip, 42);
        assert_eq!(original.len(), mutated.len());
        assert_ne!(original, mutated);
    }

    #[test]
    fn test_mutation_truncate() {
        let original = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let mutated = mutate_packet(&original, MutationKind::Truncate, 42);
        assert!(mutated.len() < original.len());
        assert!(!mutated.is_empty());
    }

    #[test]
    fn test_oversized_header() {
        let payload = vec![0x00, 0x01];
        let framed = mutate_packet(&payload, MutationKind::OversizedHeader(65_536), 42);
        assert!(framed.len() > payload.len());
    }
}
