use crate::engine::{
    ENGINE_ID, ENGINE_VERSION, ENROLLMENT_SAMPLES, MIN_MEAN_MINUTIA_QUALITY, POLICY_NAME,
};
use crate::io_util::read_bounded_regular_file;
use crate::{Error, MinutiaRecord, Result, TemplateRecord, TemplateSample};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

const TEMPLATE_SCHEMA_MAJOR: u32 = 1;
const MAX_TEMPLATE_BYTES: usize = 1024 * 1024;
const MAX_TEMPLATE_SAMPLES: usize = 8;
const MAX_MINUTIAE_PER_SAMPLE: usize = 150;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateDtoV1 {
    schema_major: u32,
    engine_id: String,
    engine_version: String,
    capture_profile_id: String,
    policy: String,
    samples: Vec<TemplateSampleDtoV1>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateSampleDtoV1 {
    mean_quality: u8,
    minutiae: Vec<MinutiaDtoV1>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MinutiaDtoV1 {
    x: i32,
    y: i32,
    theta: i32,
    quality: u8,
}

/// Save a validated explicit version-1 JSON template without overwriting a file.
pub fn save_template(path: &Path, template: &TemplateRecord) -> Result<()> {
    validate_template(template)?;
    let dto = to_dto(template);
    let bytes = serde_json::to_vec_pretty(&dto)?;
    if bytes.len() > MAX_TEMPLATE_BYTES {
        return Err(Error::invalid("template exceeds the 1 MiB limit"));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Error::invalid("template output already exists; refusing to overwrite")
            } else {
                Error::io(error)
            }
        })?;
    file.write_all(&bytes).map_err(Error::io)?;
    file.sync_all().map_err(Error::io)
}

/// Load a size-bounded explicit version-1 JSON template and validate engine and policy fields.
pub fn load_template(path: &Path) -> Result<TemplateRecord> {
    let bytes = read_bounded_regular_file(path, MAX_TEMPLATE_BYTES, "template")?;
    let dto: TemplateDtoV1 = serde_json::from_slice(&bytes)?;
    let template = from_dto(dto);
    validate_template(&template)?;
    Ok(template)
}

pub(crate) fn validate_template(template: &TemplateRecord) -> Result<()> {
    if template.schema_major != TEMPLATE_SCHEMA_MAJOR {
        return Err(Error::invalid("unsupported template schema major version"));
    }
    if template.engine_id != ENGINE_ID || template.engine_version != ENGINE_VERSION {
        return Err(Error::invalid("template engine does not match this build"));
    }
    if template.policy != POLICY_NAME {
        return Err(Error::invalid(
            "template matching policy does not match this build",
        ));
    }
    if template.capture_profile_id.is_empty() || template.capture_profile_id.len() > 128 {
        return Err(Error::invalid("template capture profile ID is invalid"));
    }
    if template.samples.len() > MAX_TEMPLATE_SAMPLES {
        return Err(Error::invalid("template has too many samples"));
    }
    if template.samples.len() != ENROLLMENT_SAMPLES {
        return Err(Error::invalid(format!(
            "M0 templates must contain exactly {ENROLLMENT_SAMPLES} samples"
        )));
    }
    for sample in &template.samples {
        if sample.minutiae.len() > MAX_MINUTIAE_PER_SAMPLE {
            return Err(Error::invalid("template sample has too many minutiae"));
        }
        if sample.minutiae.len() < fprint_bozorth3::MIN_COMPUTABLE_BOZORTH_MINUTIAE {
            return Err(Error::invalid("template sample has too few minutiae"));
        }
        if sample.mean_quality < MIN_MEAN_MINUTIA_QUALITY || sample.mean_quality > 100 {
            return Err(Error::invalid("template sample quality is outside policy"));
        }
        for minutia in &sample.minutiae {
            if !(0..360).contains(&minutia.theta)
                || minutia.x < 0
                || minutia.y < 0
                || minutia.quality > 100
            {
                return Err(Error::invalid("template contains an invalid minutia"));
            }
        }
        let quality_sum: u64 = sample
            .minutiae
            .iter()
            .map(|minutia| u64::from(minutia.quality))
            .sum();
        let count = u64::try_from(sample.minutiae.len())
            .map_err(|_| Error::invalid("template sample size is unsupported"))?;
        if quality_sum < u64::from(MIN_MEAN_MINUTIA_QUALITY) * count {
            return Err(Error::invalid(
                "template sample mean quality is below policy",
            ));
        }
        let computed_mean = u8::try_from((quality_sum + count / 2) / count)
            .map_err(|_| Error::invalid("template sample quality is invalid"))?;
        if sample.mean_quality != computed_mean {
            return Err(Error::invalid(
                "template sample mean quality is inconsistent",
            ));
        }
    }
    Ok(())
}

fn to_dto(template: &TemplateRecord) -> TemplateDtoV1 {
    TemplateDtoV1 {
        schema_major: template.schema_major,
        engine_id: template.engine_id.clone(),
        engine_version: template.engine_version.clone(),
        capture_profile_id: template.capture_profile_id.clone(),
        policy: template.policy.clone(),
        samples: template
            .samples
            .iter()
            .map(|sample| TemplateSampleDtoV1 {
                mean_quality: sample.mean_quality,
                minutiae: sample
                    .minutiae
                    .iter()
                    .map(|minutia| MinutiaDtoV1 {
                        x: minutia.x,
                        y: minutia.y,
                        theta: minutia.theta,
                        quality: minutia.quality,
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn from_dto(dto: TemplateDtoV1) -> TemplateRecord {
    TemplateRecord {
        schema_major: dto.schema_major,
        engine_id: dto.engine_id,
        engine_version: dto.engine_version,
        capture_profile_id: dto.capture_profile_id,
        policy: dto.policy,
        samples: dto
            .samples
            .into_iter()
            .map(|sample| TemplateSample {
                mean_quality: sample.mean_quality,
                minutiae: sample
                    .minutiae
                    .into_iter()
                    .map(|minutia| MinutiaRecord {
                        x: minutia.x,
                        y: minutia.y,
                        theta: minutia.theta,
                        quality: minutia.quality,
                    })
                    .collect(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CaptureMetadata, generate_synthetic};
    use std::fs;
    use tempfile::tempdir;

    fn valid_template() -> TemplateRecord {
        let mut samples = Vec::new();
        let mut metadata: Option<CaptureMetadata> = None;
        for impression in 0..ENROLLMENT_SAMPLES {
            let (image, current_metadata) = generate_synthetic(41, impression as u64).unwrap();
            metadata = Some(current_metadata.clone());
            samples.push(crate::engine::analyze_for_test(&image, &current_metadata).unwrap());
        }
        TemplateRecord {
            schema_major: 1,
            engine_id: ENGINE_ID.to_owned(),
            engine_version: ENGINE_VERSION.to_owned(),
            capture_profile_id: metadata.unwrap().capture_profile_id,
            policy: POLICY_NAME.to_owned(),
            samples,
        }
    }

    #[test]
    fn template_round_trip_and_overwrite_refusal() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("template.json");
        let template = valid_template();
        save_template(&path, &template).unwrap();
        assert!(load_template(&path).unwrap() == template);
        assert!(save_template(&path, &template).is_err());
    }

    #[test]
    fn rejects_unknown_truncated_oversized_and_mismatched_templates() {
        let temp = tempdir().unwrap();
        let template = valid_template();

        let unknown_path = temp.path().join("unknown.json");
        let mut unknown = to_dto(&template);
        unknown.schema_major = 2;
        fs::write(&unknown_path, serde_json::to_vec(&unknown).unwrap()).unwrap();
        assert!(load_template(&unknown_path).is_err());

        let truncated_path = temp.path().join("truncated.json");
        fs::write(&truncated_path, b"{\"schema_major\":1").unwrap();
        assert!(load_template(&truncated_path).is_err());

        let oversized_path = temp.path().join("oversized.json");
        fs::write(&oversized_path, vec![b' '; MAX_TEMPLATE_BYTES + 1]).unwrap();
        assert!(load_template(&oversized_path).is_err());

        let too_many_samples_path = temp.path().join("too-many-samples.json");
        let mut too_many_samples = to_dto(&valid_template());
        while too_many_samples.samples.len() <= MAX_TEMPLATE_SAMPLES {
            too_many_samples
                .samples
                .push(too_many_samples.samples[0].clone());
        }
        fs::write(
            &too_many_samples_path,
            serde_json::to_vec(&too_many_samples).unwrap(),
        )
        .unwrap();
        assert!(load_template(&too_many_samples_path).is_err());

        let too_many_minutiae_path = temp.path().join("too-many-minutiae.json");
        let mut too_many_minutiae = to_dto(&valid_template());
        too_many_minutiae.samples[0].minutiae =
            vec![too_many_minutiae.samples[0].minutiae[0].clone(); MAX_MINUTIAE_PER_SAMPLE + 1];
        fs::write(
            &too_many_minutiae_path,
            serde_json::to_vec(&too_many_minutiae).unwrap(),
        )
        .unwrap();
        assert!(load_template(&too_many_minutiae_path).is_err());

        let engine_mismatch_path = temp.path().join("engine-mismatch.json");
        let mut engine_mismatch = to_dto(&valid_template());
        engine_mismatch.engine_id = "different-engine".to_owned();
        fs::write(
            &engine_mismatch_path,
            serde_json::to_vec(&engine_mismatch).unwrap(),
        )
        .unwrap();
        assert!(load_template(&engine_mismatch_path).is_err());
    }

    #[test]
    fn template_rejects_inconsistent_declared_mean_quality() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("inconsistent-quality.json");
        let mut dto = to_dto(&valid_template());
        dto.samples[0].mean_quality = if dto.samples[0].mean_quality == 100 {
            99
        } else {
            dto.samples[0].mean_quality + 1
        };
        fs::write(&path, serde_json::to_vec(&dto).unwrap()).unwrap();
        assert!(load_template(&path).is_err());
    }

    #[test]
    fn template_rejects_rounded_mean_below_exact_policy() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("rounded-below-policy.json");
        let mut dto = to_dto(&valid_template());
        let minutiae = &mut dto.samples[0].minutiae;
        let minutiae_count = minutiae.len();
        for (index, minutia) in minutiae.iter_mut().enumerate() {
            minutia.quality = if index * 2 < minutiae_count { 25 } else { 24 };
        }
        let quality_sum: u64 = minutiae
            .iter()
            .map(|minutia| u64::from(minutia.quality))
            .sum();
        let count = u64::try_from(minutiae.len()).unwrap();
        dto.samples[0].mean_quality = u8::try_from((quality_sum + count / 2) / count).unwrap();
        assert_eq!(dto.samples[0].mean_quality, MIN_MEAN_MINUTIA_QUALITY);
        assert!(quality_sum < u64::from(MIN_MEAN_MINUTIA_QUALITY) * count);
        fs::write(&path, serde_json::to_vec(&dto).unwrap()).unwrap();
        assert!(load_template(&path).is_err());
    }

    #[test]
    fn profile_mismatch_fails_closed_at_verification() {
        let template = valid_template();
        let (image, mut metadata) = generate_synthetic(41, 9).unwrap();
        metadata.capture_profile_id = "different-profile".to_owned();
        assert!(crate::verify(&template, &image, &metadata).is_err());
    }
}
