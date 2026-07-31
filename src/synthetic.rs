use crate::source::CaptureSource;
use crate::{CanonicalImage, CaptureKind, CaptureMetadata, Result};

/// Synthetic M0 capture profile.
pub const SYNTHETIC_CAPTURE_PROFILE: &str = "synthetic-gray8-200x240-500ppi-v1";
/// Synthetic image width.
pub const SYNTHETIC_WIDTH: u32 = 200;
/// Synthetic image height.
pub const SYNTHETIC_HEIGHT: u32 = 240;
/// Synthetic scan resolution.
pub const SYNTHETIC_PPI: u16 = 500;

pub(crate) struct SyntheticSource {
    identity: u64,
    impression: u64,
}

impl SyntheticSource {
    pub(crate) const fn new(identity: u64, impression: u64) -> Self {
        Self {
            identity,
            impression,
        }
    }
}

impl CaptureSource for SyntheticSource {
    fn capture(&mut self) -> Result<(CanonicalImage, CaptureMetadata)> {
        generate_synthetic(self.identity, self.impression)
    }
}

/// Generate a deterministic procedural fingerprint-like capture.
///
/// `identity` fixes ridge flow and dislocation placement. `impression` adds only a small
/// translation and independent low-amplitude sensor noise.
pub fn generate_synthetic(
    identity: u64,
    impression: u64,
) -> Result<(CanonicalImage, CaptureMetadata)> {
    let base = render_identity(identity);
    let pixels = impression_transform(&base, identity, impression);
    let image = CanonicalImage::new(
        SYNTHETIC_WIDTH,
        SYNTHETIC_HEIGHT,
        SYNTHETIC_PPI,
        SYNTHETIC_PPI,
        pixels,
    )?;
    let metadata = CaptureMetadata {
        capture_profile_id: SYNTHETIC_CAPTURE_PROFILE.to_owned(),
        capture_kind: CaptureKind::Press,
        partial: false,
        x_resolution_ppi: SYNTHETIC_PPI,
        y_resolution_ppi: SYNTHETIC_PPI,
    };
    Ok((image, metadata))
}

struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1))
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

struct RidgeField {
    seed: u64,
    period: f64,
    angle_degrees: f64,
    curve: f64,
    noise: i64,
    dislocations: usize,
}

fn identity_hash(identity: u64) -> u64 {
    let mut value = identity ^ 0xA076_1D64_78BD_642F;
    value ^= value >> 30;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn ridge_field(identity: u64) -> RidgeField {
    let hash = identity_hash(identity);
    let mut field = match identity % 3 {
        0 => RidgeField {
            seed: hash,
            period: 9.0,
            angle_degrees: 20.0,
            curve: 0.0018,
            noise: 18,
            dislocations: 10,
        },
        1 => RidgeField {
            seed: hash,
            period: 10.0,
            angle_degrees: 55.0,
            curve: 0.0036,
            noise: 18,
            dislocations: 14,
        },
        _ => RidgeField {
            seed: hash,
            period: 10.0,
            angle_degrees: 8.0,
            curve: 0.0060,
            noise: 16,
            dislocations: 14,
        },
    };
    field.period += f64::from(u8::try_from(hash & 7).unwrap_or(0)) * 0.04 - 0.14;
    field.angle_degrees += f64::from(u8::try_from((hash >> 8) & 15).unwrap_or(0)) * 0.2 - 1.5;
    field.curve += f64::from(u8::try_from((hash >> 16) & 7).unwrap_or(0)) * 0.000_02 - 0.000_07;
    field
}

fn render_identity(identity: u64) -> Vec<u8> {
    const AMPLITUDE: f64 = 95.0;
    const MIDPOINT: f64 = 128.0;
    let field = ridge_field(identity);
    let singularities = dislocations(&field);
    let width = SYNTHETIC_WIDTH as usize;
    let height = SYNTHETIC_HEIGHT as usize;
    let (center_x, center_y) = (
        f64::from(SYNTHETIC_WIDTH) / 2.0,
        f64::from(SYNTHETIC_HEIGHT) / 2.0,
    );
    let (cosine, sine) = (
        field.angle_degrees.to_radians().cos(),
        field.angle_degrees.to_radians().sin(),
    );
    let frequency = 2.0 * std::f64::consts::PI / field.period;
    let mut texture = Lcg::new(field.seed);
    let mut pixels = vec![0_u8; width * height];

    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - center_x;
            let dy = y as f64 - center_y;
            let normal = dx * cosine + dy * sine;
            let tangent = -dx * sine + dy * cosine;
            let mut phase = frequency * (normal + field.curve * tangent * tangent);
            for &(singularity_x, singularity_y, charge) in &singularities {
                phase += charge * (y as f64 - singularity_y).atan2(x as f64 - singularity_x);
            }
            let texture_noise = texture.next() as i64 % (2 * field.noise + 1) - field.noise;
            pixels[y * width + x] =
                clamp_gray(MIDPOINT + AMPLITUDE * phase.cos() + texture_noise as f64);
        }
    }
    pixels
}

fn dislocations(field: &RidgeField) -> Vec<(f64, f64, f64)> {
    let mut random = Lcg::new(field.seed ^ 0x0D15_104A);
    let width = SYNTHETIC_WIDTH as usize;
    let height = SYNTHETIC_HEIGHT as usize;
    let margin = (2.0 * field.period) as usize + 10;
    let separation = (field.period.round() as usize).max(4);
    let mut singularities = Vec::with_capacity(field.dislocations * 2);
    for _ in 0..field.dislocations {
        let x = margin + random.next() as usize % (width - 2 * margin);
        let y = margin + random.next() as usize % (height - 2 * margin);
        let x_offset = separation + random.next() as usize % 3;
        let y_offset = random.next() as i64 % 3 - 1;
        singularities.push((x as f64, y as f64, 1.0));
        singularities.push(((x + x_offset) as f64, (y as i64 + y_offset) as f64, -1.0));
    }
    singularities
}

fn impression_transform(base: &[u8], identity: u64, impression: u64) -> Vec<u8> {
    let width = SYNTHETIC_WIDTH as usize;
    let height = SYNTHETIC_HEIGHT as usize;
    let impression_hash = identity_hash(impression ^ identity.rotate_left(19));
    let shift_x = i32::from(u8::try_from(impression_hash % 5).unwrap_or(0)) - 2;
    let shift_y = i32::from(u8::try_from((impression_hash >> 8) % 5).unwrap_or(0)) - 2;
    let mut random = Lcg::new(impression_hash ^ 0x51A7_0A5E);
    let mut pixels = vec![240_u8; base.len()];

    for y in 0..height {
        for x in 0..width {
            let source_x = i32::try_from(x).unwrap_or(i32::MAX) - shift_x;
            let source_y = i32::try_from(y).unwrap_or(i32::MAX) - shift_y;
            if source_x < 0
                || source_y < 0
                || source_x >= i32::try_from(width).unwrap_or(i32::MAX)
                || source_y >= i32::try_from(height).unwrap_or(i32::MAX)
            {
                continue;
            }
            let source_index = source_y as usize * width + source_x as usize;
            let noise = random.next() as i64 % 9 - 4;
            pixels[y * width + x] = clamp_gray(f64::from(base[source_index]) + noise as f64);
        }
    }
    pixels
}

fn clamp_gray(value: f64) -> u8 {
    (value + 0.5).floor().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_is_deterministic_and_separates_inputs() {
        let first = generate_synthetic(7, 3).unwrap().0;
        let again = generate_synthetic(7, 3).unwrap().0;
        let other_impression = generate_synthetic(7, 4).unwrap().0;
        let other_identity = generate_synthetic(8, 3).unwrap().0;
        assert!(first == again);
        assert!(first != other_impression);
        assert!(first != other_identity);
        assert_eq!(first.width(), SYNTHETIC_WIDTH);
        assert_eq!(first.height(), SYNTHETIC_HEIGHT);
    }

    #[test]
    fn different_identities_change_flow_and_ridge_endings() {
        let first = ridge_field(7);
        let second = ridge_field(10);
        assert_eq!(7 % 3, 10 % 3, "exercise identities in the same base family");
        assert!(
            first.period != second.period
                || first.angle_degrees != second.angle_degrees
                || first.curve != second.curve
        );
        assert_ne!(dislocations(&first), dislocations(&second));
    }
}
