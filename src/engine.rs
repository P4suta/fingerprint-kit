use crate::{
    CanonicalImage, CaptureMetadata, Error, MinutiaRecord, Result, TemplateRecord, TemplateSample,
    VerificationResult,
};
use std::path::PathBuf;

/// Fixed M0 enrollment sample count.
pub const ENROLLMENT_SAMPLES: usize = 3;
/// Minimum accepted mean MINDTCT minutia quality on the 0–100 scale.
pub const MIN_MEAN_MINUTIA_QUALITY: u8 = 25;
/// Fixed experimental BOZORTH3 acceptance threshold.
pub const ACCEPT_THRESHOLD: u32 = 40;
/// Fixed, explicitly experimental matching policy name.
pub const POLICY_NAME: &str = "experimental-nbis-40-v1";
/// Engine identifier persisted in version-1 templates.
pub const ENGINE_ID: &str = "nbis-mindtct-bozorth3";
/// Engine version persisted in version-1 templates.
pub const ENGINE_VERSION: &str = "fprint-mindtct-0.1.0+fprint-bozorth3-0.1.0";

/// Non-sensitive inspection summary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inspection {
    /// Validated image width.
    pub width: u32,
    /// Validated image height.
    pub height: u32,
    /// Validated horizontal resolution.
    pub x_resolution_ppi: u16,
    /// Validated vertical resolution.
    pub y_resolution_ppi: u16,
    /// Darkest-to-lightest pixel range.
    pub dynamic_range: u8,
    /// Number of MINDTCT minutiae.
    pub minutiae_count: usize,
    /// Mean MINDTCT quality on the 0–100 scale.
    pub mean_quality: f64,
}

struct Analysis {
    inspection: Inspection,
    sample: TemplateSample,
}

/// Validate and inspect one canonical capture.
pub fn inspect_capture(image: &CanonicalImage, metadata: &CaptureMetadata) -> Result<Inspection> {
    Ok(detect_capture(image, metadata)?.inspection)
}

pub(crate) fn enroll_paths(captures: &[PathBuf]) -> Result<TemplateRecord> {
    if captures.len() != ENROLLMENT_SAMPLES {
        return Err(Error::invalid(format!(
            "enrollment requires exactly {ENROLLMENT_SAMPLES} captures"
        )));
    }

    let mut profile: Option<String> = None;
    let mut samples = Vec::with_capacity(ENROLLMENT_SAMPLES);
    for capture in captures {
        let (image, metadata) = crate::source::capture_replay(capture)?;
        if let Some(expected) = &profile {
            if expected != &metadata.capture_profile_id {
                return Err(Error::invalid(
                    "all enrollment captures must use the same capture profile",
                ));
            }
        } else {
            profile = Some(metadata.capture_profile_id.clone());
        }
        samples.push(analyze(&image, &metadata)?.sample);
    }

    Ok(TemplateRecord {
        schema_major: 1,
        engine_id: ENGINE_ID.to_owned(),
        engine_version: ENGINE_VERSION.to_owned(),
        capture_profile_id: profile
            .ok_or_else(|| Error::processing("enrollment profile is unavailable"))?,
        policy: POLICY_NAME.to_owned(),
        samples,
    })
}

/// Verify one canonical probe against all enrollment samples, taking the maximum score.
pub fn verify(
    template: &TemplateRecord,
    image: &CanonicalImage,
    metadata: &CaptureMetadata,
) -> Result<VerificationResult> {
    crate::template::validate_template(template)?;
    if template.capture_profile_id != metadata.capture_profile_id {
        return Err(Error::invalid(
            "probe capture profile does not match the template",
        ));
    }
    let probe = analyze(image, metadata)?.sample;
    let probe_minutiae = to_bozorth(&probe.minutiae);
    let score = template
        .samples
        .iter()
        .map(|sample| fprint_bozorth3::match_score(&probe_minutiae, &to_bozorth(&sample.minutiae)))
        .max()
        .unwrap_or(0);

    Ok(VerificationResult {
        score,
        threshold: ACCEPT_THRESHOLD,
        matched: score >= ACCEPT_THRESHOLD,
        probe_quality: probe.mean_quality,
    })
}

fn analyze(image: &CanonicalImage, metadata: &CaptureMetadata) -> Result<Analysis> {
    let analysis = detect_capture(image, metadata)?;
    let count = analysis.inspection.minutiae_count;
    if count < fprint_bozorth3::MIN_COMPUTABLE_BOZORTH_MINUTIAE {
        return Err(Error::processing(format!(
            "capture has {count} minutiae; at least {} are required",
            fprint_bozorth3::MIN_COMPUTABLE_BOZORTH_MINUTIAE
        )));
    }
    if analysis.inspection.mean_quality < f64::from(MIN_MEAN_MINUTIA_QUALITY) {
        return Err(Error::processing(format!(
            "capture mean minutia quality is below {MIN_MEAN_MINUTIA_QUALITY}"
        )));
    }
    Ok(analysis)
}

fn detect_capture(image: &CanonicalImage, metadata: &CaptureMetadata) -> Result<Analysis> {
    metadata.validate()?;
    if metadata.partial {
        return Err(Error::invalid(
            "partial captures are not accepted by the M0 policy",
        ));
    }
    if image.x_resolution_ppi() != metadata.x_resolution_ppi
        || image.y_resolution_ppi() != metadata.y_resolution_ppi
    {
        return Err(Error::invalid(
            "image and capture metadata resolutions differ",
        ));
    }
    if image.x_resolution_ppi() != image.y_resolution_ppi() {
        return Err(Error::invalid(
            "M0 NBIS processing requires equal X and Y resolution",
        ));
    }

    let width =
        usize::try_from(image.width()).map_err(|_| Error::invalid("image width is unsupported"))?;
    let height = usize::try_from(image.height())
        .map_err(|_| Error::invalid("image height is unsupported"))?;
    let gray =
        fprint_mindtct::GrayImage::new(image.pixels(), width, height, image.x_resolution_ppi())
            .map_err(|error| {
                Error::processing(format!("MINDTCT rejected image geometry: {error}"))
            })?;
    let detected = fprint_mindtct::detect_minutiae(gray);
    let count = detected.len();
    let quality_sum: u64 = detected
        .iter()
        .map(|minutia| u64::try_from(minutia.quality).unwrap_or(0))
        .sum();
    let mean_quality = if count == 0 {
        0.0
    } else {
        quality_sum as f64 / count as f64
    };
    let rounded_quality = if count == 0 {
        0
    } else {
        u8::try_from((quality_sum + count as u64 / 2) / count as u64).unwrap_or(100)
    };
    let (minimum, maximum) = image
        .pixels()
        .iter()
        .fold((u8::MAX, u8::MIN), |(minimum, maximum), &pixel| {
            (minimum.min(pixel), maximum.max(pixel))
        });
    let inspection = Inspection {
        width: image.width(),
        height: image.height(),
        x_resolution_ppi: image.x_resolution_ppi(),
        y_resolution_ppi: image.y_resolution_ppi(),
        dynamic_range: maximum.saturating_sub(minimum),
        minutiae_count: count,
        mean_quality,
    };

    let minutiae = detected
        .into_iter()
        .map(|minutia| {
            Ok(MinutiaRecord {
                x: minutia.x,
                y: minutia.y,
                theta: minutia.theta,
                quality: u8::try_from(minutia.quality)
                    .map_err(|_| Error::processing("MINDTCT emitted invalid quality"))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Analysis {
        inspection,
        sample: TemplateSample {
            mean_quality: rounded_quality,
            minutiae,
        },
    })
}

#[cfg(test)]
pub(crate) fn analyze_for_test(
    image: &CanonicalImage,
    metadata: &CaptureMetadata,
) -> Result<TemplateSample> {
    Ok(analyze(image, metadata)?.sample)
}

fn to_bozorth(minutiae: &[MinutiaRecord]) -> Vec<fprint_bozorth3::Minutia> {
    minutiae
        .iter()
        .map(|minutia| fprint_bozorth3::Minutia {
            x: minutia.x,
            y: minutia.y,
            theta: minutia.theta,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CaptureKind, generate_synthetic};

    #[test]
    fn uniform_and_low_information_images_are_rejected() {
        let metadata = CaptureMetadata {
            capture_profile_id: "test".to_owned(),
            capture_kind: CaptureKind::Press,
            partial: false,
            x_resolution_ppi: 500,
            y_resolution_ppi: 500,
        };
        let uniform = CanonicalImage::new(200, 240, 500, 500, vec![128; 200 * 240]).unwrap();
        let uniform_inspection = inspect_capture(&uniform, &metadata).unwrap();
        assert_eq!(uniform_inspection.minutiae_count, 0);
        assert!(analyze(&uniform, &metadata).is_err());

        let low_range: Vec<u8> = (0..200 * 240)
            .map(|index| 126 + u8::try_from(index % 5).unwrap())
            .collect();
        let low_range = CanonicalImage::new(200, 240, 500, 500, low_range).unwrap();
        assert!(analyze(&low_range, &metadata).is_err());
    }

    #[test]
    fn enrollment_entry_point_rejects_uniform_captures() {
        let temp = tempfile::tempdir().unwrap();
        let metadata = CaptureMetadata {
            capture_profile_id: "test".to_owned(),
            capture_kind: CaptureKind::Press,
            partial: false,
            x_resolution_ppi: 500,
            y_resolution_ppi: 500,
        };
        let uniform = CanonicalImage::new(200, 240, 500, 500, vec![128; 200 * 240]).unwrap();
        let paths: Vec<PathBuf> = (0..ENROLLMENT_SAMPLES)
            .map(|index| temp.path().join(format!("uniform-{index}")))
            .collect();
        for path in &paths {
            crate::save_capture_bundle(path, &uniform, &metadata).unwrap();
        }
        assert!(enroll_paths(&paths).is_err());
    }

    #[test]
    fn fixed_fixture_genuine_matches_and_impostor_does_not() {
        let (_, metadata) = generate_synthetic(41, 0).unwrap();
        let mut template = TemplateRecord {
            schema_major: 1,
            engine_id: ENGINE_ID.to_owned(),
            engine_version: ENGINE_VERSION.to_owned(),
            capture_profile_id: metadata.capture_profile_id.clone(),
            policy: POLICY_NAME.to_owned(),
            samples: Vec::new(),
        };
        for impression in 0..ENROLLMENT_SAMPLES {
            let (image, sample_metadata) = generate_synthetic(41, impression as u64).unwrap();
            template
                .samples
                .push(analyze(&image, &sample_metadata).unwrap().sample);
        }
        let (genuine_image, genuine_metadata) = generate_synthetic(41, 90).unwrap();
        let genuine = verify(&template, &genuine_image, &genuine_metadata).unwrap();
        let (impostor_image, impostor_metadata) = generate_synthetic(99, 0).unwrap();
        let impostor = verify(&template, &impostor_image, &impostor_metadata).unwrap();
        assert!(genuine.score >= ACCEPT_THRESHOLD, "score={}", genuine.score);
        assert!(
            impostor.score < ACCEPT_THRESHOLD,
            "score={}",
            impostor.score
        );
    }
}
