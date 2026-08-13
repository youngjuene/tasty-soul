//! 군집 (§12.3) — k-means++ **고정 시드 42** (§R5).
//!
//! ```text
//! cluster(S):
//!   n = |S|
//!   n < 4 이면 → null (군집 없음)
//!   k = clamp(round(sqrt(n / 2)), 2, 8)
//!   §R5의 고정 시드 k-means++ 실행
//!   반환: 중심 벡터 k개 + 각 원소의 배정
//! ```
//!
//! §R5: k-means++, 고정 시드 `42`, 최대 반복 100, 수렴 임계 1e-6.
//! 입력은 **ULID 순으로 정렬된 임베딩 배열**. 거리는 코사인.
//!
//! ## 결정론 요구 (T12)
//!
//! 난수원은 이 파일 안에 있는 `Lcg`뿐이다. `rand` 크레이트를 쓰지 않는다 —
//! 버전이 바뀌면 같은 시드에서 다른 수열이 나와 과거 군집이 재현되지 않는다.
//! 부동소수 누산 순서도 결정론의 일부다. 병렬화하지 말 것.

use crate::vecmath::normalized;

/// §R5의 시드.
pub const SEED: u64 = 42;
pub const MAX_ITER: usize = 100;
pub const TOL: f64 = 1e-6;

/// 결정론적 난수원. **알고리즘을 바꾸면 과거 군집이 재현되지 않는다.**
/// splitmix64 — 상태 전이와 출력이 명세로 고정되어 있고 외부 크레이트에 의존하지 않는다.
pub struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Lcg {
        Lcg(seed)
    }
    /// splitmix64.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// `[0,1)` 균등.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// `[0,n)` 균등.
    pub fn next_below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_f64() * n as f64) as usize % n
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clustering {
    /// k개 중심 벡터. **L2 정규화되어 있다.**
    pub centroids: Vec<Vec<f32>>,
    /// 입력 순서대로의 군집 배정.
    pub assignment: Vec<usize>,
    pub k: usize,
}

/// 두 **정규화된** 벡터의 코사인 거리. 누산은 항상 f64다 —
/// f32로 누산하면 덧셈 순서에 따라 마지막 자리가 흔들려 배정이 갈릴 수 있다 (§R5).
fn cos_dist_norm(a: &[f32], b: &[f32]) -> f64 {
    let mut acc = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        acc += (*x as f64) * (*y as f64);
    }
    (1.0 - acc).clamp(0.0, 2.0)
}

/// §12.3의 `cluster(S)`. 입력은 **ULID 오름차순으로 정렬된** 임베딩이어야 한다.
/// `n < 4`이면 `None`.
///
/// 결정론(T12)을 위해 병렬화하지 않는다. 모든 순회 순서가 결과의 일부다.
pub fn cluster(vectors: &[Vec<f32>]) -> Option<Clustering> {
    let n = vectors.len();
    if n < 4 {
        return None;
    }
    // 차원이 0이거나 섞여 있으면 코사인이 정의되지 않는다 — 군집 없음으로 낮춘다.
    let dim = vectors[0].len();
    if dim == 0 || vectors.iter().any(|v| v.len() != dim) {
        return None;
    }

    // 거리가 코사인이므로 한 번만 정규화하고 이후에는 내적만 쓴다.
    let pts: Vec<Vec<f32>> = vectors.iter().map(|v| normalized(v)).collect();
    let k = choose_k(n).min(n);

    let mut rng = Lcg::new(SEED);
    let mut centroids = kmeanspp_init(&pts, k, &mut rng);
    let mut assignment = vec![0usize; n];

    for _ in 0..MAX_ITER {
        assign_all(&pts, &centroids, &mut assignment);
        refill_empty(&pts, &centroids, &mut assignment, k);
        let next = update_centroids(&pts, &assignment, &centroids, k);
        let shift = max_shift(&centroids, &next);
        centroids = next;
        if shift < TOL {
            break;
        }
    }

    // `assignment`은 마지막 배정 단계의 값이다(표준 Lloyd). 중심이 TOL 미만으로만
    // 움직였으므로 반환 중심과 실질적으로 일치하며, 빈 군집이 없음이 보장된다.
    Some(Clustering {
        centroids,
        assignment,
        k,
    })
}

/// k-means++ 초기 중심 선택. 난수원은 `Lcg`(splitmix64, 시드 42)뿐이다.
///
/// 첫 중심은 `next_below(n)`, 이후 중심은 D²(= 최근접 중심까지 코사인 거리의 제곱)
/// 가중 표집이다. 누적합과 `next_f64() * total`을 쓴다.
fn kmeanspp_init(pts: &[Vec<f32>], k: usize, rng: &mut Lcg) -> Vec<Vec<f32>> {
    let n = pts.len();
    let mut chosen: Vec<usize> = Vec::with_capacity(k);
    let first = rng.next_below(n);
    chosen.push(first);

    // d2[i] = 이미 고른 중심들까지의 최소 거리²
    let mut d2 = vec![0.0f64; n];
    for (i, p) in pts.iter().enumerate() {
        let d = cos_dist_norm(p, &pts[first]);
        d2[i] = d * d;
    }

    while chosen.len() < k {
        let total: f64 = d2.iter().sum();
        // total이 0이어도(모든 점이 동일) 난수는 항상 소비한다 — 수열이 어긋나면
        // 같은 입력에서 다른 결과가 나온다.
        let r = rng.next_f64() * total;
        let mut acc = 0.0f64;
        let mut pick = n - 1; // total == 0 이거나 반올림으로 못 넘길 때의 결정론적 귀결
        for (i, w) in d2.iter().enumerate() {
            acc += *w;
            if acc > r {
                pick = i;
                break;
            }
        }
        chosen.push(pick);
        for (i, p) in pts.iter().enumerate() {
            let d = cos_dist_norm(p, &pts[pick]);
            let dd = d * d;
            if dd < d2[i] {
                d2[i] = dd;
            }
        }
    }

    chosen.into_iter().map(|i| pts[i].clone()).collect()
}

/// 각 점을 최근접 중심에 배정한다. 거리가 같으면 **인덱스가 작은 중심**이 이긴다.
fn assign_all(pts: &[Vec<f32>], centroids: &[Vec<f32>], out: &mut [usize]) {
    for (i, p) in pts.iter().enumerate() {
        let mut best = 0usize;
        let mut best_d = f64::INFINITY;
        for (j, c) in centroids.iter().enumerate() {
            let d = cos_dist_norm(p, c);
            if d < best_d {
                best_d = d;
                best = j;
            }
        }
        out[i] = best;
    }
}

/// 빈 군집이 생기면 **가장 먼 점**을 그 군집으로 옮긴다.
/// 최원거리 점이 여럿이면 인덱스가 가장 작은 것을 쓴다 (결정론).
/// 옮긴 자리가 다시 비면 안 되므로 크기가 2 이상인 군집의 점만 후보다.
fn refill_empty(pts: &[Vec<f32>], centroids: &[Vec<f32>], assignment: &mut [usize], k: usize) {
    let mut sizes = vec![0usize; k];
    for &a in assignment.iter() {
        sizes[a] += 1;
    }
    for j in 0..k {
        if sizes[j] > 0 {
            continue;
        }
        let mut pick: Option<usize> = None;
        let mut best_d = f64::NEG_INFINITY;
        for (i, p) in pts.iter().enumerate() {
            let c = assignment[i];
            if sizes[c] < 2 {
                continue;
            }
            let d = cos_dist_norm(p, &centroids[c]);
            // 동률이면 먼저 만난 점(= 인덱스가 작은 점)을 유지한다.
            if d > best_d {
                best_d = d;
                pick = Some(i);
            }
        }
        if let Some(i) = pick {
            sizes[assignment[i]] -= 1;
            assignment[i] = j;
            sizes[j] = 1;
        }
    }
}

/// 중심 갱신 = 배정된 점들의 평균 후 L2 정규화. 누산은 f64.
fn update_centroids(
    pts: &[Vec<f32>],
    assignment: &[usize],
    prev: &[Vec<f32>],
    k: usize,
) -> Vec<Vec<f32>> {
    let dim = pts[0].len();
    let mut acc = vec![vec![0.0f64; dim]; k];
    let mut cnt = vec![0usize; k];
    for (i, p) in pts.iter().enumerate() {
        let j = assignment[i];
        cnt[j] += 1;
        for (t, x) in p.iter().enumerate() {
            acc[j][t] += *x as f64;
        }
    }

    let mut out = Vec::with_capacity(k);
    for (j, sums) in acc.into_iter().enumerate() {
        if cnt[j] == 0 {
            // `refill_empty`가 통상 막는다. 그래도 남으면 이전 중심을 유지한다.
            out.push(prev[j].clone());
            continue;
        }
        let inv = 1.0 / cnt[j] as f64;
        let mut v: Vec<f64> = sums.into_iter().map(|x| x * inv).collect();
        let norm = v.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm > 0.0 {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
        out.push(v.into_iter().map(|x| x as f32).collect());
    }
    out
}

/// 중심 이동 최대치. 중심은 모두 단위벡터이므로 유클리드 거리로 잰다.
fn max_shift(a: &[Vec<f32>], b: &[Vec<f32>]) -> f64 {
    let mut m = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let mut s = 0.0f64;
        for (p, q) in x.iter().zip(y.iter()) {
            let diff = (*p as f64) - (*q as f64);
            s += diff * diff;
        }
        let s = s.sqrt();
        if s > m {
            m = s;
        }
    }
    m
}

/// `k = clamp(round(sqrt(n / 2)), 2, 8)`.
pub fn choose_k(n: usize) -> usize {
    let k = ((n as f64) / 2.0).sqrt().round() as i64;
    k.clamp(2, 8) as usize
}

/// 실루엣 계수 평균 (코사인 거리). §12.5의 `crystal`.
/// 군집이 1개거나 표본이 2건 미만이면 `None`.
///
/// 표준 정의를 따른다:
/// - `a(i)` = 자기 군집의 **다른** 점들까지의 평균 거리
/// - `b(i)` = 다른 군집들 중 평균 거리가 가장 작은 값
/// - `s(i) = (b - a) / max(a, b)`, 단 **자기 군집 크기가 1이면 `s(i) = 0`**
pub fn silhouette(vectors: &[Vec<f32>], assignment: &[usize], k: usize) -> Option<f64> {
    let n = vectors.len();
    if k < 2 || n < 2 || assignment.len() != n {
        return None;
    }
    if assignment.iter().any(|&a| a >= k) {
        return None;
    }
    let dim = vectors[0].len();
    if dim == 0 || vectors.iter().any(|v| v.len() != dim) {
        return None;
    }

    let mut sizes = vec![0usize; k];
    for &a in assignment {
        sizes[a] += 1;
    }
    // 실제로 채워진 군집이 1개뿐이면 `b(i)`가 정의되지 않는다.
    if sizes.iter().filter(|&&s| s > 0).count() < 2 {
        return None;
    }

    let pts: Vec<Vec<f32>> = vectors.iter().map(|v| normalized(v)).collect();

    // sums[i * k + j] = 점 i에서 군집 j의 모든 점까지의 거리 합.
    // 쌍마다 한 번만 재고 양쪽에 더한다 — O(n²/2)이지만 순서는 고정이다.
    let mut sums = vec![0.0f64; n * k];
    for i in 0..n {
        for j in (i + 1)..n {
            let d = cos_dist_norm(&pts[i], &pts[j]);
            sums[i * k + assignment[j]] += d;
            sums[j * k + assignment[i]] += d;
        }
    }

    let mut total = 0.0f64;
    for i in 0..n {
        let own = assignment[i];
        if sizes[own] < 2 {
            continue; // s(i) = 0 을 그대로 더하는 것과 같다
        }
        let a = sums[i * k + own] / (sizes[own] - 1) as f64;
        let mut b = f64::INFINITY;
        for j in 0..k {
            if j == own || sizes[j] == 0 {
                continue;
            }
            let m = sums[i * k + j] / sizes[j] as f64;
            if m < b {
                b = m;
            }
        }
        if !b.is_finite() {
            continue;
        }
        let denom = if a > b { a } else { b };
        if denom > 0.0 {
            total += (b - a) / denom;
        }
    }
    Some(total / n as f64)
}

/// §12.5.1 — 실루엣의 O(n²) 회피. **난수를 쓰지 않는다.**
///
/// ```text
/// sample(S):
///   |S| ≤ max 이면 S 그대로
///   아니면 ULID 순 정렬 후 stride = ceil(|S| / max)로 균등 추출
/// ```
///
/// 고정 stride 추출이므로 재실행해도 같은 표본이 나온다 (T41).
pub fn stride_sample<T: Clone>(items: &[T], max: usize) -> Vec<T> {
    if max == 0 || items.len() <= max {
        return items.to_vec();
    }
    let stride = items.len().div_ceil(max);
    items.iter().step_by(stride).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn k_matches_spec_formula() {
        assert_eq!(choose_k(4), 2);
        assert_eq!(choose_k(8), 2);
        assert_eq!(choose_k(50), 5);
        assert_eq!(choose_k(200), 8);
        assert_eq!(choose_k(10_000), 8);
    }

    #[test]
    fn stride_sample_is_deterministic_and_bounded() {
        let items: Vec<usize> = (0..1000).collect();
        let a = stride_sample(&items, 500);
        let b = stride_sample(&items, 500);
        assert_eq!(a, b);
        assert!(a.len() <= 500);
        assert_eq!(a[0], 0);
        assert_eq!(a[1], 2, "stride = ceil(1000/500) = 2");
    }

    #[test]
    fn lcg_is_stable_across_runs() {
        let mut a = Lcg::new(SEED);
        let mut b = Lcg::new(SEED);
        let xs: Vec<u64> = (0..5).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..5).map(|_| b.next_u64()).collect();
        assert_eq!(xs, ys);
        assert!(xs.windows(2).all(|w| w[0] != w[1]));
    }

    // ─────────────────────────────────────────────── 픽스처

    const DIM: usize = 8;

    /// `center` 축 주변에 흩어진 점 `n`개. 난수원은 `Lcg`뿐이므로 픽스처도 결정론적이다.
    fn blob(seed: u64, center: usize, n: usize, jitter: f32) -> Vec<Vec<f32>> {
        let mut rng = Lcg::new(seed);
        (0..n)
            .map(|_| {
                let mut v = vec![0.0f32; DIM];
                for (t, x) in v.iter_mut().enumerate() {
                    *x = jitter * (rng.next_f64() as f32 - 0.5);
                    if t == center {
                        *x += 1.0;
                    }
                }
                v
            })
            .collect()
    }

    /// 잘 분리된 3덩어리 (덩어리당 6건 → n = 18 → k = 3).
    fn three_blobs() -> Vec<Vec<f32>> {
        let mut v = blob(1, 0, 6, 0.05);
        v.extend(blob(2, 1, 6, 0.05));
        v.extend(blob(3, 2, 6, 0.05));
        v
    }

    // ─────────────────────────────────────────────── cluster

    #[test]
    fn cluster_is_none_below_four() {
        assert!(cluster(&[]).is_none());
        assert!(cluster(&blob(9, 0, 3, 0.1)).is_none());
        assert!(cluster(&blob(9, 0, 4, 0.1)).is_some());
    }

    #[test]
    fn cluster_repeats_identically() {
        // T12 — 동일 픽스처로 k-means 2회 → 군집 배정 동일.
        let vs = {
            let mut v = three_blobs();
            v.extend(blob(4, 3, 8, 0.3));
            v.extend(blob(5, 4, 8, 0.3));
            v
        };
        let a = cluster(&vs).expect("n >= 4");
        let b = cluster(&vs).expect("n >= 4");
        assert_eq!(a.assignment, b.assignment);
        assert_eq!(a.centroids, b.centroids);
        assert_eq!(a.k, b.k);
        assert_eq!(a.k, choose_k(vs.len()));
        assert_eq!(a.centroids.len(), a.k);
        assert_eq!(a.assignment.len(), vs.len());
        assert!(a.assignment.iter().all(|&j| j < a.k));
    }

    #[test]
    fn centroids_are_unit_length() {
        let c = cluster(&three_blobs()).expect("n >= 4");
        for cen in &c.centroids {
            let n: f32 = cen.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((n - 1.0).abs() < 1e-5, "중심은 L2 정규화되어야 한다: {n}");
        }
    }

    #[test]
    fn well_separated_blobs_land_in_one_cluster_each() {
        let vs = three_blobs();
        let c = cluster(&vs).expect("n >= 4");
        assert_eq!(c.k, 3);
        for chunk in 0..3 {
            let seg = &c.assignment[chunk * 6..(chunk + 1) * 6];
            assert!(
                seg.windows(2).all(|w| w[0] == w[1]),
                "같은 덩어리는 같은 군집이어야 한다: {seg:?}"
            );
        }
        // 세 덩어리가 서로 다른 군집이어야 한다.
        let mut labels = vec![c.assignment[0], c.assignment[6], c.assignment[12]];
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), 3);
    }

    #[test]
    fn no_cluster_is_left_empty() {
        // 모든 점이 동일하면 k-means++가 같은 중심을 중복 선택한다.
        // `refill_empty`가 가장 먼(= 인덱스가 가장 작은) 점을 옮겨 빈 군집을 없앤다.
        let vs = vec![vec![1.0f32, 0.0, 0.0, 0.0]; 6];
        let c = cluster(&vs).expect("n >= 4");
        let mut sizes = vec![0usize; c.k];
        for &a in &c.assignment {
            sizes[a] += 1;
        }
        assert!(sizes.iter().all(|&s| s > 0), "빈 군집이 남았다: {sizes:?}");
        assert_eq!(sizes.iter().sum::<usize>(), 6);
    }

    #[test]
    fn mixed_dimensions_are_rejected() {
        let vs = vec![vec![1.0f32, 0.0], vec![0.0, 1.0], vec![1.0], vec![0.0, 1.0]];
        assert!(cluster(&vs).is_none());
        assert!(cluster(&vec![Vec::<f32>::new(); 5]).is_none());
    }

    // ─────────────────────────────────────────────── silhouette

    #[test]
    fn silhouette_is_none_when_undefined() {
        let vs = three_blobs();
        let c = cluster(&vs).expect("n >= 4");
        assert!(silhouette(&vs, &c.assignment, 1).is_none(), "k < 2");
        assert!(
            silhouette(&vs[..1], &c.assignment[..1], 3).is_none(),
            "표본 2건 미만"
        );
        assert!(
            silhouette(&vs, &vec![0usize; vs.len()], 3).is_none(),
            "채워진 군집이 1개"
        );
        assert!(
            silhouette(&vs, &c.assignment[..3], 3).is_none(),
            "길이 불일치"
        );
    }

    #[test]
    fn silhouette_is_high_for_well_separated_blobs() {
        let vs = three_blobs();
        let c = cluster(&vs).expect("n >= 4");
        let s = silhouette(&vs, &c.assignment, c.k).expect("k >= 2");
        assert!(s > 0.8, "잘 분리된 3덩어리의 실루엣이 낮다: {s}");
        assert!(s <= 1.0);
    }

    #[test]
    fn silhouette_is_low_when_clusters_overlap() {
        // 한 덩어리를 억지로 둘로 쪼개면 실루엣이 잘 분리된 경우보다 크게 떨어진다.
        let vs = blob(7, 0, 12, 0.4);
        let mut assignment = vec![0usize; 12];
        for (i, a) in assignment.iter_mut().enumerate() {
            *a = i % 2;
        }
        let s = silhouette(&vs, &assignment, 2).expect("k >= 2");
        assert!(s < 0.3, "겹친 군집의 실루엣이 너무 높다: {s}");
    }

    #[test]
    fn singleton_cluster_counts_as_zero() {
        // 표준 정의: 자기 군집 크기가 1인 점의 실루엣은 0이다.
        // p0·p1은 동일하고 p2만 직교 → s = (1 + 1 + 0) / 3.
        let vs = vec![vec![1.0f32, 0.0], vec![1.0, 0.0], vec![0.0, 1.0]];
        let s = silhouette(&vs, &[0, 0, 1], 2).expect("k >= 2");
        assert!((s - 2.0 / 3.0).abs() < 1e-9, "{s}");
    }

    #[test]
    fn silhouette_repeats_identically() {
        let vs = three_blobs();
        let c = cluster(&vs).expect("n >= 4");
        let a = silhouette(&vs, &c.assignment, c.k);
        let b = silhouette(&vs, &c.assignment, c.k);
        assert_eq!(a, b);
    }
}
