//! The match-on-chip seam.
//!
//! [`CaptureSource`](crate::source) covers sensors that stream pixels: the host receives an image,
//! extracts minutiae, and decides. This module covers the other archetype, where the sensor holds
//! the matcher. The host never sees a finger — it receives an opaque template it cannot read, and
//! hands that template back at verification time for the device to compare against a live scan.
//!
//! The trust boundary is inverted, so the shape of the API is too. There is no image, no minutiae,
//! no score, and no threshold to compare a score against; there is an enrollment that yields a
//! blob and a verification that yields a verdict.

use crate::{DeviceTemplate, Error, MatchVerdict, Result};

/// A sensor that enrolls and matches internally.
///
/// The enrollment callback reports progress as `(completed_stages, total_stages)`.
///
/// **A driver is not obliged to report the final stage.** libfprint's `upekts` reports a stage only
/// once the *following* poll asks for another presentation, so the poll after the last swipe is
/// "enrollment complete", which reports nothing and hands over the template instead: a 3-stage
/// enrollment emits two progress callbacks. Treat the returned template as the completion signal.
/// Waiting for `completed == total` hangs.
pub trait MatchOnChipDevice {
    /// The driver that backs this device. Recorded in every template it produces.
    fn driver_id(&self) -> &str;

    /// The device model. Recorded in every template it produces.
    fn device_profile_id(&self) -> &str;

    /// How many presentations a full enrollment needs, as the device reports it.
    fn enroll_stages(&self) -> u32;

    /// Enroll a finger and return the device's own template.
    fn enroll(&mut self, on_progress: &mut dyn FnMut(u32, u32)) -> Result<Vec<u8>>;

    /// Ask the device to compare a live finger against a template it previously produced.
    fn verify(&mut self, blob: &[u8]) -> Result<MatchVerdict>;
}

/// Enroll through a device and build a validated, storable template.
pub(crate) fn enroll_device(
    device: &mut dyn MatchOnChipDevice,
    on_progress: &mut dyn FnMut(u32, u32),
) -> Result<DeviceTemplate> {
    let driver_id = device.driver_id().to_owned();
    let device_profile_id = device.device_profile_id().to_owned();
    let blob = device.enroll(on_progress)?;
    let template = DeviceTemplate {
        schema_major: crate::engine::TEMPLATE_SCHEMA_MAJOR,
        driver_id,
        device_profile_id,
        policy: crate::engine::DEVICE_POLICY_NAME.to_owned(),
        blob,
    };
    crate::template::validate_template(&crate::TemplateRecord::MatchOnChip(template.clone()))?;
    Ok(template)
}

/// Verify a live finger against a stored device template.
///
/// The template's provenance is checked first. A blob is a reference into one particular sensor's
/// world; handing it to a different driver or model is not a comparison that can fail honestly, so
/// it is refused rather than attempted.
pub(crate) fn verify_device(
    device: &mut dyn MatchOnChipDevice,
    template: &DeviceTemplate,
) -> Result<MatchVerdict> {
    crate::template::validate_template(&crate::TemplateRecord::MatchOnChip(template.clone()))?;
    if template.driver_id != device.driver_id() {
        return Err(Error::invalid(
            "template was enrolled by a different driver",
        ));
    }
    if template.device_profile_id != device.device_profile_id() {
        return Err(Error::invalid(
            "template was enrolled on a different device model",
        ));
    }
    device.verify(&template.blob)
}

#[cfg(test)]
pub(crate) mod mock {
    use super::{MatchOnChipDevice, MatchVerdict};
    use crate::{Error, Result};

    /// A scripted stand-in for a match-on-chip sensor.
    ///
    /// It models the one behaviour that matters and that a real device taught us: enrollment
    /// reports `stages - 1` progress callbacks and signals completion by returning the template.
    pub(crate) struct MockDevice {
        driver_id: String,
        device_profile_id: String,
        stages: u32,
        /// The identity the next `enroll` mints a template for.
        pub(crate) enrolled_identity: u8,
        /// The identity the next `verify` presents.
        pub(crate) presented_identity: u8,
        /// Force the device to fail its next operation.
        pub(crate) fail_next: bool,
    }

    impl MockDevice {
        pub(crate) fn new(stages: u32) -> Self {
            Self {
                driver_id: "mock".to_owned(),
                device_profile_id: "mock-moc-sensor".to_owned(),
                stages,
                enrolled_identity: 1,
                presented_identity: 1,
                fail_next: false,
            }
        }

        pub(crate) fn with_driver(mut self, driver_id: &str) -> Self {
            self.driver_id = driver_id.to_owned();
            self
        }
    }

    impl MatchOnChipDevice for MockDevice {
        fn driver_id(&self) -> &str {
            &self.driver_id
        }

        fn device_profile_id(&self) -> &str {
            &self.device_profile_id
        }

        fn enroll_stages(&self) -> u32 {
            self.stages
        }

        fn enroll(&mut self, on_progress: &mut dyn FnMut(u32, u32)) -> Result<Vec<u8>> {
            if self.fail_next {
                self.fail_next = false;
                return Err(Error::processing("mock device refused to enroll"));
            }
            // `stages - 1`, not `stages`: the last presentation is reported by the template
            // arriving, exactly as upekts behaves.
            for completed in 1..self.stages {
                on_progress(completed, self.stages);
            }
            Ok(vec![self.enrolled_identity; 32])
        }

        fn verify(&mut self, blob: &[u8]) -> Result<MatchVerdict> {
            if self.fail_next {
                self.fail_next = false;
                return Err(Error::processing("mock device refused to verify"));
            }
            Ok(MatchVerdict {
                matched: blob.first() == Some(&self.presented_identity),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockDevice;
    use super::*;

    #[test]
    fn enroll_then_verify_matches_the_same_finger_and_rejects_another() {
        let mut device = MockDevice::new(3);
        let mut stages_seen = Vec::new();
        let template = enroll_device(&mut device, &mut |done, total| {
            stages_seen.push((done, total))
        })
        .unwrap();

        // The contract a real sensor taught us: progress stops one short, and the template is the
        // completion signal. A test asserting `stages_seen.len() == 3` would encode a fiction.
        assert_eq!(stages_seen, vec![(1, 3), (2, 3)]);
        assert_eq!(template.policy, crate::engine::DEVICE_POLICY_NAME);
        assert_eq!(template.driver_id, "mock");

        assert!(verify_device(&mut device, &template).unwrap().matched);

        device.presented_identity = 2;
        assert!(!verify_device(&mut device, &template).unwrap().matched);
    }

    #[test]
    fn a_template_is_refused_by_a_device_that_did_not_produce_it() {
        let mut device = MockDevice::new(3);
        let template = enroll_device(&mut device, &mut |_, _| {}).unwrap();

        let mut other = MockDevice::new(3).with_driver("other-driver");
        let error = verify_device(&mut other, &template).unwrap_err();
        assert!(format!("{error}").contains("different driver"));
    }

    #[test]
    fn device_failures_surface_instead_of_becoming_a_non_match() {
        let mut device = MockDevice::new(3);
        device.fail_next = true;
        assert!(enroll_device(&mut device, &mut |_, _| {}).is_err());

        let template = enroll_device(&mut device, &mut |_, _| {}).unwrap();
        device.fail_next = true;
        // A broken device must not be indistinguishable from a stranger's finger.
        assert!(verify_device(&mut device, &template).is_err());
    }

    #[test]
    fn an_empty_blob_is_rejected_at_enrollment() {
        struct EmptyDevice;
        impl MatchOnChipDevice for EmptyDevice {
            fn driver_id(&self) -> &str {
                "empty"
            }
            fn device_profile_id(&self) -> &str {
                "empty"
            }
            fn enroll_stages(&self) -> u32 {
                1
            }
            fn enroll(&mut self, _: &mut dyn FnMut(u32, u32)) -> Result<Vec<u8>> {
                Ok(Vec::new())
            }
            fn verify(&mut self, _: &[u8]) -> Result<MatchVerdict> {
                Ok(MatchVerdict { matched: true })
            }
        }
        assert!(enroll_device(&mut EmptyDevice, &mut |_, _| {}).is_err());
    }
}
