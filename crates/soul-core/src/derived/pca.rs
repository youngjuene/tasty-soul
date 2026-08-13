//! 결정론적 2성분 PCA (§13 화면 6 "구조 보기").
//!
//! 고정 시드가 필요 없다 — 공분산 고유분해는 결정론적이다. 256×256 행렬이라
//! 계산이 즉시 끝난다. 결과는 `derived.sqlite`에 캐시하고 **`T_ref`의 날짜가
//! 바뀔 때만** 무효화한다.
//!
//! ## 구현 지시
//!
//! 외부 선형대수 크레이트를 쓰지 않는다. 거듭제곱법 + 디플레이션으로 상위 2성분만 뽑는다.
//! - 초기 벡터는 정규화된 all-ones 고정값. 난수를 쓰지 않는다.
//! - 반복 200회, 수렴 1e-9.
//! - **부호 규약**: 각 성분에서 절댓값이 가장 큰 좌표가 양수가 되도록 뒤집는다.
//!   이것이 없으면 실행마다 부호가 뒤집혀 T69가 실패한다.

/// 거듭제곱법 최대 반복.
pub const MAX_ITER: usize = 200;
/// 거듭제곱법 수렴 임계.
pub const TOL: f64 = 1e-9;

/// 입력은 ULID 오름차순 임베딩. 반환은 같은 순서의 `(x, y)`.
///
/// `n < 2`이거나 차원이 0이면 좌표를 정하는 데 필요한 분산이 없으므로
/// 빈 벡터(또는 영 좌표)를 돌려준다.
pub fn project2(vectors: &[Vec<f32>]) -> Vec<(f64, f64)> {
    let n = vectors.len();
    if n < 2 {
        return vec![(0.0, 0.0); n];
    }
    let d = vectors[0].len();
    // 차원이 0이거나 섞여 있으면 공분산이 정의되지 않는다.
    if d == 0 || vectors.iter().any(|v| v.len() != d) {
        return vec![(0.0, 0.0); n];
    }

    // 1. 평균 제거. 누산은 f64로 하고 순서를 고정한다.
    let mut mean = vec![0.0f64; d];
    for v in vectors {
        for (t, x) in v.iter().enumerate() {
            mean[t] += *x as f64;
        }
    }
    let inv_n = 1.0 / n as f64;
    for m in mean.iter_mut() {
        *m *= inv_n;
    }
    let centered: Vec<Vec<f64>> = vectors
        .iter()
        .map(|v| v.iter().zip(&mean).map(|(x, m)| *x as f64 - m).collect())
        .collect();

    // 2. 공분산 (d×d, 대칭). n-1로 나눈다 — 고유벡터는 스케일과 무관하지만
    //    디플레이션에 쓰는 고유값과 단위를 맞춘다.
    let mut cov = vec![0.0f64; d * d];
    for row in &centered {
        for a in 0..d {
            let ra = row[a];
            for b in a..d {
                cov[a * d + b] += ra * row[b];
            }
        }
    }
    let inv = 1.0 / (n - 1) as f64;
    for a in 0..d {
        for b in a..d {
            let v = cov[a * d + b] * inv;
            cov[a * d + b] = v;
            cov[b * d + a] = v;
        }
    }

    // 3. 제1성분 — 정규화된 all-ones에서 출발하는 거듭제곱법.
    let mut v1 = power_iteration(&cov, d, ones(d), None);
    fix_sign(&mut v1);

    // 4. 디플레이션 후 제2성분. 매 반복 v1 방향을 제거해 수치 누수를 막는다.
    let lambda1 = rayleigh(&cov, d, &v1);
    let mut cov2 = cov;
    for a in 0..d {
        let ca = lambda1 * v1[a];
        for b in 0..d {
            cov2[a * d + b] -= ca * v1[b];
        }
    }
    let start2 = orthogonal_start(d, &v1);
    let mut v2 = power_iteration(&cov2, d, start2, Some(&v1));
    fix_sign(&mut v2);

    // 5. 투영.
    centered
        .iter()
        .map(|row| (dot(row, &v1), dot(row, &v2)))
        .collect()
}

/// 정규화된 all-ones. **고정값이며 난수를 쓰지 않는다.**
fn ones(d: usize) -> Vec<f64> {
    vec![1.0 / (d as f64).sqrt(); d]
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut acc = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        acc += x * y;
    }
    acc
}

/// 거듭제곱법. `ortho`가 주어지면 매 반복 그 방향을 제거한다.
/// 고유값이 0이면(더 뽑을 방향이 없으면) 직전 벡터를 그대로 돌려준다.
fn power_iteration(mat: &[f64], d: usize, start: Vec<f64>, ortho: Option<&[f64]>) -> Vec<f64> {
    let mut v = start;
    for _ in 0..MAX_ITER {
        let mut w = vec![0.0f64; d];
        for (a, wa) in w.iter_mut().enumerate() {
            *wa = dot(&mat[a * d..(a + 1) * d], &v);
        }
        if let Some(u) = ortho {
            let p = dot(&w, u);
            for (x, y) in w.iter_mut().zip(u.iter()) {
                *x -= p * y;
            }
        }
        let norm = dot(&w, &w).sqrt();
        if norm <= 0.0 {
            return v;
        }
        for x in w.iter_mut() {
            *x /= norm;
        }
        let mut diff = 0.0f64;
        for (x, y) in w.iter().zip(v.iter()) {
            let e = x - y;
            diff += e * e;
        }
        v = w;
        if diff.sqrt() < TOL {
            break;
        }
    }
    v
}

/// 레일리 몫 `vᵀ C v` — 단위벡터 `v`에 대한 고유값 추정.
fn rayleigh(mat: &[f64], d: usize, v: &[f64]) -> f64 {
    let mut acc = 0.0f64;
    for a in 0..d {
        acc += v[a] * dot(&mat[a * d..(a + 1) * d], v);
    }
    acc
}

/// 제2성분의 초기 벡터. all-ones에서 `v1` 성분을 뺀 것을 쓴다.
/// all-ones가 `v1`과 평행하면 표준기저를 인덱스 순으로 훑어 결정론적으로 고른다.
fn orthogonal_start(d: usize, v1: &[f64]) -> Vec<f64> {
    if let Some(v) = project_out(ones(d), v1) {
        return v;
    }
    for i in 0..d {
        let mut e = vec![0.0f64; d];
        e[i] = 1.0;
        if let Some(v) = project_out(e, v1) {
            return v;
        }
    }
    vec![0.0f64; d]
}

/// `u` 방향을 제거하고 정규화한다. 남는 길이가 없으면 `None`.
fn project_out(mut v: Vec<f64>, u: &[f64]) -> Option<Vec<f64>> {
    let p = dot(&v, u);
    for (x, y) in v.iter_mut().zip(u.iter()) {
        *x -= p * y;
    }
    let norm = dot(&v, &v).sqrt();
    if norm <= 1e-12 {
        return None;
    }
    for x in v.iter_mut() {
        *x /= norm;
    }
    Some(v)
}

/// **부호 규약** — 절댓값이 가장 큰 좌표가 양수가 되도록 성분 전체를 뒤집는다.
/// 동률이면 인덱스가 작은 좌표를 기준으로 삼는다. 이것이 T69의 전부다.
fn fix_sign(v: &mut [f64]) {
    let mut idx = 0usize;
    let mut best = 0.0f64;
    for (i, x) in v.iter().enumerate() {
        let a = x.abs();
        if a > best {
            best = a;
            idx = i;
        }
    }
    if !v.is_empty() && v[idx] < 0.0 {
        for x in v.iter_mut() {
            *x = -*x;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `dir` 방향으로 `ts`만큼 늘어선 점들. 두 번째 방향으로 아주 조금만 흔든다.
    fn along(dir: &[f32], ts: &[f32]) -> Vec<Vec<f32>> {
        ts.iter()
            .enumerate()
            .map(|(i, t)| {
                let mut v: Vec<f32> = dir.iter().map(|x| x * t).collect();
                // 제2성분이 완전히 퇴화하지 않도록 미세한 직교 흔들림을 넣는다.
                let j = 1 % v.len();
                v[j] += if i % 2 == 0 { 0.01 } else { -0.01 };
                v
            })
            .collect()
    }

    #[test]
    fn projection_repeats_identically() {
        // T69 — PCA 투영을 2회 실행 → 좌표 동일.
        let vs = along(&[0.3, -0.8, 0.5, 0.1], &[-2.0, -1.0, 0.5, 1.0, 2.5, 3.0]);
        let a = project2(&vs);
        let b = project2(&vs);
        assert_eq!(a, b, "같은 입력에서 좌표가 달라졌다");
        assert_eq!(a.len(), vs.len());
    }

    #[test]
    fn degenerate_inputs_are_safe() {
        assert!(project2(&[]).is_empty());
        assert_eq!(project2(&[vec![1.0, 2.0]]), vec![(0.0, 0.0)]);
        // 차원 0
        assert_eq!(project2(&vec![Vec::<f32>::new(); 3]), vec![(0.0, 0.0); 3]);
        // 차원 섞임
        let mixed = vec![vec![1.0f32, 0.0], vec![0.0]];
        assert_eq!(project2(&mixed), vec![(0.0, 0.0); 2]);
        // 모든 점이 동일 → 분산 0. 크래시 없이 원점.
        let same = vec![vec![1.0f32, 2.0, 3.0]; 4];
        for (x, y) in project2(&same) {
            assert!(x.abs() < 1e-9 && y.abs() < 1e-9);
        }
    }

    #[test]
    fn first_component_captures_the_spread() {
        // x축으로만 퍼진 데이터 → 제1성분이 x축, 좌표가 원 값과 일치한다.
        let vs = vec![
            vec![-2.0f32, 0.0],
            vec![-1.0, 0.0],
            vec![1.0, 0.0],
            vec![2.0, 0.0],
        ];
        let out = project2(&vs);
        let xs: Vec<f64> = out.iter().map(|p| p.0).collect();
        let ys: Vec<f64> = out.iter().map(|p| p.1).collect();
        for (got, want) in xs.iter().zip([-2.0, -1.0, 1.0, 2.0]) {
            assert!((got - want).abs() < 1e-9, "{got} != {want}");
        }
        assert!(ys.iter().all(|y| y.abs() < 1e-9), "제2성분에 분산이 없다");
    }

    #[test]
    fn sign_convention_flips_negative_dominant_axis() {
        // 주방향의 절댓값 최대 좌표가 음수(-0.8)인 데이터.
        // 규약이 없으면 제1성분이 (-0.8, 0.45, 0.4)로 나와 첫 점의 x가 음수가 된다.
        let dir = [-0.8f32, 0.45, 0.4];
        let vs = along(&dir, &[-2.0, -1.0, 1.0, 2.0]);
        let out = project2(&vs);
        assert!(out[0].0 > 0.0, "부호 규약이 적용되지 않았다: {:?}", out[0]);
        assert!(out[3].0 < 0.0);
        // 규약은 결정론의 근거이므로 재실행에서도 같아야 한다.
        assert_eq!(out, project2(&vs));
    }

    #[test]
    fn components_are_ordered_by_variance() {
        // x로 크게, y로 작게 퍼진 데이터.
        let vs = vec![
            vec![-4.0f32, -0.2],
            vec![-2.0, 0.2],
            vec![2.0, -0.2],
            vec![4.0, 0.2],
        ];
        let out = project2(&vs);
        let var = |f: fn(&(f64, f64)) -> f64| -> f64 {
            let m: f64 = out.iter().map(f).sum::<f64>() / out.len() as f64;
            out.iter().map(|p| (f(p) - m).powi(2)).sum::<f64>()
        };
        assert!(
            var(|p| p.0) > var(|p| p.1) * 10.0,
            "제1성분이 제2성분보다 분산이 커야 한다"
        );
    }

    #[test]
    fn components_are_orthogonal_in_the_projection() {
        let vs = along(&[0.6, 0.3, -0.7, 0.2], &[-3.0, -1.0, 0.0, 1.0, 2.0, 4.0]);
        let out = project2(&vs);
        // 중심화된 데이터의 두 성분 좌표는 상관이 0이어야 한다.
        let cov: f64 = out.iter().map(|p| p.0 * p.1).sum();
        let scale: f64 = out.iter().map(|p| p.0 * p.0).sum::<f64>().sqrt();
        assert!(
            cov.abs() < 1e-6 * scale.max(1.0),
            "성분이 직교하지 않는다: {cov}"
        );
    }
}
