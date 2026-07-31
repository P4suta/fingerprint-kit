#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod bundle;
mod domain;
mod engine;
mod error;
mod io_util;
pub mod protocol;
mod source;
mod synthetic;
mod template;

pub use bundle::{load_capture_bundle, save_capture_bundle};
pub use domain::{
    CanonicalImage, CaptureKind, CaptureMetadata, MinutiaRecord, TemplateRecord, TemplateSample,
    VerificationResult,
};
pub use engine::{
    ACCEPT_THRESHOLD, ENGINE_ID, ENGINE_VERSION, ENROLLMENT_SAMPLES, Inspection,
    MIN_MEAN_MINUTIA_QUALITY, POLICY_NAME, inspect_capture, verify,
};
pub use error::{Error, Result};
pub use synthetic::{
    SYNTHETIC_CAPTURE_PROFILE, SYNTHETIC_HEIGHT, SYNTHETIC_PPI, SYNTHETIC_WIDTH, generate_synthetic,
};
pub use template::{load_template, save_template};

use std::path::{Path, PathBuf};

/// Inspect a capture bundle after replaying and validating its image and metadata.
pub fn inspect_bundle(path: &Path) -> Result<Inspection> {
    let (image, metadata) = source::capture_replay(path)?;
    inspect_capture(&image, &metadata)
}

/// Enroll exactly three replay capture bundles and create a new versioned template file.
pub fn enroll_bundles(captures: &[PathBuf], out: &Path) -> Result<TemplateRecord> {
    let record = engine::enroll_paths(captures)?;
    save_template(out, &record)?;
    Ok(record)
}

/// Verify a replay capture bundle against a versioned template file.
pub fn verify_bundle(template_path: &Path, capture_path: &Path) -> Result<VerificationResult> {
    let template = load_template(template_path)?;
    let (image, metadata) = source::capture_replay(capture_path)?;
    verify(&template, &image, &metadata)
}

/// Create one deterministic synthetic capture bundle without overwriting an existing path.
pub fn create_synthetic_bundle(identity: u64, impression: u64, out: &Path) -> Result<()> {
    let (image, metadata) = source::capture_synthetic(identity, impression)?;
    save_capture_bundle(out, &image, &metadata)
}

/// Run the complete hardware-free M0 vertical slice in a temporary directory.
pub fn run_demo() -> Result<(VerificationResult, VerificationResult)> {
    let temp = tempfile::tempdir().map_err(Error::io)?;
    let enrollment: Vec<PathBuf> = (0..ENROLLMENT_SAMPLES)
        .map(|index| temp.path().join(format!("enroll-{index}")))
        .collect();

    for (index, path) in enrollment.iter().enumerate() {
        create_synthetic_bundle(41, index as u64, path)?;
    }

    let genuine_path = temp.path().join("genuine");
    let impostor_path = temp.path().join("impostor");
    let template_path = temp.path().join("template.json");
    create_synthetic_bundle(41, 90, &genuine_path)?;
    create_synthetic_bundle(99, 0, &impostor_path)?;
    enroll_bundles(&enrollment, &template_path)?;

    let genuine = verify_bundle(&template_path, &genuine_path)?;
    let impostor = verify_bundle(&template_path, &impostor_path)?;
    if !genuine.matched || impostor.matched {
        return Err(Error::processing(
            "demo policy expectation failed: synthetic fixture is not discriminating",
        ));
    }
    Ok((genuine, impostor))
}
