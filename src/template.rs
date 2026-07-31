use crate::engine::{
    DEVICE_POLICY_NAME, ENGINE_ID, ENGINE_VERSION, ENROLLMENT_SAMPLES, MIN_MEAN_MINUTIA_QUALITY,
    POLICY_NAME, TEMPLATE_SCHEMA_MAJOR,
};
use crate::io_util::read_bounded_regular_file;
use crate::{
    Error, MAX_DEVICE_TEMPLATE_BYTES, MinutiaRecord, Result, TemplateRecord, TemplateSample,
};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

const MAX_TEMPLATE_BYTES: usize = 1024 * 1024;
const MAX_TEMPLATE_SAMPLES: usize = 8;
const MAX_MINUTIAE_PER_SAMPLE: usize = 150;
const MAX_IDENTIFIER_BYTES: usize = 128;

/// The on-disk template, version 2.
///
/// `kind` is the discriminant, and it is required: version 1 had no such field and only ever meant
/// host-image, so a v1 file fails to parse here rather than being silently reinterpreted. That is
/// the intended path for a schema change in a format that fails closed.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TemplateDtoV2 {
    HostImage {
        schema_major: u32,
        engine_id: String,
        engine_version: String,
        capture_profile_id: String,
        policy: String,
        samples: Vec<TemplateSampleDtoV1>,
    },
    MatchOnChip {
        schema_major: u32,
        driver_id: String,
        device_profile_id: String,
        policy: String,
        /// The device's opaque template, standard-alphabet padded base64.
        blob_base64: String,
    },
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
    let dto: TemplateDtoV2 = serde_json::from_slice(&bytes)?;
    let template = from_dto(dto)?;
    validate_template(&template)?;
    Ok(template)
}

pub(crate) fn validate_template(template: &TemplateRecord) -> Result<()> {
    if template.schema_major() != TEMPLATE_SCHEMA_MAJOR {
        return Err(Error::invalid("unsupported template schema major version"));
    }
    match template {
        TemplateRecord::HostImage(template) => validate_host_image_template(template),
        TemplateRecord::MatchOnChip(template) => validate_device_template(template),
    }
}

/// Validate a match-on-chip template.
///
/// Note what is *absent*: no minutiae count, no BOZORTH3 floor, no mean-quality policy. Those are
/// host-image concepts, and applying them to a blob the host cannot read would be theatre. What
/// can be checked is provenance and bounds — which driver and model produced the blob, and that it
/// is neither empty nor larger than any real device would return.
fn validate_device_template(template: &crate::DeviceTemplate) -> Result<()> {
    if template.policy != DEVICE_POLICY_NAME {
        return Err(Error::invalid(
            "template matching policy does not match this build",
        ));
    }
    validate_identifier(&template.driver_id, "template driver ID")?;
    validate_identifier(&template.device_profile_id, "template device profile ID")?;
    if template.blob.is_empty() {
        return Err(Error::invalid("device template blob is empty"));
    }
    if template.blob.len() > MAX_DEVICE_TEMPLATE_BYTES {
        return Err(Error::invalid("device template blob exceeds the limit"));
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES {
        return Err(Error::invalid(format!("{label} is empty or too long")));
    }
    Ok(())
}

fn validate_host_image_template(template: &crate::HostImageTemplate) -> Result<()> {
    if template.engine_id != ENGINE_ID || template.engine_version != ENGINE_VERSION {
        return Err(Error::invalid("template engine does not match this build"));
    }
    if template.policy != POLICY_NAME {
        return Err(Error::invalid(
            "template matching policy does not match this build",
        ));
    }
    if template.capture_profile_id.is_empty()
        || template.capture_profile_id.len() > MAX_IDENTIFIER_BYTES
    {
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

fn to_dto(template: &TemplateRecord) -> TemplateDtoV2 {
    match template {
        TemplateRecord::HostImage(template) => TemplateDtoV2::HostImage {
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
        },
        TemplateRecord::MatchOnChip(template) => TemplateDtoV2::MatchOnChip {
            schema_major: template.schema_major,
            driver_id: template.driver_id.clone(),
            device_profile_id: template.device_profile_id.clone(),
            policy: template.policy.clone(),
            blob_base64: BASE64.encode(&template.blob),
        },
    }
}

fn from_dto(dto: TemplateDtoV2) -> Result<TemplateRecord> {
    Ok(match dto {
        TemplateDtoV2::HostImage {
            schema_major,
            engine_id,
            engine_version,
            capture_profile_id,
            policy,
            samples,
        } => TemplateRecord::HostImage(crate::HostImageTemplate {
            schema_major,
            engine_id,
            engine_version,
            capture_profile_id,
            policy,
            samples: samples
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
        }),
        TemplateDtoV2::MatchOnChip {
            schema_major,
            driver_id,
            device_profile_id,
            policy,
            blob_base64,
        } => {
            // Decode before the length check so an oversized blob is rejected on its real size,
            // not on the ~4/3 base64 inflation of it.
            let blob = BASE64
                .decode(blob_base64.as_bytes())
                .map_err(|_| Error::invalid("device template blob is not valid base64"))?;
            TemplateRecord::MatchOnChip(crate::DeviceTemplate {
                schema_major,
                driver_id,
                device_profile_id,
                policy,
                blob,
            })
        }
    })
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
        TemplateRecord::HostImage(crate::HostImageTemplate {
            schema_major: TEMPLATE_SCHEMA_MAJOR,
            engine_id: ENGINE_ID.to_owned(),
            engine_version: ENGINE_VERSION.to_owned(),
            capture_profile_id: metadata.unwrap().capture_profile_id,
            policy: POLICY_NAME.to_owned(),
            samples,
        })
    }

    fn valid_device_template() -> TemplateRecord {
        TemplateRecord::MatchOnChip(crate::DeviceTemplate {
            schema_major: TEMPLATE_SCHEMA_MAJOR,
            driver_id: "upekts".to_owned(),
            device_profile_id: "upek-touchstrip".to_owned(),
            policy: DEVICE_POLICY_NAME.to_owned(),
            // 241 bytes is what a real UPEK TouchStrip returned.
            blob: vec![0x5A; 241],
        })
    }

    /// The host-image DTO's mutable samples, for tests that corrupt one field at a time.
    fn samples_of(dto: &mut TemplateDtoV2) -> &mut Vec<TemplateSampleDtoV1> {
        match dto {
            TemplateDtoV2::HostImage { samples, .. } => samples,
            TemplateDtoV2::MatchOnChip { .. } => unreachable!("expected a host-image template"),
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
        match &mut unknown {
            TemplateDtoV2::HostImage { schema_major, .. } => *schema_major += 1,
            TemplateDtoV2::MatchOnChip { .. } => unreachable!(),
        }
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
        {
            let samples = samples_of(&mut too_many_samples);
            while samples.len() <= MAX_TEMPLATE_SAMPLES {
                samples.push(samples[0].clone());
            }
        }
        fs::write(
            &too_many_samples_path,
            serde_json::to_vec(&too_many_samples).unwrap(),
        )
        .unwrap();
        assert!(load_template(&too_many_samples_path).is_err());

        let too_many_minutiae_path = temp.path().join("too-many-minutiae.json");
        let mut too_many_minutiae = to_dto(&valid_template());
        {
            let samples = samples_of(&mut too_many_minutiae);
            samples[0].minutiae = vec![samples[0].minutiae[0].clone(); MAX_MINUTIAE_PER_SAMPLE + 1];
        }
        fs::write(
            &too_many_minutiae_path,
            serde_json::to_vec(&too_many_minutiae).unwrap(),
        )
        .unwrap();
        assert!(load_template(&too_many_minutiae_path).is_err());

        let engine_mismatch_path = temp.path().join("engine-mismatch.json");
        let mut engine_mismatch = to_dto(&valid_template());
        match &mut engine_mismatch {
            TemplateDtoV2::HostImage { engine_id, .. } => {
                *engine_id = "different-engine".to_owned();
            }
            TemplateDtoV2::MatchOnChip { .. } => unreachable!(),
        }
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
        {
            let samples = samples_of(&mut dto);
            samples[0].mean_quality = if samples[0].mean_quality == 100 {
                99
            } else {
                samples[0].mean_quality + 1
            };
        }
        fs::write(&path, serde_json::to_vec(&dto).unwrap()).unwrap();
        assert!(load_template(&path).is_err());
    }

    #[test]
    fn template_rejects_rounded_mean_below_exact_policy() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("rounded-below-policy.json");
        let mut dto = to_dto(&valid_template());
        {
            let samples = samples_of(&mut dto);
            let minutiae = &mut samples[0].minutiae;
            let minutiae_count = minutiae.len();
            for (index, minutia) in minutiae.iter_mut().enumerate() {
                minutia.quality = if index * 2 < minutiae_count { 25 } else { 24 };
            }
            let quality_sum: u64 = minutiae
                .iter()
                .map(|minutia| u64::from(minutia.quality))
                .sum();
            let count = u64::try_from(minutiae.len()).unwrap();
            samples[0].mean_quality = u8::try_from((quality_sum + count / 2) / count).unwrap();
            assert_eq!(samples[0].mean_quality, MIN_MEAN_MINUTIA_QUALITY);
            assert!(quality_sum < u64::from(MIN_MEAN_MINUTIA_QUALITY) * count);
        }
        fs::write(&path, serde_json::to_vec(&dto).unwrap()).unwrap();
        assert!(load_template(&path).is_err());
    }

    #[test]
    fn device_template_round_trips_through_the_file_format() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("device-template.json");
        let template = valid_device_template();
        save_template(&path, &template).unwrap();
        assert_eq!(load_template(&path).unwrap(), template);
    }

    #[test]
    fn a_version_1_template_no_longer_loads() {
        // v1 had no `kind`, and every v1 file meant host-image. Rather than assume that, the
        // format refuses it: the discriminant is required, so an old file fails to parse.
        let temp = tempdir().unwrap();
        let path = temp.path().join("v1.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&serde_json::to_vec(&to_dto(&valid_template())).unwrap())
                .unwrap();
        value.as_object_mut().unwrap().remove("kind");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(load_template(&path).is_err());
    }

    #[test]
    fn the_two_kinds_cannot_be_confused_for_one_another() {
        let temp = tempdir().unwrap();

        // A device template carrying the host-image policy, and vice versa. Each is refused,
        // which is the whole point of keeping the policy names disjoint.
        let wrong_device_policy = temp.path().join("wrong-device-policy.json");
        let TemplateRecord::MatchOnChip(mut device) = valid_device_template() else {
            unreachable!()
        };
        device.policy = POLICY_NAME.to_owned();
        fs::write(
            &wrong_device_policy,
            serde_json::to_vec(&to_dto(&TemplateRecord::MatchOnChip(device))).unwrap(),
        )
        .unwrap();
        assert!(load_template(&wrong_device_policy).is_err());

        let wrong_host_policy = temp.path().join("wrong-host-policy.json");
        let TemplateRecord::HostImage(mut host) = valid_template() else {
            unreachable!()
        };
        host.policy = DEVICE_POLICY_NAME.to_owned();
        fs::write(
            &wrong_host_policy,
            serde_json::to_vec(&to_dto(&TemplateRecord::HostImage(host))).unwrap(),
        )
        .unwrap();
        assert!(load_template(&wrong_host_policy).is_err());
    }

    #[test]
    fn device_template_bounds_fail_closed() {
        let temp = tempdir().unwrap();

        for (name, mutate) in [
            (
                "empty-blob",
                Box::new(|t: &mut crate::DeviceTemplate| t.blob.clear())
                    as Box<dyn Fn(&mut crate::DeviceTemplate)>,
            ),
            (
                "oversized-blob",
                Box::new(|t: &mut crate::DeviceTemplate| {
                    t.blob = vec![0; MAX_DEVICE_TEMPLATE_BYTES + 1];
                }),
            ),
            (
                "empty-driver",
                Box::new(|t: &mut crate::DeviceTemplate| t.driver_id.clear()),
            ),
            (
                "empty-profile",
                Box::new(|t: &mut crate::DeviceTemplate| t.device_profile_id.clear()),
            ),
        ] {
            let TemplateRecord::MatchOnChip(mut template) = valid_device_template() else {
                unreachable!()
            };
            mutate(&mut template);
            let path = temp.path().join(format!("{name}.json"));
            fs::write(
                &path,
                serde_json::to_vec(&to_dto(&TemplateRecord::MatchOnChip(template))).unwrap(),
            )
            .unwrap();
            assert!(load_template(&path).is_err(), "{name} should fail closed");
        }
    }

    #[test]
    fn an_unknown_field_is_rejected_in_either_kind() {
        let temp = tempdir().unwrap();
        for (name, template) in [
            ("host", valid_template()),
            ("device", valid_device_template()),
        ] {
            let mut value: serde_json::Value =
                serde_json::from_slice(&serde_json::to_vec(&to_dto(&template)).unwrap()).unwrap();
            value
                .as_object_mut()
                .unwrap()
                .insert("surprise".to_owned(), serde_json::Value::Bool(true));
            let path = temp.path().join(format!("{name}-unknown-field.json"));
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(
                load_template(&path).is_err(),
                "{name} must deny unknown fields"
            );
        }
    }

    #[test]
    fn profile_mismatch_fails_closed_at_verification() {
        let template = valid_template();
        let (image, mut metadata) = generate_synthetic(41, 9).unwrap();
        metadata.capture_profile_id = "different-profile".to_owned();
        assert!(crate::verify(&template, &image, &metadata).is_err());
    }
}
