use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Maximum packed Gray8 image size accepted by the host pipeline.
pub const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;

/// A validated packed Gray8 image.
///
/// Pixels are row-major with a top-left origin, X rightward, Y downward, and dark ridges.
#[derive(Clone, PartialEq, Eq)]
pub struct CanonicalImage {
    width: u32,
    height: u32,
    x_resolution_ppi: u16,
    y_resolution_ppi: u16,
    pixels: Vec<u8>,
}

impl CanonicalImage {
    /// Validate dimensions, resolution, area, and exact packed pixel length.
    pub fn new(
        width: u32,
        height: u32,
        x_resolution_ppi: u16,
        y_resolution_ppi: u16,
        pixels: Vec<u8>,
    ) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::invalid("image dimensions must be non-zero"));
        }
        if x_resolution_ppi == 0 || y_resolution_ppi == 0 {
            return Err(Error::invalid(
                "image resolution must be known and non-zero",
            ));
        }
        let width = usize::try_from(width)
            .map_err(|_| Error::invalid("image width exceeds this platform"))?;
        let height = usize::try_from(height)
            .map_err(|_| Error::invalid("image height exceeds this platform"))?;
        let area = checked_pixel_count(width, height)?;
        if area > MAX_IMAGE_BYTES {
            return Err(Error::invalid("image exceeds the 16 MiB limit"));
        }
        if pixels.len() != area {
            return Err(Error::invalid(
                "pixel length does not match packed Gray8 dimensions",
            ));
        }
        Ok(Self {
            width: u32::try_from(width)
                .map_err(|_| Error::invalid("image width exceeds the domain"))?,
            height: u32::try_from(height)
                .map_err(|_| Error::invalid("image height exceeds the domain"))?,
            x_resolution_ppi,
            y_resolution_ppi,
            pixels,
        })
    }

    /// Image width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Image height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Horizontal scan resolution in pixels per inch.
    #[must_use]
    pub const fn x_resolution_ppi(&self) -> u16 {
        self.x_resolution_ppi
    }

    /// Vertical scan resolution in pixels per inch.
    #[must_use]
    pub const fn y_resolution_ppi(&self) -> u16 {
        self.y_resolution_ppi
    }

    /// Borrow the packed pixels. Callers must treat them as sensitive biometric data.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

fn checked_pixel_count(width: usize, height: usize) -> Result<usize> {
    width
        .checked_mul(height)
        .ok_or_else(|| Error::invalid("image dimensions overflow"))
}

impl fmt::Debug for CanonicalImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("x_resolution_ppi", &self.x_resolution_ppi)
            .field("y_resolution_ppi", &self.y_resolution_ppi)
            .field("pixels", &"<redacted>")
            .finish()
    }
}

/// Capture interaction shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    /// A stationary finger press.
    Press,
    /// A swipe capture.
    Swipe,
    /// The capture shape is not known.
    Unknown,
}

/// Non-identifying metadata required to interpret a capture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureMetadata {
    /// Sensor/capture geometry profile identifier.
    pub capture_profile_id: String,
    /// Press, swipe, or unknown capture shape.
    pub capture_kind: CaptureKind,
    /// Whether the source reports an incomplete capture.
    pub partial: bool,
    /// Horizontal scan resolution in pixels per inch.
    pub x_resolution_ppi: u16,
    /// Vertical scan resolution in pixels per inch.
    pub y_resolution_ppi: u16,
}

impl CaptureMetadata {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.capture_profile_id.is_empty() || self.capture_profile_id.len() > 128 {
            return Err(Error::invalid("capture profile ID is empty or too long"));
        }
        if self.x_resolution_ppi == 0 || self.y_resolution_ppi == 0 {
            return Err(Error::invalid(
                "capture resolution must be known and non-zero",
            ));
        }
        Ok(())
    }
}

/// One quality-bearing minutia stored in a versioned template.
#[derive(Clone, PartialEq, Eq)]
pub struct MinutiaRecord {
    /// NIST X coordinate, bottom-left origin.
    pub x: i32,
    /// NIST Y coordinate, bottom-left origin.
    pub y: i32,
    /// NIST ridge orientation in degrees.
    pub theta: i32,
    /// MINDTCT reliability, 0 through 100.
    pub quality: u8,
}

impl fmt::Debug for MinutiaRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MinutiaRecord(<redacted>)")
    }
}

/// One enrollment capture's quality-bearing minutiae.
#[derive(Clone, PartialEq, Eq)]
pub struct TemplateSample {
    /// Mean minutia quality for policy validation.
    pub mean_quality: u8,
    /// Detected minutiae. This payload is sensitive biometric data.
    pub minutiae: Vec<MinutiaRecord>,
}

impl fmt::Debug for TemplateSample {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TemplateSample")
            .field("mean_quality", &self.mean_quality)
            .field(
                "minutiae",
                &format_args!("<{} redacted>", self.minutiae.len()),
            )
            .finish()
    }
}

/// A host-image template: the host holds the pixels, extracts minutiae, and does the matching.
#[derive(Clone, PartialEq, Eq)]
pub struct HostImageTemplate {
    /// Template schema major version.
    pub schema_major: u32,
    /// Extraction/matching engine identifier.
    pub engine_id: String,
    /// Extraction/matching engine version.
    pub engine_version: String,
    /// Capture profile shared by all enrollment samples.
    pub capture_profile_id: String,
    /// Experimental fixed matching policy name.
    pub policy: String,
    /// Quality-bearing enrollment samples.
    pub samples: Vec<TemplateSample>,
}

impl fmt::Debug for HostImageTemplate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostImageTemplate")
            .field("schema_major", &self.schema_major)
            .field("engine_id", &self.engine_id)
            .field("engine_version", &self.engine_version)
            .field("capture_profile_id", &self.capture_profile_id)
            .field("policy", &self.policy)
            .field(
                "samples",
                &format_args!("<{} redacted>", self.samples.len()),
            )
            .finish()
    }
}

/// Upper bound on one device-held template blob, before base64 expansion.
///
/// A match-on-chip blob is small by nature: the sensor keeps the biometric and hands the host a
/// reference it cannot interpret. A UPEK TouchStrip returns 241 bytes. The bound is set far above
/// that so other match-on-chip families fit, and it still leaves the base64 form well inside the
/// template file's own 1 MiB limit.
pub const MAX_DEVICE_TEMPLATE_BYTES: usize = 64 * 1024;

/// A match-on-chip template: an opaque blob the device produced and only the device can read.
///
/// This is the inverse of [`HostImageTemplate`]. No minutiae, no quality, and no score — the host
/// stores the blob and hands it back for the sensor to compare against a live finger, so there is
/// nothing here for a host-side matcher to act on. `driver_id` and `device_profile_id` record
/// where the blob came from, because a blob is meaningless to any other driver or model.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceTemplate {
    /// Template schema major version.
    pub schema_major: u32,
    /// The driver that produced the blob. A blob is not portable across drivers.
    pub driver_id: String,
    /// The device model that produced the blob.
    pub device_profile_id: String,
    /// Experimental fixed matching policy name.
    pub policy: String,
    /// The device's own template. Opaque here, and sensitive biometric data.
    pub blob: Vec<u8>,
}

impl fmt::Debug for DeviceTemplate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceTemplate")
            .field("schema_major", &self.schema_major)
            .field("driver_id", &self.driver_id)
            .field("device_profile_id", &self.device_profile_id)
            .field("policy", &self.policy)
            .field(
                "blob",
                &format_args!("<{} bytes redacted>", self.blob.len()),
            )
            .finish()
    }
}

/// A stored template, discriminated by where the matching happens.
///
/// The two arms are deliberately not merged into one struct with optional fields. Their policies,
/// their validation rules, and the very question they answer are disjoint, and an enum makes the
/// mismatches unrepresentable: a match-on-chip template cannot carry minutiae to be scored against
/// the NBIS threshold, and a host-image template cannot be smuggled to a sensor as a device blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateRecord {
    /// Matching runs on the host over NBIS minutiae.
    HostImage(HostImageTemplate),
    /// Matching runs on the sensor over its own opaque template.
    MatchOnChip(DeviceTemplate),
}

impl TemplateRecord {
    /// The schema major version this record declares, whichever arm it is.
    #[must_use]
    pub const fn schema_major(&self) -> u32 {
        match self {
            Self::HostImage(template) => template.schema_major,
            Self::MatchOnChip(template) => template.schema_major,
        }
    }

    /// The matching policy this record declares, whichever arm it is.
    #[must_use]
    pub fn policy(&self) -> &str {
        match self {
            Self::HostImage(template) => &template.policy,
            Self::MatchOnChip(template) => &template.policy,
        }
    }
}

/// Result of one host-side, fixed-policy verification.
///
/// Only the host-image path produces this. A match-on-chip sensor reports [`MatchVerdict`]
/// instead, because it exposes no score and no threshold to compare one against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerificationResult {
    /// Maximum BOZORTH3 score across enrollment samples.
    pub score: u32,
    /// Fixed experimental acceptance threshold.
    pub threshold: u32,
    /// Whether `score >= threshold`.
    pub matched: bool,
    /// Probe's mean MINDTCT minutia quality.
    pub probe_quality: u8,
}

/// Result of one on-device comparison.
///
/// Deliberately just the verdict. The sensor decides, using a threshold and a scoring function it
/// does not publish, so inventing a host-side score here would be fabricating precision the device
/// never reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchVerdict {
    /// Whether the device recognised the presented finger.
    pub matched: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_image_rejects_invalid_geometry_without_panicking() {
        assert!(CanonicalImage::new(0, 1, 500, 500, Vec::new()).is_err());
        assert!(CanonicalImage::new(1, 0, 500, 500, Vec::new()).is_err());
        assert!(CanonicalImage::new(u32::MAX, u32::MAX, 500, 500, Vec::new()).is_err());
        assert!(CanonicalImage::new(2, 2, 500, 500, vec![0; 3]).is_err());
        assert!(CanonicalImage::new(4097, 4097, 500, 500, vec![0; MAX_IMAGE_BYTES + 1]).is_err());
        assert!(CanonicalImage::new(1, 1, 0, 500, vec![0]).is_err());
        assert!(CanonicalImage::new(1, 1, 500, 0, vec![0]).is_err());
    }

    #[test]
    fn image_area_product_overflow_is_rejected_without_panicking() {
        assert!(checked_pixel_count(usize::MAX, 2).is_err());
    }

    #[test]
    fn debug_redacts_pixels_and_minutiae() {
        let image = CanonicalImage::new(1, 1, 500, 500, vec![123]).unwrap();
        let rendered = format!("{image:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("123"));

        let minutia = MinutiaRecord {
            x: 11,
            y: 22,
            theta: 33,
            quality: 44,
        };
        assert_eq!(format!("{minutia:?}"), "MinutiaRecord(<redacted>)");

        let sample = TemplateSample {
            mean_quality: 44,
            minutiae: vec![minutia],
        };
        let rendered_sample = format!("{sample:?}");
        assert!(rendered_sample.contains("<1 redacted>"));
        assert!(!rendered_sample.contains("x: 11"));

        let template = TemplateRecord::HostImage(HostImageTemplate {
            schema_major: 2,
            engine_id: "engine".to_owned(),
            engine_version: "version".to_owned(),
            capture_profile_id: "profile".to_owned(),
            policy: "policy".to_owned(),
            samples: vec![sample],
        });
        let rendered_template = format!("{template:?}");
        assert!(rendered_template.contains("<1 redacted>"));
        assert!(!rendered_template.contains("x: 11"));
    }

    #[test]
    fn debug_redacts_the_device_template_blob() {
        // The blob is the whole biometric on a match-on-chip sensor, so it gets the same
        // treatment as pixels and minutiae: length only, never content.
        let template = DeviceTemplate {
            schema_major: 2,
            driver_id: "driver".to_owned(),
            device_profile_id: "profile".to_owned(),
            policy: "policy".to_owned(),
            blob: vec![0xAB, 0xCD, 0xEF],
        };
        let rendered = format!("{template:?}");
        assert!(rendered.contains("<3 bytes redacted>"));
        assert!(!rendered.contains("171"));
        assert!(!rendered.contains("AB"));
    }

    #[test]
    fn template_record_exposes_the_common_fields_from_either_arm() {
        let host = TemplateRecord::HostImage(HostImageTemplate {
            schema_major: 2,
            engine_id: String::new(),
            engine_version: String::new(),
            capture_profile_id: String::new(),
            policy: "host-policy".to_owned(),
            samples: Vec::new(),
        });
        let device = TemplateRecord::MatchOnChip(DeviceTemplate {
            schema_major: 2,
            driver_id: String::new(),
            device_profile_id: String::new(),
            policy: "device-policy".to_owned(),
            blob: Vec::new(),
        });
        assert_eq!(host.schema_major(), 2);
        assert_eq!(device.schema_major(), 2);
        assert_eq!(host.policy(), "host-policy");
        assert_eq!(device.policy(), "device-policy");
    }
}
