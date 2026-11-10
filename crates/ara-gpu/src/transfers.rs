//! Shared transfer range validation, before native encoding.
use crate::{GpuError, Result};

pub(crate) fn validate_fill(offset: u64, size: u64, capacity: u64) -> Result<()> {
    if size == 0
        || !size.is_multiple_of(4)
        || !offset.is_multiple_of(4)
        || offset.checked_add(size).is_none_or(|end| end > capacity)
    {
        return Err(GpuError::Contract("Invalid buffer fill range".into()));
    }
    Ok(())
}
