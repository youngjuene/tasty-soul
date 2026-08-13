//! 임베딩 조회 — 캐시 우선, 미스일 때만 API (§R3 · §D3).
//!
//! > 로컬 임베딩 인코더를 **번들하지도 조달하지도 않는다.**
//!
//! **임베딩 대상은 `machine.prose` 문자열 단독이다** (§9.1). tags나 axes를 이어붙이지 않는다.
//! 캐시 키가 이 문자열에 걸리므로 정확히 지켜야 한다.
//!
//! `soul rebuild --offline`은 캐시 미스 시 **에러 종료**한다 (T28).
//!
//! ## 이 모듈이 텍스트를 절대 만지지 않는 이유
//!
//! 여기서 trim·정규화·접두어 추가를 하는 순간 캐시 키가 호출자가 생각하는 문자열과
//! 달라진다. 워밍된 캐시가 통째로 미스가 되고 `--offline` 재빌드가 까닭 없이 실패한다.
//! **호출자가 넘긴 문자열을 그대로 쓴다.**

use soul_core::db::embed_cache::{cache_key, Space};
use soul_core::db::Db;
use soul_core::error::{Result, SoulError};

pub struct Embedder<'a> {
    pub db: &'a Db,
    pub client: Option<&'a crate::openai::OpenAi>,
    pub provider: String,
    pub model: String,
    pub dims: usize,
    /// true면 캐시 미스가 에러다 (§R3).
    pub offline: bool,
}

impl Embedder<'_> {
    /// 캐시에 있으면 그대로, 없으면 API 호출 후 캐시에 넣는다.
    ///
    /// **캐시 히트면 네트워크를 건드리지 않는다** — `client`가 `None`이어도 성공한다.
    /// 이것이 §R2 "재빌드는 재호출이 아니라 재생"을 성립시키는 지점이다.
    pub async fn get(&self, text: &str) -> Result<Vec<f32>> {
        // §R3 — 키는 (provider, model, dims, text) 네 성분 전부에 걸린다.
        let key = cache_key(&self.provider, &self.model, self.dims, text);
        if let Some(v) = self.db.embed_get(&key)? {
            return Ok(v);
        }

        if self.offline {
            // T28 — `--offline`은 여기서 멈춘다. 조용히 재임베딩하면 오프라인 검증이 거짓이 된다.
            return Err(SoulError::EmbedCacheMiss(miss_detail(&key, text)));
        }

        let client = self.client.ok_or_else(|| {
            SoulError::config(format!(
                "임베딩 클라이언트가 없습니다 (API 키 미설정). 캐시 미스: {}",
                miss_detail(&key, text)
            ))
        })?;

        let mut vecs = client
            .embed(&self.model, self.dims, &[text.to_string()])
            .await?;
        let vec = match vecs.pop() {
            Some(v) if vecs.is_empty() => v,
            _ => {
                return Err(SoulError::invalid(
                    "임베딩 응답이 비었습니다 (1개를 요청했다)".to_string(),
                ))
            }
        };
        // §20.2 — 여기서 걸러야 캐시 키의 dims 가 벡터 길이와 어긋나지 않는다.
        if vec.len() != self.dims {
            return Err(SoulError::invalid(format!(
                "임베딩 차원 불일치: dims={} 를 요청했는데 {}차원이 왔습니다",
                self.dims,
                vec.len()
            )));
        }
        self.db.embed_put(&key, self.dims, &vec)?;
        // f16 손실을 캐시 히트/미스 사이에서 동일하게 만든다 — 히트일 때만 반올림된
        // 값이 나오면 §R5 군집 입력이 첫 실행과 재빌드에서 달라진다.
        self.db
            .embed_get(&key)?
            .ok_or_else(|| SoulError::invalid("임베딩 캐시 기록 직후 조회에 실패했습니다"))
    }

    /// 관측과 벡터를 연결한다. **공간을 반드시 지정한다** (§12.7, T49).
    ///
    /// `Space::Object`에는 `ingest.machine.prose`만, `Space::Critique`에는
    /// `context.critique`만 넣는다. `reading.prose`는 텍스트 캐시(`get`)까지만 쓰고
    /// **여기로 오면 안 된다** — 군집이 "무엇을 좋아하는가"에서 흘러나간다 (§18-3).
    pub async fn get_and_link(&self, space: Space, obs_id: &str, text: &str) -> Result<Vec<f32>> {
        let vec = self.get(text).await?;
        self.db.obs_vec_put(space, obs_id, &vec)?;
        Ok(vec)
    }
}

/// 미스 원인을 사람이 읽을 수 있게. 텍스트는 앞 40자만 — 오류 메시지가 서술문으로 뒤덮이지 않게 한다.
fn miss_detail(key: &str, text: &str) -> String {
    let head: String = text.chars().take(40).collect();
    let ellipsis = if text.chars().count() > 40 { "…" } else { "" };
    format!("key={key} text=\"{head}{ellipsis}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROVIDER: &str = soul_core::rebuild::EMBED_PROVIDER;
    const MODEL: &str = "text-embedding-3-small";
    const DIMS: usize = 4;

    /// f16으로 정확히 표현되는 값만 쓴다 — 왕복 비교가 흔들리지 않게.
    fn vec4() -> Vec<f32> {
        vec![0.5, -0.25, 0.125, 1.0]
    }

    fn embedder(db: &Db, offline: bool) -> Embedder<'_> {
        Embedder {
            db,
            // **client 를 None 으로 둔다.** 네트워크로 새면 테스트가 실패한다.
            client: None,
            provider: PROVIDER.to_string(),
            model: MODEL.to_string(),
            dims: DIMS,
            offline,
        }
    }

    fn warm(db: &Db, text: &str) {
        db.embed_put(&cache_key(PROVIDER, MODEL, DIMS, text), DIMS, &vec4())
            .unwrap();
    }

    #[tokio::test]
    async fn cache_hit_never_calls_the_api() {
        // client: None 이므로 API를 부르면 곧장 실패한다.
        let db = Db::open_in_memory().unwrap();
        warm(&db, "차갑고 정돈된 실내");
        let v = embedder(&db, false)
            .get("차갑고 정돈된 실내")
            .await
            .unwrap();
        assert_eq!(v, vec4());
    }

    #[tokio::test]
    async fn offline_miss_is_an_error() {
        // T28 — `soul rebuild --offline` 은 캐시 미스에서 에러 종료한다 (§R3).
        let db = Db::open_in_memory().unwrap();
        let err = embedder(&db, true)
            .get("워밍되지 않은 문장")
            .await
            .unwrap_err();
        assert!(
            matches!(err, SoulError::EmbedCacheMiss(_)),
            "받은 에러: {err}"
        );
        assert!(err.to_string().contains("워밍되지 않은 문장"), "{err}");
    }

    #[tokio::test]
    async fn offline_hit_succeeds() {
        // §R3 — 캐시가 워밍된 상태에서만 오프라인 재빌드가 성공한다.
        let db = Db::open_in_memory().unwrap();
        warm(&db, "차갑고 정돈된 실내");
        assert_eq!(
            embedder(&db, true).get("차갑고 정돈된 실내").await.unwrap(),
            vec4()
        );
    }

    #[tokio::test]
    async fn dims_change_invalidates_the_cache() {
        // T28 — embed.dims 를 바꾸면 기존 캐시는 미스가 되어야 한다.
        let db = Db::open_in_memory().unwrap();
        warm(&db, "같은 문장");
        let mut e = embedder(&db, true);
        e.dims = DIMS + 1;
        assert!(matches!(
            e.get("같은 문장").await.unwrap_err(),
            SoulError::EmbedCacheMiss(_)
        ));
    }

    #[tokio::test]
    async fn model_change_invalidates_the_cache() {
        let db = Db::open_in_memory().unwrap();
        warm(&db, "같은 문장");
        let mut e = embedder(&db, true);
        e.model = "text-embedding-3-large".to_string();
        assert!(matches!(
            e.get("같은 문장").await.unwrap_err(),
            SoulError::EmbedCacheMiss(_)
        ));
    }

    #[tokio::test]
    async fn text_is_used_verbatim() {
        // §9.1 — 호출자가 넘긴 문자열을 가공하지 않는다.
        // 공백·개행이 붙은 문자열을 그대로 키로 쓰는지 확인한다.
        let db = Db::open_in_memory().unwrap();
        let text = "  앞뒤 공백이 있는 문장\n";
        warm(&db, text);
        assert_eq!(embedder(&db, true).get(text).await.unwrap(), vec4());
        // trim 된 문자열은 다른 키다 — 즉 이 모듈이 trim 하지 않았다는 증거.
        assert!(embedder(&db, true).get(text.trim()).await.is_err());
    }

    #[tokio::test]
    async fn missing_client_when_online_is_a_config_error() {
        // 온라인인데 클라이언트가 없으면 조용히 0벡터를 만들지 않고 에러다 (§15 "임베딩 실패").
        let db = Db::open_in_memory().unwrap();
        assert!(matches!(
            embedder(&db, false).get("미스").await.unwrap_err(),
            SoulError::Config(_)
        ));
    }

    #[tokio::test]
    async fn get_and_link_writes_only_the_named_space() {
        // T49 · §12.7 — 공간을 틀리면 군집이 조용히 오염된다.
        let db = Db::open_in_memory().unwrap();
        warm(&db, "비평문");
        let e = embedder(&db, true);
        e.get_and_link(Space::Critique, "01BBB", "비평문")
            .await
            .unwrap();

        assert_eq!(
            db.obs_vec_get(Space::Critique, "01BBB").unwrap().unwrap(),
            vec4()
        );
        assert!(
            db.obs_vec_get(Space::Object, "01BBB").unwrap().is_none(),
            "비평 벡터가 주 공간으로 새어 들어갔다"
        );
        assert!(db.obs_vec_all(Space::Object).unwrap().is_empty());
    }

    #[tokio::test]
    async fn get_and_link_object_space() {
        let db = Db::open_in_memory().unwrap();
        warm(&db, "대상 서술");
        embedder(&db, true)
            .get_and_link(Space::Object, "01AAA", "대상 서술")
            .await
            .unwrap();
        assert_eq!(db.obs_vec_all(Space::Object).unwrap().len(), 1);
        assert!(db.obs_vec_all(Space::Critique).unwrap().is_empty());
    }

    #[tokio::test]
    async fn provider_matches_the_core_constant() {
        // §R3 — 캐시 키의 provider 성분은 soul-core 와 반드시 같아야 한다.
        // 여기가 어긋나면 rebuild 가 만든 캐시를 pipeline 이 못 찾는다.
        assert_eq!(PROVIDER, "openai");
    }
}
