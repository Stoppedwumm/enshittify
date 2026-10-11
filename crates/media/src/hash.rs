//! BLAKE3 content hashing.

use std::path::Path;

use cutroom_core::MediaId;

/// Hashes a file. Large files are memory-mapped and hashed on the rayon pool.
/// Call from a background thread: a multi-GB file takes seconds.
pub fn hash_file(path: &Path) -> std::io::Result<MediaId> {
    let mut h = blake3::Hasher::new();
    h.update_mmap_rayon(path)?;
    Ok(MediaId::from_hash(h.finalize()))
}

/// Verifies that `path` has content hash `expected` (relink check).
/// A mismatching file is never accepted.
pub fn verify_file(path: &Path, expected: MediaId) -> std::io::Result<bool> {
    Ok(hash_file(path)? == expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_in_memory_hash() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin");
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        let id = hash_file(&p).unwrap();
        assert_eq!(id, MediaId::of_bytes(&data));
        assert!(verify_file(&p, id).unwrap());
        assert!(!verify_file(&p, MediaId::of_bytes(b"other")).unwrap());
    }
}
