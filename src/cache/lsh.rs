/// Random projection locality-sensitive hashing.
///
/// Time: hashing is O(d * b), where d is the vector dimension and b is the number of bits.
/// Space: O(d * b) for the projection table, bounded by construction.
#[derive(Debug)]
pub struct RandomProjectionLsh {
    projections: Vec<Vec<i8>>,
}

impl RandomProjectionLsh {
    pub fn new(vector_dimension: usize, num_bits: usize, seed: u64) -> Self {
        let mut state = seed;
        let mut projections = Vec::with_capacity(num_bits);

        for _ in 0..num_bits {
            let mut projection = Vec::with_capacity(vector_dimension);

            for _ in 0..vector_dimension {
                state = splitmix64(state);
                let bit = if state & 1 == 0 { -1 } else { 1 };
                projection.push(bit);
            }

            projections.push(projection);
        }

        Self { projections }
    }

    pub fn hash(&self, vector: &[i8]) -> u64 {
        let mut hash = 0_u64;

        for (bit_index, projection) in self.projections.iter().enumerate() {
            let mut score = 0_i32;
            for (value, weight) in vector.iter().zip(projection.iter()) {
                score += i32::from(*value) * i32::from(*weight);
            }

            if score >= 0 {
                hash |= 1_u64 << bit_index;
            }
        }

        hash
    }
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut result = value;
    result = (result ^ (result >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    result = (result ^ (result >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    result ^ (result >> 31)
}
