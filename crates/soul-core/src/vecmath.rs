//! 벡터 연산 (§20.3).
//!
//! **float16 고정 길이 블롭으로 저장한다.** 256차원 × 2바이트 = 관측당 512 B,
//! 5,000건이면 2.5 MB로 전량 메모리 상주가 가능하다 (T45).
//! 벡터 색인(HNSW 등)을 도입하지 않는다 — 이 규모에서는 색인 유지 비용이 더 크다.

use half::f16;

/// L2 정규화. 0 벡터는 그대로 돌려준다.
pub fn l2_normalize(v: &mut [f32]) {
    let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

pub fn normalized(v: &[f32]) -> Vec<f32> {
    let mut out = v.to_vec();
    l2_normalize(&mut out);
    out
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

pub fn norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

/// 코사인 유사도. 정규화 여부와 무관하게 올바른 값을 준다.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let na = norm(a);
    let nb = norm(b);
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    (dot(a, b) / (na * nb)).clamp(-1.0, 1.0)
}

/// **§12 전체가 쓰는 거리.** `1 - cosine_similarity`, 범위 `[0, 2]`.
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    1.0 - cosine_similarity(a, b)
}

/// 정규화된 벡터 전용 고속 경로. 호출자가 정규화를 보장해야 한다.
#[inline]
pub fn cosine_distance_normalized(a: &[f32], b: &[f32]) -> f32 {
    (1.0 - dot(a, b)).clamp(0.0, 2.0)
}

/// 원소별 평균. 빈 입력이면 `None`.
pub fn mean(vectors: &[Vec<f32>]) -> Option<Vec<f32>> {
    let first = vectors.first()?;
    let d = first.len();
    let mut acc = vec![0.0f64; d];
    for v in vectors {
        debug_assert_eq!(v.len(), d, "차원이 섞였다");
        for (i, x) in v.iter().enumerate() {
            acc[i] += *x as f64;
        }
    }
    let n = vectors.len() as f64;
    Some(acc.into_iter().map(|x| (x / n) as f32).collect())
}

/// §12.5의 `centroid(X) = normalize(mean(...))`. `|X| < 3` 판정은 호출자 책임이다.
pub fn centroid(vectors: &[Vec<f32>]) -> Option<Vec<f32>> {
    mean(vectors).map(|m| normalized(&m))
}

// ───────────────────────────────────────────────────── f16 블롭 인코딩

/// f32 슬라이스를 little-endian f16 블롭으로 만든다. 길이 = `2 * v.len()`.
pub fn to_f16_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 2);
    for x in v {
        out.extend_from_slice(&f16::from_f32(*x).to_le_bytes());
    }
    out
}

/// f16 블롭을 f32로 되돌린다. 길이가 홀수면 `None`.
pub fn from_f16_blob(b: &[u8]) -> Option<Vec<f32>> {
    if !b.len().is_multiple_of(2) {
        return None;
    }
    Some(
        b.chunks_exact(2)
            .map(|c| f16::from_le_bytes([c[0], c[1]]).to_f32())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_blob_is_two_bytes_per_dim() {
        // T45 — dims=256 이면 관측당 512 B.
        let v = vec![0.5f32; 256];
        assert_eq!(to_f16_blob(&v).len(), 512);
    }

    #[test]
    fn f16_roundtrip_is_close_enough_for_cosine() {
        let v: Vec<f32> = (0..256).map(|i| ((i as f32) * 0.013).sin()).collect();
        let v = normalized(&v);
        let back = from_f16_blob(&to_f16_blob(&v)).unwrap();
        assert!(
            cosine_distance(&v, &back) < 1e-5,
            "f16 손실이 코사인에서 무시할 수준이어야 한다"
        );
    }

    #[test]
    fn cosine_distance_bounds() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let c = vec![-1.0, 0.0];
        assert!((cosine_distance(&a, &a)).abs() < 1e-6);
        assert!((cosine_distance(&a, &b) - 1.0).abs() < 1e-6);
        assert!((cosine_distance(&a, &c) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn centroid_is_unit_length() {
        let vs = vec![vec![1.0, 1.0], vec![1.0, 0.0]];
        let c = centroid(&vs).unwrap();
        assert!((norm(&c) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn zero_vector_is_safe() {
        let z = vec![0.0f32; 4];
        assert_eq!(cosine_similarity(&z, &z), 0.0);
        assert_eq!(normalized(&z), z);
    }
}
