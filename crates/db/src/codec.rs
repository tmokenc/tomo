//! Codec helpers shared by stats / future modules.

/// Encode a `u64` as 8 little-endian bytes — counters keep a simple format
/// so the database files are inspectable with `hexdump` if needed.
pub fn u64_to_bytes(v: u64) -> [u8; 8] {
    v.to_le_bytes()
}

pub fn u64_from_bytes(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let n = bytes.len().min(8);
    buf[..n].copy_from_slice(&bytes[..n]);
    u64::from_le_bytes(buf)
}
