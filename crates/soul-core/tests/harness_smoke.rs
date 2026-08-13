//! 픽스처 하네스 자체의 검증.
mod common;

#[test]
fn fixture_100_is_valid_and_deterministic() {
    let a = common::fixture_100();
    let b = common::fixture_100();
    assert_eq!(a.len(), b.len());
    assert!(a.len() >= 100, "관측 100건 이상이어야 한다: {}", a.len());
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x, y, "픽스처는 결정론적이어야 한다");
        x.validate()
            .unwrap_or_else(|e| panic!("불변식 위반 {}: {e}", x.id()));
    }
    // ULID 사전순 = 생성순
    let ids: Vec<_> = a.iter().map(|o| o.id().clone()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
}

#[test]
fn fake_embed_is_deterministic_and_unit_length() {
    let v1 = common::fake_embed("차갑고 정돈된", 256);
    let v2 = common::fake_embed("차갑고 정돈된", 256);
    assert_eq!(v1, v2);
    assert_eq!(v1.len(), 256);
    assert!((soul_core::vecmath::norm(&v1) - 1.0).abs() < 1e-5);
    assert_ne!(v1, common::fake_embed("다른 텍스트", 256));
}
