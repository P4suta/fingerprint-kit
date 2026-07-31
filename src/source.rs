use crate::synthetic::SyntheticSource;
use crate::{CanonicalImage, CaptureMetadata, Result, load_capture_bundle};
use std::path::{Path, PathBuf};

pub(crate) trait CaptureSource {
    fn capture(&mut self) -> Result<(CanonicalImage, CaptureMetadata)>;
}

pub(crate) struct ReplaySource {
    directory: PathBuf,
}

impl ReplaySource {
    pub(crate) fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
        }
    }
}

impl CaptureSource for ReplaySource {
    fn capture(&mut self) -> Result<(CanonicalImage, CaptureMetadata)> {
        load_capture_bundle(&self.directory)
    }
}

pub(crate) fn capture_replay(path: &Path) -> Result<(CanonicalImage, CaptureMetadata)> {
    ReplaySource::new(path).capture()
}

pub(crate) fn capture_synthetic(
    identity: u64,
    impression: u64,
) -> Result<(CanonicalImage, CaptureMetadata)> {
    SyntheticSource::new(identity, impression).capture()
}
