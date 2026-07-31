use crate::{Error, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub(crate) fn read_bounded_regular_file(
    path: &Path,
    maximum_bytes: usize,
    label: &str,
) -> Result<Vec<u8>> {
    let file = File::open(path).map_err(Error::io)?;
    let metadata = file.metadata().map_err(Error::io)?;
    if !metadata.is_file() {
        return Err(Error::invalid(format!("{label} must be a regular file")));
    }
    if metadata.len() > u64::try_from(maximum_bytes).unwrap_or(u64::MAX) {
        return Err(Error::invalid(format!("{label} exceeds size limit")));
    }

    let read_limit = maximum_bytes
        .checked_add(1)
        .ok_or_else(|| Error::invalid(format!("{label} size limit is invalid")))?;
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(maximum_bytes)
            .min(maximum_bytes),
    );
    file.take(u64::try_from(read_limit).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(Error::io)?;
    if bytes.len() > maximum_bytes {
        return Err(Error::invalid(format!("{label} exceeds size limit")));
    }
    Ok(bytes)
}
