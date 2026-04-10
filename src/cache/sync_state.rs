use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SyncState {
    // Map of File Path -> SHA-256 Hash
    pub files: HashMap<PathBuf, String>,
}

impl SyncState {
    const STATE_FILE: &'static str = ".sync_state.json";

    /// Load the ledger from disk when the server starts
    pub fn load() -> Self {
        match fs::read_to_string(Self::STATE_FILE) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(), // Create a new state if the file doesn't exist
        }
    }

    /// Save the ledger to disk (Atomic overwrite)
    pub fn save(&self) {
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = fs::write(Self::STATE_FILE, content);
        }
    }

    /// Return `true` if it is a new file or has been changed
    pub fn is_modified(&self, path: &PathBuf, current_hash: &str) -> bool {
        match self.files.get(path) {
            Some(saved_hash) => saved_hash != current_hash,
            None => true,
        }
    }

    /// Update the hash after a successful embedding
    pub fn update_file(&mut self, path: PathBuf, new_hash: String) {
        self.files.insert(path, new_hash);
        self.save();
    }

    pub async fn compute_hash_stream(path: &Path) -> anyhow::Result<String> {
        // Open the file (acquires file descriptor; doesn't load data into memory yet)
        let mut file = tokio::fs::File::open(path).await?;
        let mut hasher = Sha256::new();

        // Allocate a static 64KB buffer (65,536 bytes)
        // This is the ONLY memory footprint maintained during the hashing process!
        let mut buffer = vec![0u8; 65536].into_boxed_slice();

        loop {
            // Read 64KB chunks from disk into the buffer
            let bytes_read = file.read(&mut buffer).await?;

            // If no more bytes are read -> End of File (EOF)
            if bytes_read == 0 {
                break;
            }

            // Feed the bytes just read into the hasher for processing
            hasher.update(&buffer[..bytes_read]);
        }

        // Finalize and convert the result to a 64-character Hex string
        let hash = hasher.finalize();
        let mut result = String::with_capacity(hash.len() * 2);
        for byte in hash {
            use std::fmt::Write as _;
            let _ = write!(&mut result, "{byte:02x}");
        }
        Ok(result)
    }
}
