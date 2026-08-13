//! surprisal (§12.4) — **투입 시점에만 계산하고 이후 동결한다** (§R4).
//!
//! ```text
//! P = 이 투입 직전까지의 ingest 집합 (supersede된 것 제외)
//! C = cluster(P)
//!
//! C == null 이면:              min_dist = null
//! 그 외:                        min_dist = min over c ∈ C.중심 의 cosine_distance(e_new, c)
//!
//! D = P의 min_dist 중 null이 아닌 값들
//! |D| < 10 이면:               surprisal = 1.0
//! 그 외:                        surprisal = |{d ∈ D : d < min_dist}| / |D|
//! ```
//!
//! `min_dist`와 `surprisal`은 **독립적으로 결정된다.** `min_dist`가 `null`이어도
//! `surprisal`은 `1.0`으로 정해진다. **`|D| = 0`일 때 나눗셈을 수행하지 않는다** (T18).
//!
//! §12.3의 캐시 규칙: 캐시 생성 시점보다 관측이 **10% 또는 20건 이상** 늘었을 때만
//! 재계산한다. 그 사이의 투입은 캐시된 중심 벡터로 `min_dist`를 잰다 (T46).
//! 이 근사가 허용되는 이유는 두 값이 어차피 동결되기 때문이다.

use super::cluster::Clustering;
use crate::vecmath::cosine_distance;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surprisal {
    pub min_dist: Option<f64>,
    pub surprisal: f64,
}

/// `centroids`가 `None`이면 `min_dist = null`. `prior_min_dists`는 `P`의 null 아닌 min_dist들.
pub fn compute(
    new_vec: &[f32],
    centroids: Option<&Clustering>,
    prior_min_dists: &[f64],
) -> Surprisal {
    // ── min_dist: 군집이 없으면 null. 있으면 중심들까지의 최소 코사인 거리.
    let mut min_dist: Option<f64> = None;
    if let Some(c) = centroids {
        for cen in &c.centroids {
            let d = cosine_distance(new_vec, cen) as f64;
            min_dist = match min_dist {
                Some(m) if m <= d => Some(m),
                _ => Some(d),
            };
        }
    }

    // ── surprisal: min_dist와 **독립적으로** 정해진다.
    let total = prior_min_dists.len();
    let surprisal = if total < 10 {
        // |D| = 0 을 포함한다. 여기서 나눗셈을 하지 않는 것이 T18의 전부다.
        1.0
    } else {
        match min_dist {
            Some(md) => {
                let below = prior_min_dists.iter().filter(|d| **d < md).count();
                below as f64 / total as f64
            }
            // min_dist가 null이라는 것은 `cluster(P) == null`, 즉 |P| < 4라는 뜻이고
            // D ⊆ P이므로 |D| >= 10과 동시에 성립할 수 없다. 방어적으로 null 경로와
            // 같은 1.0을 쓴다 — 비교할 기준이 없으면 "전부 이것보다 가깝다"고 본다.
            None => 1.0,
        }
    };

    Surprisal {
        min_dist,
        surprisal,
    }
}

/// §12.3 캐시 갱신 판정. `cached_n`은 캐시를 만든 시점의 관측 수.
pub fn should_recluster(cached_n: usize, current_n: usize) -> bool {
    if cached_n == 0 {
        return true;
    }
    let grown = current_n.saturating_sub(cached_n);
    grown >= 20 || (grown as f64) >= (cached_n as f64) * 0.10
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clustering(centroids: Vec<Vec<f32>>) -> Clustering {
        let k = centroids.len();
        Clustering {
            centroids,
            assignment: vec![0; k],
            k,
        }
    }

    #[test]
    fn no_centroids_means_null_min_dist() {
        // C == null → min_dist = null. surprisal은 그와 무관하게 1.0.
        let s = compute(&[1.0, 0.0], None, &[]);
        assert_eq!(s.min_dist, None);
        assert_eq!(s.surprisal, 1.0);
    }

    #[test]
    fn empty_d_does_not_divide() {
        // T18 — |D| = 0 에서 크래시 없이 surprisal = 1.0.
        let c = clustering(vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
        let s = compute(&[1.0, 0.0], Some(&c), &[]);
        assert_eq!(s.surprisal, 1.0);
        assert!(s.surprisal.is_finite(), "0으로 나눈 흔적이 있다");
        // min_dist는 독립적으로 정해진다.
        assert!(s.min_dist.expect("중심이 있다").abs() < 1e-6);
    }

    #[test]
    fn fewer_than_ten_priors_is_always_one() {
        let c = clustering(vec![vec![1.0, 0.0]]);
        let nine: Vec<f64> = (0..9).map(|i| i as f64 * 0.1).collect();
        assert_eq!(nine.len(), 9);
        assert_eq!(compute(&[0.0, 1.0], Some(&c), &nine).surprisal, 1.0);
    }

    #[test]
    fn ten_or_more_priors_uses_the_ratio() {
        // 중심 (1,0)에서 새 벡터까지의 거리 = 1 - cos = 1.0.
        let c = clustering(vec![vec![1.0, 0.0]]);
        let d: Vec<f64> = vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.7, 0.8, 0.9, 1.5, 1.6];
        let s = compute(&[0.0, 1.0], Some(&c), &d);
        assert!((s.min_dist.expect("중심이 있다") - 1.0).abs() < 1e-6);
        // 1.0보다 작은 값이 8개 / 10.
        assert!((s.surprisal - 0.8).abs() < 1e-12, "{}", s.surprisal);
    }

    #[test]
    fn min_dist_is_the_nearest_centroid() {
        let c = clustering(vec![
            vec![0.0, 1.0],  // 거리 1.0
            vec![-1.0, 0.0], // 거리 2.0
            vec![1.0, 1.0],  // 거리 1 - 0.7071 = 0.2929 ← 최소
        ]);
        let s = compute(&[1.0, 0.0], Some(&c), &[]);
        let got = s.min_dist.expect("중심이 있다");
        assert!((got - (1.0 - 0.5f64.sqrt())).abs() < 1e-5, "{got}");
    }

    #[test]
    fn very_familiar_item_has_low_surprisal() {
        // 새 벡터가 중심과 거의 같으면 min_dist ≈ 0 → 그보다 작은 과거 값이 없다.
        let c = clustering(vec![vec![1.0, 0.0]]);
        let d: Vec<f64> = (1..=12).map(|i| i as f64 * 0.05).collect();
        let s = compute(&[1.0, 0.0], Some(&c), &d);
        assert_eq!(s.surprisal, 0.0);
    }

    #[test]
    fn compute_is_deterministic() {
        let c = clustering(vec![vec![0.6, 0.8], vec![-0.6, 0.8]]);
        let d: Vec<f64> = (0..15).map(|i| i as f64 * 0.07).collect();
        let a = compute(&[0.3, 0.9], Some(&c), &d);
        let b = compute(&[0.3, 0.9], Some(&c), &d);
        assert_eq!(a, b);
    }

    #[test]
    fn recluster_rule_matches_spec() {
        assert!(should_recluster(0, 1));
        assert!(!should_recluster(100, 105));
        assert!(should_recluster(100, 110), "10% 증가");
        assert!(should_recluster(1000, 1020), "20건 증가");
        assert!(!should_recluster(1000, 1019));
    }
}
