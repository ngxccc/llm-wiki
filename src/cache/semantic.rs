use crate::cache::lsh::RandomProjectionLsh;
use rustc_hash::FxHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{Arc, Mutex};

type FxHashSet = HashSet<u64, BuildHasherDefault<FxHasher>>;

const DEFAULT_GREY_ZONE_JACCARD: f32 = 0.25;

/// Quantized embedding stored in the cache.
///
/// Time: quantization is O(d).
/// Space: O(d) with i8 storage instead of f32, a 4x reduction before overhead.
#[derive(Debug, Clone)]
pub struct QuantizedVector {
    pub values: Box<[i8]>,
    pub scale: f32,
    pub norm: f32,
}

/// Cache probe outcome used by the MCP search backend.
#[derive(Debug, Clone)]
pub enum CacheOutcome<V> {
    SureHit { value: Arc<V> },
    GreyZone { value: Arc<V> },
    Miss,
}

#[derive(Debug)]
struct CacheEntry<V> {
    query: Arc<str>,
    trigrams: Arc<[u64]>,
    vector: QuantizedVector,
    value: Arc<V>,
    bucket: u64,
}

#[derive(Debug)]
struct CacheState<V> {
    entries: HashMap<Arc<str>, CacheEntry<V>>,
    buckets: HashMap<u64, Vec<Arc<str>>>,
    lru: VecDeque<Arc<str>>,
}

/// Memory-bounded semantic cache with LSH bucket lookup and lexical fallback.
///
/// Probe time: O(1) for bucket selection plus O(k * d) for k candidates in the bucket.
/// Space: O(n * d8) where d8 stores i8-quantized embeddings, plus bounded bucket metadata.
#[derive(Debug)]
pub struct SemanticCache<V> {
    vector_dimension: usize,
    max_entries: usize,
    lsh: RandomProjectionLsh,
    state: Mutex<CacheState<V>>,
}

impl<V> SemanticCache<V> {
    pub fn new(max_entries: usize, vector_dimension: usize) -> Self {
        Self {
            vector_dimension,
            max_entries,
            lsh: RandomProjectionLsh::new(vector_dimension, 64, 0x00C0_FFEE_u64),
            state: Mutex::new(CacheState {
                entries: HashMap::new(),
                buckets: HashMap::new(),
                lru: VecDeque::new(),
            }),
        }
    }

    pub fn vector_dimension(&self) -> usize {
        self.vector_dimension
    }

    pub fn insert(&self, query: &str, vector: &[f32], value: V) {
        let query_key: Arc<str> = Arc::from(query);
        let quantized = quantize_vector(vector);
        let bucket = self.lsh.hash(&quantized.values);
        let trigrams = trigram_fingerprint(query);
        let value = Arc::new(value);

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some(existing) = state.entries.insert(
            Arc::clone(&query_key),
            CacheEntry {
                query: Arc::clone(&query_key),
                trigrams,
                vector: quantized,
                value,
                bucket,
            },
        ) {
            remove_from_bucket(&mut state.buckets, existing.bucket, existing.query.as_ref());
        }

        state
            .lru
            .retain(|candidate| candidate.as_ref() != query_key.as_ref());
        state.lru.push_back(Arc::clone(&query_key));
        state.buckets.entry(bucket).or_default().push(query_key);
        evict_if_needed(self.max_entries, &mut state);
    }

    pub fn probe(&self, query: &str, vector: &[f32]) -> CacheOutcome<V> {
        let quantized = quantize_vector(vector);
        let bucket = self.lsh.hash(&quantized.values);
        let query_trigrams = trigram_fingerprint(query);

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(candidate_keys) = state.buckets.get(&bucket).cloned() else {
            return CacheOutcome::Miss;
        };

        let mut best: Option<(Arc<str>, f32, Arc<V>)> = None;

        for candidate_key in candidate_keys {
            let Some(entry) = state.entries.get(&candidate_key) else {
                continue;
            };

            let score = cosine_similarity(&quantized, &entry.vector);
            if best
                .as_ref()
                .is_none_or(|(_, best_score, _)| score > *best_score)
            {
                best = Some((Arc::clone(&entry.query), score, Arc::clone(&entry.value)));
            }
        }

        let Some((query_key, score, value)) = best else {
            return CacheOutcome::Miss;
        };

        if score >= 0.95 {
            touch_lru(&mut state.lru, &query_key);
            return CacheOutcome::SureHit { value };
        }

        if (0.85..0.95).contains(&score) {
            if let Some(entry) = state.entries.get(&query_key) {
                if jaccard_similarity(&query_trigrams, &entry.trigrams) >= DEFAULT_GREY_ZONE_JACCARD
                {
                    touch_lru(&mut state.lru, &query_key);
                    return CacheOutcome::GreyZone { value };
                }
            }
        }

        CacheOutcome::Miss
    }
}

/// Scalar quantization with i8 storage.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn quantize_vector(vector: &[f32]) -> QuantizedVector {
    let max_abs = vector
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0.0_f32, f32::max)
        .max(f32::EPSILON);
    let scale = max_abs / 127.0;

    let values = vector
        .iter()
        .copied()
        .map(|component| {
            let scaled = (component / scale).round();
            scaled.clamp(-127.0, 127.0) as i8
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();

    let norm = vector
        .iter()
        .map(|component| component * component)
        .sum::<f32>()
        .sqrt();

    QuantizedVector {
        values,
        scale,
        norm,
    }
}

fn cosine_similarity(left: &QuantizedVector, right: &QuantizedVector) -> f32 {
    if left.norm == 0.0 || right.norm == 0.0 {
        return 0.0;
    }

    let dot = left
        .values
        .iter()
        .zip(right.values.iter())
        .map(|(a, b)| f32::from(*a) * f32::from(*b))
        .sum::<f32>()
        * left.scale
        * right.scale;

    dot / (left.norm * right.norm)
}

fn trigram_fingerprint(text: &str) -> Arc<[u64]> {
    let mut fingerprint_set = FxHashSet::default();

    for window in text.as_bytes().windows(3) {
        let mut trigram_hasher = FxHasher::default();
        trigram_hasher.write(window);
        fingerprint_set.insert(trigram_hasher.finish());
    }

    let mut fingerprint = fingerprint_set.into_iter().collect::<Vec<_>>();
    fingerprint.sort_unstable();
    fingerprint.dedup();
    Arc::from(fingerprint.into_boxed_slice())
}

#[allow(clippy::cast_precision_loss)]
fn jaccard_similarity(left: &[u64], right: &[u64]) -> f32 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }

    let mut intersection = 0_usize;
    let mut left_index = 0_usize;
    let mut right_index = 0_usize;

    while left_index < left.len() && right_index < right.len() {
        match left[left_index].cmp(&right[right_index]) {
            std::cmp::Ordering::Less => left_index += 1,
            std::cmp::Ordering::Greater => right_index += 1,
            std::cmp::Ordering::Equal => {
                intersection += 1;
                left_index += 1;
                right_index += 1;
            }
        }
    }

    let union = left.len() + right.len() - intersection;
    intersection as f32 / union as f32
}

fn touch_lru(lru: &mut VecDeque<Arc<str>>, key: &Arc<str>) {
    lru.retain(|candidate| candidate.as_ref() != key.as_ref());
    lru.push_back(Arc::clone(key));
}

fn remove_from_bucket(buckets: &mut HashMap<u64, Vec<Arc<str>>>, bucket: u64, key: &str) {
    if let Some(entries) = buckets.get_mut(&bucket) {
        entries.retain(|candidate| candidate.as_ref() != key);
        if entries.is_empty() {
            buckets.remove(&bucket);
        }
    }
}

fn evict_if_needed<V>(max_entries: usize, state: &mut CacheState<V>) {
    while state.entries.len() > max_entries {
        let Some(victim) = state.lru.pop_front() else {
            break;
        };

        if let Some(entry) = state.entries.remove(victim.as_ref()) {
            remove_from_bucket(&mut state.buckets, entry.bucket, entry.query.as_ref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{jaccard_similarity, quantize_vector, SemanticCache};

    #[test]
    fn quantization_keeps_directional_signal() {
        let vector = quantize_vector(&[1.0, 2.0, 3.0]);
        assert_eq!(vector.values.len(), 3);
        assert!(vector.norm > 0.0);
    }

    #[test]
    fn jaccard_is_one_for_identical_trigrams() {
        let left = [1_u64, 2_u64, 3_u64];
        assert_eq!(jaccard_similarity(&left, &left), 1.0);
    }

    #[test]
    fn cache_can_return_a_sure_hit() {
        let cache = SemanticCache::new(8, 4);
        let vector = [0.9, 0.1, 0.2, 0.3];

        cache.insert("Arc<RwLock<T>>", &vector, String::from("cached result"));

        match cache.probe("Arc<RwLock<T>>", &vector) {
            super::CacheOutcome::SureHit { value, .. } => {
                assert_eq!(value.as_str(), "cached result");
            }
            other => panic!("unexpected cache outcome: {other:?}"),
        }
    }
}
