#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod bundle;
mod domain;
mod engine;
mod error;
mod io_util;
pub mod moc;
pub mod protocol;
mod source;
mod synthetic;
mod template;
mod worker;

pub use bundle::{load_capture_bundle, save_capture_bundle};
pub use domain::{
    CanonicalImage, CaptureKind, CaptureMetadata, DeviceTemplate, HostImageTemplate,
    MAX_DEVICE_TEMPLATE_BYTES, MatchVerdict, MinutiaRecord, TemplateRecord, TemplateSample,
    VerificationResult,
};
pub use engine::{
    ACCEPT_THRESHOLD, DEVICE_POLICY_NAME, ENGINE_ID, ENGINE_VERSION, ENROLLMENT_SAMPLES,
    Inspection, MIN_MEAN_MINUTIA_QUALITY, POLICY_NAME, TEMPLATE_SCHEMA_MAJOR, inspect_capture,
    verify,
};
pub use error::{Error, Result};
pub use moc::MatchOnChipDevice;
pub use synthetic::{
    SYNTHETIC_CAPTURE_PROFILE, SYNTHETIC_HEIGHT, SYNTHETIC_PPI, SYNTHETIC_WIDTH, generate_synthetic,
};
pub use template::{load_template, save_template};
pub use worker::WorkerDevice;

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

/// Enroll a finger on a match-on-chip device and write the resulting template.
///
/// `on_progress` receives `(completed_stages, total_stages)`. It is not called for the final
/// presentation on every device — see [`MatchOnChipDevice`] — so drive any UI from the return of
/// this function, not from a progress count reaching the total.
pub fn enroll_device_template(
    device: &mut dyn MatchOnChipDevice,
    out: &Path,
    on_progress: &mut dyn FnMut(u32, u32),
) -> Result<DeviceTemplate> {
    let template = moc::enroll_device(device, on_progress)?;
    save_template(out, &TemplateRecord::MatchOnChip(template.clone()))?;
    Ok(template)
}

/// Verify a live finger on a match-on-chip device against a stored template file.
///
/// Fails closed if the file holds a host-image template: those are matched here, from pixels, and
/// mean nothing to a sensor that expects its own blob back.
pub fn verify_device_template(
    device: &mut dyn MatchOnChipDevice,
    template_path: &Path,
) -> Result<MatchVerdict> {
    let TemplateRecord::MatchOnChip(template) = load_template(template_path)? else {
        return Err(Error::invalid(
            "this template matches on the host; verify it against a capture, not a device",
        ));
    };
    moc::verify_device(device, &template)
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
