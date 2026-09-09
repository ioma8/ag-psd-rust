//! Shared read-time resource limits.
//!
//! These constants bound allocations derived from attacker-controlled counts
//! before any memory is reserved. They are deliberately conservative: the
//! underlying file bytes are the ultimate bound on data, and these limits stop
//! a tiny input from claiming a giant decoded buffer (e.g. a small PSD whose
//! header dimensions imply gigabytes of channel data).

/// Maximum bytes reserved for a single decoded pixel buffer (one channel plane,
/// one RGBA preview, or similar). Photoshop's maximum PSD dimension (30000)
/// yields at most ~3.6 GiB for a 32-bit channel plane, which no fixture in
/// this crate approaches; 1 GiB per buffer keeps legitimate large documents
/// readable while refusing hostile claims on small inputs.
pub const MAX_DECODED_BUFFER_BYTES: usize = 1 << 30;
pub const MAX_CONTAINER_ITEMS: u64 = 1_000_000;

/// Validate that a decoded buffer of `len` bytes is within the shared policy
/// limit, returning a structured error otherwise.
pub fn check_decoded_buffer(len: usize, what: &str) -> Result<(), crate::support::error::PsdError> {
    if len > MAX_DECODED_BUFFER_BYTES {
        return Err(crate::support::error::PsdError::InvalidFormat(format!(
            "{} requires {} bytes, exceeding the decode limit of {}; refusing to allocate",
            what, len, MAX_DECODED_BUFFER_BYTES
        )));
    }
    Ok(())
}

pub fn check_container_count(
    count: u64,
    what: &str,
) -> Result<(), crate::support::error::PsdError> {
    if count > MAX_CONTAINER_ITEMS {
        return Err(crate::support::error::PsdError::InvalidFormat(format!(
            "{} declares {} items, exceeding the limit of {}",
            what, count, MAX_CONTAINER_ITEMS
        )));
    }
    Ok(())
}
