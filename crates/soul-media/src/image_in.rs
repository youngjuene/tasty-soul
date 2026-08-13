//! 이미지 입력 처리 (§9.4). **포맷을 가리지 않는다.**
//!
//! | 계층 | 처리 |
//! |---|---|
//! | `image` 크레이트가 디코딩 가능 | JPEG·PNG·WebP·GIF·TIFF·BMP·ICO·TGA·PNM 등 — 직접 |
//! | ffmpeg으로 디코딩 가능 | HEIC·HEIF·AVIF 등 — ffmpeg 경유 |
//! | 벡터 | SVG — 긴 변 1280으로 래스터화 (T25) |
//! | RAW | CR2·NEF·ARW·DNG — 내장 프리뷰 JPEG를 추출 (T25d) |
//! | 그 외 | 디코딩 실패 시에만 거부하고 사유를 표시한다 |
//!
//! **포맷별 화이트리스트를 두지 않는다.** 시도해보고 실패하면 거부하는 편이 낫다.
//!
//! ## 공통 후처리 — 순서가 중요하다
//!
//! - 알파 채널이 있으면 **흰색 위에 합성한다.** 합성 없이 JPEG로 바꾸면 투명 영역이
//!   검게 나온다 (T24)
//! - 애니메이션(GIF·APNG·움직이는 WebP)은 **첫 프레임만** 쓴다 (T25c)
//! - 여러 장이 든 파일(다중 페이지 TIFF, ICO)은 **가장 큰 것 하나만** (T25b)
//! - **긴 변 `image_max_edge_px`(1280)로 리사이즈. 축소만 하고 확대하지 않는다** (T60)
//! - JPEG q=85
//! - **EXIF 전체 제거 후 전송** (T24b). 재인코딩하므로 자동으로 제거되지만,
//!   결과 바이트에 EXIF 세그먼트가 0바이트인지 테스트로 확인한다
//! - 원본은 복사하지 않고 썸네일만 저장한다 (§20.4, T42)

use image::{DynamicImage, ImageEncoder};
use soul_core::error::{Result, SoulError};

/// §9.4 — 재인코딩 품질.
pub(crate) const JPEG_QUALITY: u8 = 85;

pub struct PreparedImage {
    /// 전송용 JPEG 바이트. EXIF 없음.
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// **원본 파일의 mime.** 변환 결과가 아니다 (§9.4).
    pub source_mime: String,
}

/// 파일을 전송 가능한 JPEG로 만든다.
pub fn prepare(
    path: &std::path::Path,
    max_edge_px: u32,
    ffmpeg: Option<&crate::ffmpeg::FfmpegTools>,
) -> Result<PreparedImage> {
    let bytes = std::fs::read(path)?;
    if bytes.is_empty() {
        return Err(SoulError::invalid(format!(
            "빈 파일입니다: {}",
            path.display()
        )));
    }
    // §9.4 — source.mime 은 변환 결과가 아니라 원본 파일의 mime.
    let source_mime = sniff_mime(&bytes, path);

    // 화이트리스트 없이 계층별로 시도한다. 전부 실패했을 때만 사유를 모아 거부한다.
    let mut reasons: Vec<String> = Vec::new();
    let decoded = decode_any(&bytes, path, max_edge_px, ffmpeg, &mut reasons).ok_or_else(|| {
        SoulError::invalid(format!(
            "이미지를 디코딩하지 못했습니다 ({source_mime}): {}",
            reasons.join(" / ")
        ))
    })?;

    // 공통 후처리 — 순서가 중요하다.
    let flattened = flatten_on_white(decoded); // T24
    let resized = downscale_to_max_edge(flattened, max_edge_px); // T60
    let width = resized.width();
    let height = resized.height();
    let jpeg = encode_jpeg(&resized, JPEG_QUALITY)?; // T24b: 재인코딩 → EXIF 소멸

    Ok(PreparedImage {
        jpeg,
        width,
        height,
        source_mime,
    })
}

/// §9.4 계층 1~4. 실패 사유를 `reasons`에 쌓는다.
fn decode_any(
    bytes: &[u8],
    path: &std::path::Path,
    max_edge_px: u32,
    ffmpeg: Option<&crate::ffmpeg::FfmpegTools>,
    reasons: &mut Vec<String>,
) -> Option<DynamicImage> {
    // 1. image 크레이트
    match decode_with_image_crate(bytes) {
        Ok(img) => return Some(img),
        Err(e) => reasons.push(format!("image: {e}")),
    }
    // 2. SVG 래스터화 (T25)
    if looks_like_svg(bytes) {
        match rasterize_svg(bytes, max_edge_px) {
            Ok(img) => return Some(img),
            Err(e) => reasons.push(format!("svg: {e}")),
        }
    }
    // 3. RAW 내장 프리뷰 JPEG (T25d)
    match extract_embedded_jpeg(bytes) {
        Some(jpeg) => match image::load_from_memory(&jpeg) {
            Ok(img) => return Some(img),
            Err(e) => reasons.push(format!("내장 JPEG: {e}")),
        },
        None => reasons.push("내장 JPEG: 없음".to_string()),
    }
    // 4. ffmpeg 경유 (HEIC·AVIF 등)
    match ffmpeg {
        Some(tools) => match decode_via_ffmpeg(tools, path) {
            Ok(img) => return Some(img),
            Err(e) => reasons.push(format!("ffmpeg: {e}")),
        },
        None => reasons.push("ffmpeg: 사용 불가".to_string()),
    }
    None
}

/// 계층 1. 다중 페이지 TIFF는 가장 큰 페이지를 고른다 (T25b).
/// ICO는 `image` 크레이트가 이미 가장 큰 엔트리를 고르고, 애니메이션(GIF·APNG·WebP)은
/// 첫 프레임만 디코딩한다 (T25c).
fn decode_with_image_crate(bytes: &[u8]) -> image::ImageResult<DynamicImage> {
    if matches!(image::guess_format(bytes), Ok(image::ImageFormat::Tiff)) {
        if let Some(rewritten) = tiff_pick_largest_page(bytes) {
            if let Ok(img) = image::load_from_memory(&rewritten) {
                return Ok(img);
            }
        }
    }
    image::load_from_memory(bytes)
}

/// 계층 4. ffmpeg으로 한 프레임만 PNG로 뽑아 다시 읽는다.
fn decode_via_ffmpeg(
    tools: &crate::ffmpeg::FfmpegTools,
    path: &std::path::Path,
) -> std::result::Result<DynamicImage, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let out = dir.path().join("frame.png");
    let input = path.to_str().ok_or("경로가 UTF-8이 아닙니다")?;
    let output = out.to_str().ok_or("임시 경로가 UTF-8이 아닙니다")?;
    crate::ffmpeg::run(
        tools,
        &[
            "-y",
            "-nostdin",
            "-loglevel",
            "error",
            "-i",
            input,
            "-frames:v",
            "1",
            output,
        ],
    )
    .map_err(|e| e.to_string())?;
    let png = std::fs::read(&out).map_err(|e| e.to_string())?;
    image::load_from_memory(&png).map_err(|e| e.to_string())
}

// ─── 공통 후처리 ────────────────────────────────────────────────────────────

/// T24 — 알파를 흰색 위에 합성한다. 빼먹으면 투명 영역이 JPEG에서 검게 나온다.
pub(crate) fn flatten_on_white(img: DynamicImage) -> DynamicImage {
    if !img.color().has_alpha() {
        return img;
    }
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let mut out = image::RgbImage::new(w, h);
    for (x, y, px) in rgba.enumerate_pixels() {
        let a = u32::from(px[3]);
        let over = |c: u8| -> u8 {
            // c*a + 255*(255-a) 을 255로 나눈 반올림값
            (((u32::from(c) * a) + (255 * (255 - a)) + 127) / 255).min(255) as u8
        };
        out.put_pixel(x, y, image::Rgb([over(px[0]), over(px[1]), over(px[2])]));
    }
    DynamicImage::ImageRgb8(out)
}

/// T60 — 긴 변을 `max_edge_px`로 **축소만** 한다. 원본이 더 작으면 그대로 둔다.
pub(crate) fn downscale_to_max_edge(img: DynamicImage, max_edge_px: u32) -> DynamicImage {
    if max_edge_px == 0 {
        return img;
    }
    let longest = img.width().max(img.height());
    if longest <= max_edge_px {
        return img;
    }
    img.resize(
        max_edge_px,
        max_edge_px,
        image::imageops::FilterType::Lanczos3,
    )
}

/// q=85 JPEG 재인코딩. 메타데이터를 하나도 넣지 않으므로 EXIF가 사라진다 (T24b).
pub(crate) fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let rgb = img.to_rgb8();
    if rgb.width() == 0 || rgb.height() == 0 {
        return Err(SoulError::invalid("크기가 0인 이미지입니다"));
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| SoulError::invalid(format!("JPEG 인코딩 실패: {e}")))?;
    Ok(out)
}

// ─── SVG ───────────────────────────────────────────────────────────────────

/// 앞부분에 `<svg` 가 보이면 SVG로 본다. 확장자는 믿지 않는다.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    String::from_utf8_lossy(head)
        .to_ascii_lowercase()
        .contains("<svg")
}

/// T25 — 벡터는 원본 픽셀 개념이 없으므로 **긴 변을 `max_edge_px`에 맞춰** 래스터화한다.
fn rasterize_svg(bytes: &[u8], max_edge_px: u32) -> std::result::Result<DynamicImage, String> {
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(bytes, &opt).map_err(|e| e.to_string())?;
    let size = tree.size();
    let (w, h) = (size.width(), size.height());
    if !(w > 0.0 && h > 0.0) {
        return Err("SVG 크기가 0입니다".to_string());
    }
    let target = if max_edge_px == 0 { 1280 } else { max_edge_px } as f32;
    let scale = target / w.max(h);
    let pw = ((w * scale).round() as u32).max(1);
    let ph = ((h * scale).round() as u32).max(1);
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph).ok_or("픽스맵을 만들지 못했습니다")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia는 프리멀티플라이드 RGBA다. 되돌린 뒤 공통 후처리에 맡긴다.
    let data = pixmap.take_demultiplied();
    let buf = image::RgbaImage::from_raw(pw, ph, data).ok_or("픽셀 버퍼 크기 불일치")?;
    Ok(DynamicImage::ImageRgba8(buf))
}

// ─── RAW 내장 프리뷰 ────────────────────────────────────────────────────────

/// RAW 파일에서 내장 프리뷰 JPEG를 뽑는다. SOI/EOI 마커를 스캔해 가장 큰 것을 쓴다.
///
/// EXIF 안에 든 썸네일 JPEG가 프리뷰 JPEG 안에 **중첩**되므로 단순히 첫 EOI를 찾으면
/// 프리뷰가 잘린다. 마커 길이를 따라 세그먼트를 건너뛰며 진짜 EOI를 찾는다.
pub fn extract_embedded_jpeg(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut best: Option<(usize, usize)> = None;
    let mut i = 0usize;
    while i + 3 < bytes.len() {
        if bytes[i] == 0xFF && bytes[i + 1] == 0xD8 && bytes[i + 2] == 0xFF {
            if let Some(end) = jpeg_stream_end(bytes, i) {
                if best.is_none_or(|(s, e)| end - i > e - s) {
                    best = Some((i, end));
                }
                i = end; // 이 스트림 안에 든 중첩 썸네일은 건너뛴다
                continue;
            }
        }
        i += 1;
    }
    best.map(|(s, e)| bytes[s..e].to_vec())
}

/// `start`(SOI)에서 시작하는 JPEG 스트림의 끝(EOI 다음 인덱스)을 돌려준다.
fn jpeg_stream_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 2; // SOI 다음
    loop {
        // 마커가 와야 할 자리다. 아니면 형식 오류.
        if bytes.get(i) != Some(&0xFF) {
            return None;
        }
        let mut m = i + 1;
        while m < bytes.len() && bytes[m] == 0xFF {
            m += 1; // 채움 0xFF
        }
        if m >= bytes.len() {
            return None;
        }
        let marker = bytes[m];
        i = m + 1;
        match marker {
            0xD9 => return Some(i),         // EOI
            0x01 | 0xD0..=0xD8 => continue, // 길이 없는 마커
            0xDA => {
                // SOS — 엔트로피 부호 구간을 지나 다음 마커까지 스캔한다
                let len = read_be16(bytes, i)? as usize;
                if len < 2 {
                    return None;
                }
                i += len;
                loop {
                    if i + 1 >= bytes.len() {
                        return None;
                    }
                    if bytes[i] == 0xFF {
                        let n = bytes[i + 1];
                        // 0xFF00 은 채워넣은 0xFF, RSTn 은 재시작 마커
                        if n != 0x00 && !(0xD0..=0xD7).contains(&n) && n != 0xFF {
                            break;
                        }
                    }
                    i += 1;
                }
            }
            _ => {
                let len = read_be16(bytes, i)? as usize;
                if len < 2 {
                    return None;
                }
                i = i.checked_add(len)?;
                if i > bytes.len() {
                    return None;
                }
            }
        }
    }
}

fn read_be16(bytes: &[u8], at: usize) -> Option<u16> {
    let hi = *bytes.get(at)?;
    let lo = *bytes.get(at + 1)?;
    Some(u16::from(hi) << 8 | u16::from(lo))
}

// ─── 다중 페이지 TIFF ───────────────────────────────────────────────────────

/// T25b — IFD 체인을 훑어 가장 큰 페이지를 찾고, 그 IFD가 첫 장이 되도록 헤더를 고친 사본을
/// 돌려준다. TIFF 오프셋은 파일 절대 위치라 헤더만 바꿔도 유효하다.
/// 다시 쓸 필요가 없으면(페이지가 하나거나 첫 장이 가장 큼) `None`.
fn tiff_pick_largest_page(bytes: &[u8]) -> Option<Vec<u8>> {
    let le = match bytes.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    if read_u16(bytes, 2, le)? != 42 {
        return None; // BigTIFF(43)는 다루지 않는다
    }
    let mut off = read_u32(bytes, 4, le)? as usize;
    let mut pages: Vec<(usize, u64)> = Vec::new();
    while off != 0 && pages.len() < 64 {
        let n = read_u16(bytes, off, le)? as usize;
        let next_at = off.checked_add(2 + n * 12)?;
        if next_at + 4 > bytes.len() {
            return None;
        }
        let (mut w, mut h) = (0u64, 0u64);
        for k in 0..n {
            let e = off + 2 + k * 12;
            let tag = read_u16(bytes, e, le)?;
            if tag != 256 && tag != 257 {
                continue;
            }
            let ty = read_u16(bytes, e + 2, le)?;
            if read_u32(bytes, e + 4, le)? != 1 {
                continue;
            }
            let v = match ty {
                3 => u64::from(read_u16(bytes, e + 8, le)?),
                4 => u64::from(read_u32(bytes, e + 8, le)?),
                _ => continue,
            };
            if tag == 256 {
                w = v;
            } else {
                h = v;
            }
        }
        pages.push((off, w.saturating_mul(h)));
        off = read_u32(bytes, next_at, le)? as usize;
    }
    if pages.len() < 2 {
        return None;
    }
    let (best_off, best_area) = *pages.iter().max_by_key(|(_, area)| *area)?;
    if best_off == pages[0].0 || best_area == 0 {
        return None;
    }
    let mut out = bytes.to_vec();
    write_u32(&mut out, 4, best_off as u32, le);
    // 고른 IFD를 마지막 장으로 만든다
    let n = read_u16(&out, best_off, le)? as usize;
    let next_at = best_off + 2 + n * 12;
    write_u32(&mut out, next_at, 0, le);
    Some(out)
}

fn read_u16(b: &[u8], at: usize, le: bool) -> Option<u16> {
    let s: [u8; 2] = b.get(at..at + 2)?.try_into().ok()?;
    Some(if le {
        u16::from_le_bytes(s)
    } else {
        u16::from_be_bytes(s)
    })
}

fn read_u32(b: &[u8], at: usize, le: bool) -> Option<u32> {
    let s: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
    Some(if le {
        u32::from_le_bytes(s)
    } else {
        u32::from_be_bytes(s)
    })
}

fn write_u32(b: &mut [u8], at: usize, v: u32, le: bool) {
    let s = if le { v.to_le_bytes() } else { v.to_be_bytes() };
    b[at..at + 4].copy_from_slice(&s);
}

// ─── mime 판별 ─────────────────────────────────────────────────────────────

/// magic bytes 우선. 알 수 없을 때만 확장자를 참고한다 (거부 판단에는 쓰지 않는다).
fn sniff_mime(bytes: &[u8], path: &std::path::Path) -> String {
    if let Some(kind) = infer::get(bytes) {
        return kind.mime_type().to_string();
    }
    if looks_like_svg(bytes) {
        return "image/svg+xml".to_string();
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "svg" | "svgz" => "image/svg+xml",
        "arw" => "image/x-sony-arw",
        "nef" => "image/x-nikon-nef",
        "dng" => "image/x-adobe-dng",
        "cr3" => "image/x-canon-cr3",
        "raf" => "image/x-fuji-raf",
        "rw2" => "image/x-panasonic-rw2",
        "orf" => "image/x-olympus-orf",
        _ => "application/octet-stream",
    }
    .to_string()
}

// ─── data URL ──────────────────────────────────────────────────────────────

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// base64 data URL (Responses API `input_image`).
pub fn to_data_url(jpeg: &[u8]) -> String {
    let mut s = String::with_capacity(24 + jpeg.len().div_ceil(3) * 4);
    s.push_str("data:image/jpeg;base64,");
    for chunk in jpeg.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let n = (b0 << 16) | (b1 << 8) | b2;
        s.push(B64[(n >> 18 & 63) as usize] as char);
        s.push(B64[(n >> 12 & 63) as usize] as char);
        if chunk.len() > 1 {
            s.push(B64[(n >> 6 & 63) as usize] as char);
        } else {
            s.push('=');
        }
        if chunk.len() > 2 {
            s.push(B64[(n & 63) as usize] as char);
        } else {
            s.push('=');
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, Rgba, RgbaImage};

    fn tmp_write(name: &str, bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        (dir, p)
    }

    fn png_bytes(img: &DynamicImage) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// T24 — 투명 영역은 흰색이 되어야 한다. 합성을 빼먹으면 검게 나온다.
    #[test]
    fn t24_alpha_is_composited_on_white() {
        let mut src = RgbaImage::new(40, 40);
        for (x, y, px) in src.enumerate_pixels_mut() {
            *px = if x < 20 && y < 20 {
                Rgba([255, 0, 0, 255]) // 좌상단만 불투명 빨강
            } else {
                Rgba([0, 0, 0, 0]) // 나머지는 완전 투명
            };
        }
        let bytes = png_bytes(&DynamicImage::ImageRgba8(src));
        let (_d, path) = tmp_write("alpha.png", &bytes);

        let prepared = prepare(&path, 1280, None).unwrap();
        let out = image::load_from_memory(&prepared.jpeg).unwrap();

        let clear = out.get_pixel(35, 35);
        assert!(
            clear[0] > 240 && clear[1] > 240 && clear[2] > 240,
            "투명 영역이 흰색이어야 하는데 {clear:?}"
        );
        let opaque = out.get_pixel(5, 5);
        assert!(
            opaque[0] > 200 && opaque[1] < 60,
            "빨강이 유지되어야 한다: {opaque:?}"
        );
        assert_eq!(prepared.source_mime, "image/png");
    }

    /// 반투명 픽셀도 흰색과 섞여야 한다.
    #[test]
    fn t24_semi_transparent_blends_toward_white() {
        let mut src = RgbaImage::new(8, 8);
        for px in src.pixels_mut() {
            *px = Rgba([0, 0, 0, 128]); // 50% 검정
        }
        let flat = flatten_on_white(DynamicImage::ImageRgba8(src));
        let px = flat.to_rgb8().get_pixel(4, 4).0;
        assert!(
            (120..=140).contains(&px[0]),
            "50% 검정은 중간 회색이어야 한다: {px:?}"
        );
    }

    /// T24b — 결과 JPEG에 EXIF(APP1) 세그먼트가 남으면 안 된다.
    #[test]
    fn t24b_exif_is_stripped() {
        let src = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            64,
            48,
            image::Rgb([10, 120, 200]),
        ));
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
            .write_image(
                src.to_rgb8().as_raw(),
                64,
                48,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();

        // SOI 바로 뒤에 APP1(EXIF) 세그먼트를 끼워 넣는다.
        let mut payload = Vec::new();
        payload.extend_from_slice(b"Exif\0\0");
        payload.extend_from_slice(b"II\x2a\x00\x08\x00\x00\x00\x00\x00"); // 빈 TIFF 헤더
        payload.extend_from_slice(&[0u8; 64]);
        let seg_len = (payload.len() + 2) as u16;
        let mut with_exif = Vec::new();
        with_exif.extend_from_slice(&jpeg[..2]); // SOI
        with_exif.extend_from_slice(&[0xFF, 0xE1]);
        with_exif.extend_from_slice(&seg_len.to_be_bytes());
        with_exif.extend_from_slice(&payload);
        with_exif.extend_from_slice(&jpeg[2..]);
        assert!(
            contains(&with_exif, b"Exif\0\0"),
            "픽스처에 EXIF가 있어야 한다"
        );

        let (_d, path) = tmp_write("exif.jpg", &with_exif);
        let prepared = prepare(&path, 1280, None).unwrap();

        assert!(
            !contains(&prepared.jpeg, b"Exif\0\0"),
            "EXIF 문자열이 남았다"
        );
        assert!(!has_marker(&prepared.jpeg, 0xE1), "APP1 마커가 남았다");
        assert_eq!(prepared.source_mime, "image/jpeg");
        assert_eq!((prepared.width, prepared.height), (64, 48));
    }

    /// T60 — 원본이 상한보다 작으면 확대하지 않는다.
    #[test]
    fn t60_never_upscales() {
        let src =
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(200, 100, image::Rgb([1, 2, 3])));
        let (_d, path) = tmp_write("small.png", &png_bytes(&src));
        let prepared = prepare(&path, 1280, None).unwrap();
        assert_eq!((prepared.width, prepared.height), (200, 100));
    }

    /// 긴 변이 상한을 넘으면 비율을 지켜 축소한다.
    #[test]
    fn downscales_long_edge_keeping_ratio() {
        let src = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            2000,
            1000,
            image::Rgb([9, 9, 9]),
        ));
        let (_d, path) = tmp_write("big.png", &png_bytes(&src));
        let prepared = prepare(&path, 1280, None).unwrap();
        assert_eq!((prepared.width, prepared.height), (1280, 640));
    }

    /// T25 — SVG는 긴 변 1280으로 래스터화한다.
    #[test]
    fn t25_svg_is_rasterized() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
             <rect x="0" y="0" width="100" height="50" fill="#00ff00"/>
           </svg>"##;
        let (_d, path) = tmp_write("vec.svg", svg);
        let prepared = prepare(&path, 1280, None).unwrap();
        assert_eq!((prepared.width, prepared.height), (1280, 640));
        assert_eq!(prepared.source_mime, "image/svg+xml");
        let out = image::load_from_memory(&prepared.jpeg).unwrap();
        let px = out.get_pixel(640, 320);
        assert!(px[1] > 200 && px[0] < 80, "초록이어야 한다: {px:?}");
    }

    /// SVG는 배경이 투명하므로 빈 영역이 흰색이 되어야 한다 (T24 규칙과 동일).
    #[test]
    fn svg_transparent_background_becomes_white() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
             <circle cx="50" cy="50" r="10" fill="#000000"/>
           </svg>"##;
        let (_d, path) = tmp_write("circle.svg", svg);
        let prepared = prepare(&path, 256, None).unwrap();
        let out = image::load_from_memory(&prepared.jpeg).unwrap();
        let corner = out.get_pixel(3, 3);
        assert!(corner[0] > 240, "빈 영역이 흰색이어야 한다: {corner:?}");
    }

    /// T25b — 다중 페이지 TIFF는 가장 큰 페이지 하나만 쓴다.
    #[test]
    fn t25b_multipage_tiff_picks_largest() {
        // 첫 장이 작고 둘째 장이 크다 → 둘째 장이 선택되어야 한다.
        let tiff = make_multipage_tiff(&[(32, 24, [255, 0, 0]), (96, 64, [0, 0, 255])]);
        let (_d, path) = tmp_write("multi.tiff", &tiff);
        let prepared = prepare(&path, 1280, None).unwrap();
        assert_eq!((prepared.width, prepared.height), (96, 64));
        let out = image::load_from_memory(&prepared.jpeg).unwrap();
        let px = out.get_pixel(48, 32);
        assert!(
            px[2] > 200 && px[0] < 60,
            "큰 페이지(파랑)여야 한다: {px:?}"
        );
    }

    /// 첫 장이 가장 크면 다시 쓰지 않는다.
    #[test]
    fn tiff_first_page_largest_needs_no_rewrite() {
        let tiff = make_multipage_tiff(&[(96, 64, [0, 0, 255]), (32, 24, [255, 0, 0])]);
        assert!(tiff_pick_largest_page(&tiff).is_none());
        let (_d, path) = tmp_write("multi2.tiff", &tiff);
        let prepared = prepare(&path, 1280, None).unwrap();
        assert_eq!((prepared.width, prepared.height), (96, 64));
    }

    /// T25c — 애니메이션 GIF는 첫 프레임만 쓴다.
    #[test]
    fn t25c_animated_gif_uses_first_frame() {
        let mut buf = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut buf);
            enc.set_repeat(image::codecs::gif::Repeat::Infinite)
                .unwrap();
            for color in [Rgba([255, 0, 0, 255]), Rgba([0, 0, 255, 255])] {
                let frame = RgbaImage::from_pixel(64, 64, color);
                enc.encode_frame(image::Frame::from_parts(
                    frame,
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(100, 1),
                ))
                .unwrap();
            }
        }
        let (_d, path) = tmp_write("anim.gif", &buf);
        let prepared = prepare(&path, 1280, None).unwrap();
        assert_eq!((prepared.width, prepared.height), (64, 64));
        let out = image::load_from_memory(&prepared.jpeg).unwrap();
        let px = out.get_pixel(32, 32);
        assert!(
            px[0] > 180 && px[2] < 80,
            "첫 프레임(빨강)이어야 하는데 {px:?}"
        );
    }

    /// T25d — 알 수 없는 컨테이너라도 내장 JPEG를 찾아낸다. 가장 큰 것을 고른다.
    #[test]
    fn t25d_embedded_preview_jpeg_is_extracted() {
        let small = jpeg_of(32, 32, [255, 0, 0]);
        let large = jpeg_of(160, 120, [0, 200, 0]);
        let mut raw = Vec::new();
        raw.extend_from_slice(&[0u8; 128]); // RAW 헤더 흉내
        raw.extend_from_slice(&small);
        raw.extend_from_slice(&[0x5Au8; 64]);
        raw.extend_from_slice(&large);
        raw.extend_from_slice(&[0u8; 32]);

        let found = extract_embedded_jpeg(&raw).expect("내장 JPEG를 찾아야 한다");
        assert_eq!(found, large, "가장 큰 JPEG를 골라야 한다");

        let (_d, path) = tmp_write("shot.raw", &raw);
        let prepared = prepare(&path, 1280, None).unwrap();
        assert_eq!((prepared.width, prepared.height), (160, 120));
    }

    /// 중첩된 EXIF 썸네일 때문에 프리뷰가 잘리면 안 된다.
    #[test]
    fn embedded_jpeg_survives_nested_thumbnail() {
        let thumb = jpeg_of(16, 16, [0, 0, 255]);
        let outer = jpeg_of(200, 150, [200, 100, 0]);
        // outer 의 SOI 뒤에 APP1 을 끼워 그 안에 thumb 를 통째로 넣는다.
        let mut payload = Vec::new();
        payload.extend_from_slice(b"Exif\0\0");
        payload.extend_from_slice(&thumb);
        let seg_len = (payload.len() + 2) as u16;
        let mut nested = Vec::new();
        nested.extend_from_slice(&outer[..2]);
        nested.extend_from_slice(&[0xFF, 0xE1]);
        nested.extend_from_slice(&seg_len.to_be_bytes());
        nested.extend_from_slice(&payload);
        nested.extend_from_slice(&outer[2..]);

        let found = extract_embedded_jpeg(&nested).expect("찾아야 한다");
        assert_eq!(found.len(), nested.len(), "바깥 JPEG 전체가 나와야 한다");
        let img = image::load_from_memory(&found).unwrap();
        assert_eq!(img.dimensions(), (200, 150));
    }

    #[test]
    fn embedded_jpeg_none_for_garbage() {
        assert!(extract_embedded_jpeg(&[0u8; 512]).is_none());
    }

    /// 디코딩 불가능한 파일은 사유와 함께 거부한다.
    #[test]
    fn undecodable_file_is_rejected_with_reason() {
        let (_d, path) = tmp_write("junk.bin", &[0x11u8; 4096]);
        let msg = match prepare(&path, 1280, None) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("디코딩되면 안 된다"),
        };
        assert!(msg.contains("디코딩하지 못했습니다"), "{msg}");
        assert!(
            msg.contains("ffmpeg"),
            "남은 계층도 사유에 남아야 한다: {msg}"
        );
    }

    /// 계층 4 — `image`도 SVG도 내장 JPEG도 아니면 ffmpeg으로 한 프레임을 뽑는다.
    /// (HEIC·AVIF가 이 경로다. 여기서는 ffmpeg이 만든 영상으로 대신 확인한다.)
    /// `locate`가 `None`이면 건너뛴다. **네트워크를 쓰지 않는다.**
    #[test]
    fn ffmpeg_tier_decodes_what_image_crate_cannot() {
        let Some(tools) = crate::ffmpeg::locate(std::path::Path::new("/nonexistent")) else {
            eprintln!("ffmpeg 없음 — 건너뜀");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("one.mp4");
        crate::ffmpeg::run(
            &tools,
            &[
                "-y",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:size=200x120:duration=1:rate=1",
                "-pix_fmt",
                "yuv420p",
                src.to_str().unwrap(),
            ],
        )
        .unwrap();

        assert!(
            image::load_from_memory(&std::fs::read(&src).unwrap()).is_err(),
            "image 크레이트가 못 읽는 파일이어야 계층 4를 검증한다"
        );
        let prepared = prepare(&src, 1280, Some(&tools)).unwrap();
        assert_eq!((prepared.width, prepared.height), (200, 120));
        let out = image::load_from_memory(&prepared.jpeg).unwrap();
        let px = out.get_pixel(100, 60);
        assert!(px[0] > 150 && px[1] < 100, "빨강이어야 한다: {px:?}");

        // ffmpeg 없이 같은 파일을 주면 사유와 함께 거부한다.
        assert!(prepare(&src, 1280, None).is_err());
    }

    #[test]
    fn data_url_encodes_base64() {
        assert_eq!(to_data_url(b""), "data:image/jpeg;base64,");
        assert_eq!(to_data_url(b"f"), "data:image/jpeg;base64,Zg==");
        assert_eq!(to_data_url(b"fo"), "data:image/jpeg;base64,Zm8=");
        assert_eq!(to_data_url(b"foo"), "data:image/jpeg;base64,Zm9v");
        assert_eq!(to_data_url(b"foobar"), "data:image/jpeg;base64,Zm9vYmFy");
        assert_eq!(
            to_data_url(&[0xFF, 0xD8, 0xFF]),
            "data:image/jpeg;base64,/9j/"
        );
    }

    // ── 픽스처 도우미 ──────────────────────────────────────────────────────

    fn jpeg_of(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    /// JPEG 마커 체인을 훑어 `code` 세그먼트가 있는지 본다.
    fn has_marker(jpeg: &[u8], code: u8) -> bool {
        let mut i = 2usize;
        while i + 3 < jpeg.len() && jpeg[i] == 0xFF {
            let m = jpeg[i + 1];
            if m == code {
                return true;
            }
            if m == 0xDA || m == 0xD9 {
                return false;
            }
            let len = read_be16(jpeg, i + 2).unwrap() as usize;
            i += 2 + len;
        }
        false
    }

    /// 압축 없는 RGB8 다중 페이지 TIFF(리틀엔디언)를 만든다.
    fn make_multipage_tiff(pages: &[(u32, u32, [u8; 3])]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"II");
        out.extend_from_slice(&42u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // IFD0 오프셋은 나중에

        let mut bps_at = Vec::new();
        let mut data_at = Vec::new();
        for (w, h, rgb) in pages {
            bps_at.push(out.len() as u32);
            for _ in 0..3 {
                out.extend_from_slice(&8u16.to_le_bytes());
            }
            data_at.push(out.len() as u32);
            for _ in 0..(*w as usize * *h as usize) {
                out.extend_from_slice(rgb);
            }
            if !out.len().is_multiple_of(2) {
                out.push(0); // TIFF 오프셋은 짝수 정렬
            }
        }

        let ifd_start = out.len();
        let ifd_size = 2 + 10 * 12 + 4;
        for (i, (w, h, _)) in pages.iter().enumerate() {
            let my = ifd_start + i * ifd_size;
            let next = if i + 1 < pages.len() {
                (my + ifd_size) as u32
            } else {
                0
            };
            let mut ifd: Vec<u8> = Vec::new();
            ifd.extend_from_slice(&10u16.to_le_bytes());
            let mut entry = |tag: u16, ty: u16, count: u32, value: u32, short: bool| {
                ifd.extend_from_slice(&tag.to_le_bytes());
                ifd.extend_from_slice(&ty.to_le_bytes());
                ifd.extend_from_slice(&count.to_le_bytes());
                if short && count == 1 {
                    ifd.extend_from_slice(&(value as u16).to_le_bytes());
                    ifd.extend_from_slice(&0u16.to_le_bytes());
                } else {
                    ifd.extend_from_slice(&value.to_le_bytes());
                }
            };
            entry(256, 4, 1, *w, false); // ImageWidth
            entry(257, 4, 1, *h, false); // ImageLength
            entry(258, 3, 3, bps_at[i], false); // BitsPerSample → 별도 영역
            entry(259, 3, 1, 1, true); // Compression = none
            entry(262, 3, 1, 2, true); // Photometric = RGB
            entry(273, 4, 1, data_at[i], false); // StripOffsets
            entry(277, 3, 1, 3, true); // SamplesPerPixel
            entry(278, 4, 1, *h, false); // RowsPerStrip
            entry(279, 4, 1, w * h * 3, false); // StripByteCounts
            entry(284, 3, 1, 1, true); // PlanarConfiguration
            ifd.extend_from_slice(&next.to_le_bytes());
            assert_eq!(ifd.len(), ifd_size);
            out.extend_from_slice(&ifd);
        }
        let start = (ifd_start as u32).to_le_bytes();
        out[4..8].copy_from_slice(&start);
        out
    }
}
