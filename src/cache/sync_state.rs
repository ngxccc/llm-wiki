use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SyncState {
    // Map từ Đường dẫn File -> Mã SHA-256 Hash
    pub files: HashMap<PathBuf, String>,
}

impl SyncState {
    const STATE_FILE: &'static str = ".sync_state.json";

    /// Nạp sổ cái từ ổ cứng lúc bật Server
    pub fn load() -> Self {
        match fs::read_to_string(Self::STATE_FILE) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(), // File chưa có thì tạo mới
        }
    }

    /// Lưu sổ cái xuống ổ cứng (Ghi đè Atomic)
    pub fn save(&self) {
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = fs::write(Self::STATE_FILE, content);
        }
    }

    /// Trả về `true` nếu file MỚI hoặc BỊ SỬA ĐỔI
    pub fn is_modified(&self, path: &PathBuf, current_hash: &str) -> bool {
        match self.files.get(path) {
            Some(saved_hash) => saved_hash != current_hash,
            None => true, // File mới toanh!
        }
    }

    /// Cập nhật Hash mới sau khi Embed thành công
    pub fn update_file(&mut self, path: PathBuf, new_hash: String) {
        self.files.insert(path, new_hash);
        self.save(); // Chốt sổ!
    }

    pub async fn compute_hash_stream(path: &Path) -> anyhow::Result<String> {
        // Mở file (chỉ lấy File Descriptor, chưa load data)
        let mut file = tokio::fs::File::open(path).await?;
        let mut hasher = Sha256::new();

        // Cấp phát một Buffer tĩnh 64KB (65536 bytes)
        // Đây là bộ nhớ DUY NHẤT bị tốn trong suốt quá trình băm!
        let mut buffer = vec![0u8; 65536].into_boxed_slice();

        loop {
            // Hút 64KB từ ổ cứng vào Buffer
            let bytes_read = file.read(&mut buffer).await?;

            // Nếu hút không ra nước nữa -> Hết file (EOF)
            if bytes_read == 0 {
                break;
            }

            // Bơm số byte VỪA ĐỌC ĐƯỢC vào Hasher để nó trộn
            hasher.update(&buffer[..bytes_read]);
        }

        // Chốt sổ, ép ra chuỗi Hex 64 ký tự
        let hash = hasher.finalize();
        let mut result = String::with_capacity(hash.len() * 2);
        for byte in hash {
            use std::fmt::Write as _;
            let _ = write!(&mut result, "{byte:02x}");
        }
        Ok(result)
    }
}
