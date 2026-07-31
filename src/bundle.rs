use crate::domain::MAX_IMAGE_BYTES;
use crate::io_util::read_bounded_regular_file;
use crate::{CanonicalImage, CaptureKind, CaptureMetadata, Error, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

const CAPTURE_MANIFEST: &str = "capture.json";
const CAPTURE_IMAGE: &str = "image.pgm";
const CAPTURE_BUNDLE_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: usize = 64 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureManifestV1 {
    version: u32,
    capture_profile_id: String,
    capture_kind: CaptureKind,
    partial: bool,
    width: u32,
    height: u32,
    x_resolution_ppi: u16,
    y_resolution_ppi: u16,
    image: String,
}

/// Save fixed-name `capture.json` and binary P5 `image.pgm` files in a new directory.
pub fn save_capture_bundle(
    directory: &Path,
    image: &CanonicalImage,
    metadata: &CaptureMetadata,
) -> Result<()> {
    metadata.validate()?;
    validate_consistency(image, metadata)?;
    fs::create_dir(directory).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            Error::invalid("capture output already exists; refusing to overwrite")
        } else {
            Error::io(error)
        }
    })?;

    let manifest = CaptureManifestV1 {
        version: CAPTURE_BUNDLE_VERSION,
        capture_profile_id: metadata.capture_profile_id.clone(),
        capture_kind: metadata.capture_kind,
        partial: metadata.partial,
        width: image.width(),
        height: image.height(),
        x_resolution_ppi: image.x_resolution_ppi(),
        y_resolution_ppi: image.y_resolution_ppi(),
        image: CAPTURE_IMAGE.to_owned(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(Error::invalid("capture manifest exceeds size limit"));
    }
    create_new_file(&directory.join(CAPTURE_MANIFEST), &manifest_bytes)?;
    create_new_file(&directory.join(CAPTURE_IMAGE), &encode_pgm(image))?;
    Ok(())
}

/// Load and validate a fixed-name capture bundle.
pub fn load_capture_bundle(directory: &Path) -> Result<(CanonicalImage, CaptureMetadata)> {
    validate_bundle_entries(directory)?;
    let manifest_path = directory.join(CAPTURE_MANIFEST);
    let manifest_bytes =
        read_bounded_regular_file(&manifest_path, MAX_MANIFEST_BYTES, "capture manifest")?;
    let manifest: CaptureManifestV1 = serde_json::from_slice(&manifest_bytes)?;
    if manifest.version != CAPTURE_BUNDLE_VERSION {
        return Err(Error::invalid("unsupported capture bundle version"));
    }
    if manifest.image != CAPTURE_IMAGE {
        return Err(Error::invalid(
            "capture bundle must use fixed image.pgm name",
        ));
    }

    let metadata = CaptureMetadata {
        capture_profile_id: manifest.capture_profile_id,
        capture_kind: manifest.capture_kind,
        partial: manifest.partial,
        x_resolution_ppi: manifest.x_resolution_ppi,
        y_resolution_ppi: manifest.y_resolution_ppi,
    };
    metadata.validate()?;

    let pgm_path = directory.join(CAPTURE_IMAGE);
    let maximum_pgm_size = MAX_IMAGE_BYTES
        .checked_add(128)
        .ok_or_else(|| Error::invalid("PGM size limit overflow"))?;
    let pgm = read_bounded_regular_file(&pgm_path, maximum_pgm_size, "PGM")?;
    let (width, height, pixels) = decode_pgm(&pgm)?;
    if width != manifest.width || height != manifest.height {
        return Err(Error::invalid("manifest and PGM dimensions differ"));
    }

    let image = CanonicalImage::new(
        width,
        height,
        metadata.x_resolution_ppi,
        metadata.y_resolution_ppi,
        pixels,
    )?;
    validate_consistency(&image, &metadata)?;
    Ok((image, metadata))
}

fn validate_bundle_entries(directory: &Path) -> Result<()> {
    let mut saw_manifest = false;
    let mut saw_image = false;
    for entry in fs::read_dir(directory).map_err(Error::io)? {
        let entry = entry.map_err(Error::io)?;
        let name = entry.file_name();
        if name == CAPTURE_MANIFEST {
            if saw_manifest {
                return Err(Error::invalid("capture bundle contains duplicate manifest"));
            }
            saw_manifest = true;
        } else if name == CAPTURE_IMAGE {
            if saw_image {
                return Err(Error::invalid("capture bundle contains duplicate image"));
            }
            saw_image = true;
        } else {
            return Err(Error::invalid(
                "capture bundle contains an unexpected entry",
            ));
        }
    }
    if !saw_manifest || !saw_image {
        return Err(Error::invalid(
            "capture bundle must contain capture.json and image.pgm",
        ));
    }
    Ok(())
}

fn validate_consistency(image: &CanonicalImage, metadata: &CaptureMetadata) -> Result<()> {
    if image.x_resolution_ppi() != metadata.x_resolution_ppi
        || image.y_resolution_ppi() != metadata.y_resolution_ppi
    {
        return Err(Error::invalid(
            "image and capture metadata resolutions differ",
        ));
    }
    Ok(())
}

fn create_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(Error::io)?;
    file.write_all(bytes).map_err(Error::io)?;
    file.sync_all().map_err(Error::io)
}

fn encode_pgm(image: &CanonicalImage) -> Vec<u8> {
    let header = format!("P5\n{} {}\n255\n", image.width(), image.height());
    let mut bytes = Vec::with_capacity(header.len() + image.pixels().len());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(image.pixels());
    bytes
}

fn decode_pgm(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let mut cursor = 0;
    let magic = next_token(bytes, &mut cursor)?;
    if magic != b"P5" {
        return Err(Error::invalid("PGM must use binary P5 encoding"));
    }
    let width = parse_u32(next_token(bytes, &mut cursor)?, "PGM width")?;
    let height = parse_u32(next_token(bytes, &mut cursor)?, "PGM height")?;
    let max_value = parse_u32(next_token(bytes, &mut cursor)?, "PGM maxval")?;
    if max_value != 255 {
        return Err(Error::invalid("PGM maxval must be 255"));
    }
    if cursor >= bytes.len() || !bytes[cursor].is_ascii_whitespace() {
        return Err(Error::invalid("PGM header is truncated"));
    }
    if bytes[cursor] == b'\r' && bytes.get(cursor + 1) == Some(&b'\n') {
        cursor += 2;
    } else {
        cursor += 1;
    }

    let area = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| Error::invalid("PGM dimensions overflow"))?;
    let expected =
        usize::try_from(area).map_err(|_| Error::invalid("PGM dimensions exceed this platform"))?;
    if expected > MAX_IMAGE_BYTES {
        return Err(Error::invalid("PGM exceeds image size limit"));
    }
    if bytes.len().saturating_sub(cursor) != expected {
        return Err(Error::invalid(
            "PGM pixel length does not match its dimensions",
        ));
    }
    Ok((width, height, bytes[cursor..].to_vec()))
}

fn next_token<'a>(bytes: &'a [u8], cursor: &mut usize) -> Result<&'a [u8]> {
    loop {
        while bytes.get(*cursor).is_some_and(u8::is_ascii_whitespace) {
            *cursor += 1;
        }
        if bytes.get(*cursor) != Some(&b'#') {
            break;
        }
        while bytes.get(*cursor).is_some_and(|byte| *byte != b'\n') {
            *cursor += 1;
        }
    }
    let start = *cursor;
    while bytes
        .get(*cursor)
        .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'#')
    {
        *cursor += 1;
    }
    if start == *cursor {
        return Err(Error::invalid("PGM header is truncated"));
    }
    Ok(&bytes[start..*cursor])
}

fn parse_u32(token: &[u8], label: &str) -> Result<u32> {
    let text =
        std::str::from_utf8(token).map_err(|_| Error::invalid(format!("{label} is not ASCII")))?;
    text.parse()
        .map_err(|_| Error::invalid(format!("{label} is invalid")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture() -> (CanonicalImage, CaptureMetadata) {
        let image = CanonicalImage::new(3, 2, 500, 500, vec![0, 10, 32, 100, 200, 255]).unwrap();
        let metadata = CaptureMetadata {
            capture_profile_id: "test-profile".into(),
            capture_kind: CaptureKind::Press,
            partial: false,
            x_resolution_ppi: 500,
            y_resolution_ppi: 500,
        };
        (image, metadata)
    }

    #[test]
    fn bundle_round_trip_is_byte_exact() {
        let temp = tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let (image, metadata) = fixture();
        save_capture_bundle(&first, &image, &metadata).unwrap();
        let loaded = load_capture_bundle(&first).unwrap();
        assert!(loaded == (image, metadata));
        save_capture_bundle(&second, &loaded.0, &loaded.1).unwrap();
        assert!(
            fs::read(first.join(CAPTURE_MANIFEST)).unwrap()
                == fs::read(second.join(CAPTURE_MANIFEST)).unwrap()
        );
        assert!(
            fs::read(first.join(CAPTURE_IMAGE)).unwrap()
                == fs::read(second.join(CAPTURE_IMAGE)).unwrap()
        );
        assert!(save_capture_bundle(&first, &loaded.0, &loaded.1).is_err());
    }

    #[test]
    fn pgm_rejects_truncation_bad_maxval_and_wrong_pixel_length() {
        assert!(decode_pgm(b"P").is_err());
        assert!(decode_pgm(b"P5\n2 2\n255\n\x00\x01").is_err());
        assert!(decode_pgm(b"P5\n1 1\n65535\n\x00").is_err());
        assert!(decode_pgm(b"P2\n1 1\n255\n\x00").is_err());
        assert!(decode_pgm(b"P5\n1 1\n255").is_err());
    }

    #[test]
    fn manifest_rejects_size_and_schema_mismatch() {
        let temp = tempdir().unwrap();
        let oversized = temp.path().join("oversized");
        let (image, metadata) = fixture();
        save_capture_bundle(&oversized, &image, &metadata).unwrap();
        fs::write(
            oversized.join(CAPTURE_MANIFEST),
            vec![b' '; MAX_MANIFEST_BYTES + 1],
        )
        .unwrap();
        assert!(load_capture_bundle(&oversized).is_err());

        let bad_version = temp.path().join("bad-version");
        save_capture_bundle(&bad_version, &image, &metadata).unwrap();
        let manifest_path = bad_version.join(CAPTURE_MANIFEST);
        let bytes = fs::read(&manifest_path).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["version"] = 2.into();
        fs::write(&manifest_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(load_capture_bundle(&bad_version).is_err());
    }

    #[test]
    fn bundle_rejects_unexpected_entries() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("bundle");
        let (image, metadata) = fixture();
        save_capture_bundle(&bundle, &image, &metadata).unwrap();
        fs::write(bundle.join("unexpected.bin"), b"not part of version 1").unwrap();
        assert!(load_capture_bundle(&bundle).is_err());
    }
}
