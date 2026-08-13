//! 썸네일 (§20.4). **원본 미디어를 복사하지 않는다** (T42).
//!
//! | 항목 | 크기 |
//! |---|---|
//! | 썸네일 (긴 변 `thumb_max_edge_px`) | ~15 KB |
//! | `source.sha256` | 32 B |
//! | `source.origin` 원본 경로 | 수백 B |
//!
//! | `kind` | 썸네일 원본 |
//! |---|---|
//! | `image` | 원본 이미지 |
//! | `video` (로컬) | 추출한 첫 프레임 |
//! | `video`·`audio` (YouTube) | YouTube 썸네일을 **이때만 한 번** 내려받아 저장 (T11e) |
//! | `text` | 없음. 아카이브는 서술문 앞 40자를 타일에 렌더한다 (T70c) |

use soul_core::error::{Result, SoulError};
use soul_core::paths::Paths;

/// 이미지 바이트에서 썸네일을 만들어 `cache/thumbs/ab/cd/<sha>.jpg`에 저장한다.
///
/// 넘겨받는 것은 **이미 메모리에 있는 이미지 바이트**다. 원본 파일을 복사하지도,
/// 원본 경로를 다시 읽지도 않는다 (T42).
pub fn write(
    paths: &Paths,
    sha256_hex: &str,
    source_image: &[u8],
    max_edge_px: u32,
) -> Result<std::path::PathBuf> {
    if sha256_hex.len() < 4 || !sha256_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SoulError::invalid(format!(
            "썸네일 키가 sha256 16진 문자열이 아닙니다: {sha256_hex}"
        )));
    }
    let img = image::load_from_memory(source_image)
        .map_err(|e| SoulError::invalid(format!("썸네일 원본을 디코딩하지 못했습니다: {e}")))?;

    // 이미지 경로와 같은 후처리: 흰 배경 합성 → 축소만 → JPEG.
    let img = crate::image_in::flatten_on_white(img);
    let img = crate::image_in::downscale_to_max_edge(img, max_edge_px);
    let jpeg = crate::image_in::encode_jpeg(&img, crate::image_in::JPEG_QUALITY)?;

    let dest = paths.thumb_file(sha256_hex);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // 같은 디렉토리에 임시로 쓰고 rename — 반쯤 쓰인 썸네일이 남지 않게 한다.
    let tmp = dest.with_extension("jpg.tmp");
    std::fs::write(&tmp, &jpeg)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

pub fn exists(paths: &Paths, sha256_hex: &str) -> bool {
    paths.thumb_file(sha256_hex).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    const SHA: &str = "abcd1234ef567890abcd1234ef567890abcd1234ef567890abcd1234ef567890";

    fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let img =
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb(rgb)));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn writes_to_sharded_path_and_shrinks() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        let src = png(1000, 500, [12, 34, 56]);

        assert!(!exists(&paths, SHA));
        let out = write(&paths, SHA, &src, 256).unwrap();
        assert_eq!(out, paths.thumb_file(SHA));
        assert!(out.ends_with(format!("thumbs/ab/cd/{SHA}.jpg")));
        assert!(exists(&paths, SHA));

        let bytes = std::fs::read(&out).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!(img.dimensions(), (256, 128));
    }

    /// T42 — 원본을 복사하지 않는다. 저장되는 것은 재인코딩된 작은 JPEG뿐이다.
    #[test]
    fn t42_does_not_copy_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        let src = png(2000, 2000, [200, 30, 30]);

        let out = write(&paths, SHA, &src, 256).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert_ne!(bytes, src, "원본 바이트를 그대로 두면 안 된다");
        assert!(bytes.len() < src.len(), "썸네일이 원본보다 작아야 한다");
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "JPEG 여야 한다");

        // 캐시 트리에 생긴 파일은 썸네일 하나뿐이다.
        let mut files = Vec::new();
        collect(&paths.thumbs(), &mut files);
        assert_eq!(files, vec![out]);
    }

    /// 원본이 상한보다 작으면 확대하지 않는다 (T60과 같은 규칙).
    #[test]
    fn never_upscales() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        let out = write(&paths, SHA, &png(64, 40, [1, 2, 3]), 256).unwrap();
        let img = image::load_from_memory(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(img.dimensions(), (64, 40));
    }

    /// 알파는 흰색 위에 합성한다 — 아카이브 타일이 검게 나오지 않게.
    #[test]
    fn alpha_becomes_white() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        let rgba = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            32,
            32,
            image::Rgba([0, 0, 0, 0]),
        ));
        let mut buf = std::io::Cursor::new(Vec::new());
        rgba.write_to(&mut buf, image::ImageFormat::Png).unwrap();

        let out = write(&paths, SHA, &buf.into_inner(), 256).unwrap();
        let img = image::load_from_memory(&std::fs::read(&out).unwrap()).unwrap();
        let px = img.get_pixel(16, 16);
        assert!(px[0] > 240 && px[1] > 240 && px[2] > 240, "{px:?}");
    }

    #[test]
    fn rejects_bad_key_and_bad_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        assert!(write(&paths, "zz", &png(8, 8, [0, 0, 0]), 256).is_err());
        assert!(write(&paths, SHA, b"not an image", 256).is_err());
    }

    /// 다시 쓰면 같은 경로를 덮어쓴다 (임시 파일이 남지 않는다).
    #[test]
    fn rewrite_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        write(&paths, SHA, &png(100, 100, [0, 0, 0]), 256).unwrap();
        let out = write(&paths, SHA, &png(100, 100, [255, 255, 255]), 256).unwrap();
        let mut files = Vec::new();
        collect(&paths.thumbs(), &mut files);
        assert_eq!(files, vec![out]);
    }

    fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(&p, out);
            } else {
                out.push(p);
            }
        }
        out.sort();
    }
}
