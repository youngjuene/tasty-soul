//! 임베딩 캐시 (§R3 · §20.2 · §20.3).
//!
//! > 임베딩은 관측 JSON이 아니라 `derived.sqlite`에만 저장한다
//! > (float 수백 개는 git diff를 파괴한다).
//!
//! **캐시 키 = `sha256(provider || 0x00 || model_id || 0x00 || dims || 0x00 || text)`**
//! 제공자·모델·차원 중 하나라도 바뀌면 자동으로 캐시 미스가 된다 (T28).
//!
//! 저장은 **float16 고정 길이 블롭**이다 (§20.3, T45).
//!
//! **이것이 재현성의 유일한 네트워크 예외이며** `--offline`으로 검증한다.

use crate::error::{Result, SoulError};
use crate::obs::model::Layer;
use crate::vecmath::{from_f16_blob, to_f16_blob};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};

/// §R3의 캐시 키. `dims`는 ASCII 십진수로 넣는다.
pub fn cache_key(provider: &str, model_id: &str, dims: usize, text: &str) -> String {
    // 0x00 구분자가 요점이다. 이게 없으면 ("ab","c")와 ("a","bc")가 같은 키가 되고,
    // 모델 이름 끝이 숫자면 dims 변경이 묻힌다 (T28).
    let mut h = Sha256::new();
    h.update(provider.as_bytes());
    h.update([0x00]);
    h.update(model_id.as_bytes());
    h.update([0x00]);
    h.update(dims.to_string().as_bytes());
    h.update([0x00]);
    h.update(text.as_bytes());
    hex::encode(h.finalize())
}

/// §12.7의 두 임베딩 공간.
///
/// # 불변식 — id 색인에는 대상만 들어간다
///
/// `obs_vec`(Object)에는 **`ingest` 의 `machine.prose`** 만, `critique_vec` 에는
/// **`context` 의 `critique`** 만 넣는다. `reading.prose` 는 텍스트 키 캐시
/// (`embed_put`)에만 두고 **`obs_vec_put` 으로 색인하지 않는다.**
///
/// 이유: `divergence`·`coherence` 는 텍스트로 조회하므로 캐시만으로 충분하지만,
/// id 색인은 `cluster()`·`soul_similar` 가 "투입 대상들"의 목록으로 쓴다.
/// 거기에 사용자 정정문이 섞이면 군집이 "무엇을 좋아하는가"가 아니라
/// "무엇에 대해 무슨 말이 오갔는가"로 흐른다 — 결과는 여전히 그럴듯해 보인다 (§18-3, T49).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// `ingest.machine.prose`. 군집·drift·crystal·surprisal이 쓰는 **유일한** 공간.
    Object,
    /// `context.critique`, `layer=cultural`인 `reading.prose`.
    /// `cluster()`·`state_at()`·`surprisal` 계산에 **절대 넣지 않는다.**
    Critique,
}

impl Space {
    /// reading의 layer에 따른 공간. sensory는 대상이 `machine.prose`이므로 Object다.
    pub fn for_reading(layer: Layer) -> Space {
        match layer {
            Layer::Sensory => Space::Object,
            Layer::Cultural => Space::Critique,
        }
    }
    pub fn table(&self) -> &'static str {
        match self {
            Space::Object => "obs_vec",
            Space::Critique => "critique_vec",
        }
    }
}

impl super::Db {
    pub fn embed_get(&self, key: &str) -> Result<Option<Vec<f32>>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT vec FROM embed_cache WHERE key = ?1",
                rusqlite::params![key],
                |r| r.get(0),
            )
            .optional()?;
        match blob {
            None => Ok(None),
            Some(b) => decode_vec(&b, key).map(Some),
        }
    }

    pub fn embed_put(&self, key: &str, dims: usize, vec: &[f32]) -> Result<()> {
        // dims는 키에 들어간 값이다(§R3). 벡터 길이와 어긋나면 키가 거짓말을 하게 된다.
        if vec.len() != dims {
            return Err(SoulError::invalid(format!(
                "임베딩 차원 불일치: 키의 dims={dims}, 벡터 길이={}",
                vec.len()
            )));
        }
        self.conn.execute(
            "INSERT INTO embed_cache(key, dims, vec) VALUES(?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET dims = excluded.dims, vec = excluded.vec",
            rusqlite::params![key, dims as i64, to_f16_blob(vec)],
        )?;
        Ok(())
    }

    /// 관측 ID ↔ 벡터 연결. 공간을 반드시 지정한다 (§12.7).
    pub fn obs_vec_put(&self, space: Space, obs_id: &str, vec: &[f32]) -> Result<()> {
        // `table()`은 정적 문자열 두 개뿐이라 문자열 조립이 안전하다 (사용자 입력 아님).
        self.conn.execute(
            &format!(
                "INSERT INTO {t}(obs_id, vec) VALUES(?1, ?2)
                 ON CONFLICT(obs_id) DO UPDATE SET vec = excluded.vec",
                t = space.table()
            ),
            rusqlite::params![obs_id, to_f16_blob(vec)],
        )?;
        Ok(())
    }

    pub fn obs_vec_get(&self, space: Space, obs_id: &str) -> Result<Option<Vec<f32>>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                &format!("SELECT vec FROM {t} WHERE obs_id = ?1", t = space.table()),
                rusqlite::params![obs_id],
                |r| r.get(0),
            )
            .optional()?;
        match blob {
            None => Ok(None),
            Some(b) => decode_vec(&b, obs_id).map(Some),
        }
    }

    /// 공간 전체를 메모리에 올린다. 5,000건 × 512 B = 2.5 MB (§20.3).
    /// `soul_similar`는 완전탐색으로 충분하다 — 벡터 색인을 도입하지 않는다.
    pub fn obs_vec_all(&self, space: Space) -> Result<Vec<(String, Vec<f32>)>> {
        // **ULID 오름차순**이다. §R5가 군집 입력 순서를 결정론의 일부로 못박았다 (T12).
        let mut stmt = self.conn.prepare(&format!(
            "SELECT obs_id, vec FROM {t} ORDER BY obs_id ASC",
            t = space.table()
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, blob) = row?;
            let v = decode_vec(&blob, &id)?;
            out.push((id, v));
        }
        Ok(out)
    }

    pub fn embed_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM embed_cache", [], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }
}

/// f16 블롭 → f32. 길이가 홀수면 캐시가 손상된 것이다.
fn decode_vec(blob: &[u8], what: &str) -> Result<Vec<f32>> {
    from_f16_blob(blob).ok_or_else(|| {
        SoulError::invalid(format!(
            "손상된 f16 벡터 블롭 ({what}): {}바이트는 2의 배수가 아닙니다",
            blob.len()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[test]
    fn cache_key_matches_spec_byte_layout() {
        // sha256(b"p\x00m\x00256\x00t") — 명세 §R3의 바이트 배치를 못박는다.
        assert_eq!(
            cache_key("p", "m", 256, "t"),
            "40f7129f32bdec1e48d9ef1f6a87f06a5ca810ba8619e958ef7a9cf2d211bcf3"
        );
    }

    #[test]
    fn cache_key_changes_when_dims_change() {
        // T28 — embed.dims 변경 → 기존 캐시 미스.
        let a = cache_key(
            "openai",
            "text-embedding-3-small",
            256,
            "차갑고 정돈된 실내",
        );
        let b = cache_key(
            "openai",
            "text-embedding-3-small",
            512,
            "차갑고 정돈된 실내",
        );
        assert_ne!(a, b);
        assert_eq!(
            b,
            cache_key(
                "openai",
                "text-embedding-3-small",
                512,
                "차갑고 정돈된 실내"
            ),
            "같은 입력은 같은 키여야 한다"
        );
    }

    #[test]
    fn cache_key_changes_with_provider_model_and_text() {
        let base = cache_key("openai", "m", 256, "t");
        assert_ne!(base, cache_key("voyage", "m", 256, "t"));
        assert_ne!(base, cache_key("openai", "m2", 256, "t"));
        assert_ne!(base, cache_key("openai", "m", 256, "t2"));
    }

    #[test]
    fn null_separator_prevents_field_bleeding() {
        // 구분자가 없으면 ("ab","c")와 ("a","bc")가 같은 바이트열이 된다.
        assert_ne!(cache_key("ab", "c", 8, "x"), cache_key("a", "bc", 8, "x"));
        // 모델 이름 끝의 숫자가 dims로 새지 않는다.
        assert_ne!(cache_key("p", "m2", 56, "x"), cache_key("p", "m", 256, "x"));
    }

    #[test]
    fn embed_roundtrip_is_f16_lossy_but_close() {
        let db = Db::open_in_memory().unwrap();
        let v: Vec<f32> = (0..256).map(|i| ((i as f32) * 0.013).sin()).collect();
        let key = cache_key("openai", "m", 256, "t");
        db.embed_put(&key, 256, &v).unwrap();

        let back = db.embed_get(&key).unwrap().unwrap();
        assert_eq!(back.len(), 256);
        assert!(crate::vecmath::cosine_distance(&v, &back) < 1e-5);
        assert!(db.embed_get("없는키").unwrap().is_none());
        assert_eq!(db.embed_count().unwrap(), 1);
    }

    #[test]
    fn stored_blob_is_two_bytes_per_dim() {
        // T45 — embed.dims = 256 → 관측당 512 B.
        let db = Db::open_in_memory().unwrap();
        db.embed_put("k", 256, &vec![0.5f32; 256]).unwrap();
        let len: i64 = db
            .conn
            .query_row(
                "SELECT length(vec) FROM embed_cache WHERE key='k'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(len, 512);

        db.obs_vec_put(Space::Object, "01OBS", &vec![0.5f32; 256])
            .unwrap();
        let len: i64 = db
            .conn
            .query_row("SELECT length(vec) FROM obs_vec", [], |r| r.get(0))
            .unwrap();
        assert_eq!(len, 512);
    }

    #[test]
    fn embed_put_rejects_dims_mismatch() {
        let db = Db::open_in_memory().unwrap();
        let err = db.embed_put("k", 256, &[0.1, 0.2]).unwrap_err();
        assert!(matches!(err, SoulError::Invalid(_)), "받은 에러: {err}");
    }

    #[test]
    fn embed_put_overwrites_same_key() {
        let db = Db::open_in_memory().unwrap();
        db.embed_put("k", 2, &[1.0, 0.0]).unwrap();
        db.embed_put("k", 2, &[0.0, 1.0]).unwrap();
        assert_eq!(db.embed_count().unwrap(), 1);
        assert_eq!(db.embed_get("k").unwrap().unwrap(), vec![0.0, 1.0]);
    }

    #[test]
    fn the_two_spaces_never_mix() {
        // T49 · §12.7 — 같은 obs_id라도 공간이 다르면 다른 벡터다.
        // 여기서 섞이면 군집이 "무엇을 좋아하는가"에서 "무엇에 대해 글이 쓰였는가"로 흐른다.
        let db = Db::open_in_memory().unwrap();
        db.obs_vec_put(Space::Object, "01AAA", &[1.0, 0.0]).unwrap();
        db.obs_vec_put(Space::Critique, "01AAA", &[0.0, 1.0])
            .unwrap();
        db.obs_vec_put(Space::Critique, "01BBB", &[0.0, -1.0])
            .unwrap();

        assert_eq!(
            db.obs_vec_get(Space::Object, "01AAA").unwrap().unwrap(),
            vec![1.0, 0.0]
        );
        assert_eq!(
            db.obs_vec_get(Space::Critique, "01AAA").unwrap().unwrap(),
            vec![0.0, 1.0]
        );
        assert!(db.obs_vec_get(Space::Object, "01BBB").unwrap().is_none());

        let object = db.obs_vec_all(Space::Object).unwrap();
        assert_eq!(object.len(), 1, "주 공간에 비평 벡터가 새어 들어왔다");
        assert_eq!(object[0].0, "01AAA");
        assert_eq!(db.obs_vec_all(Space::Critique).unwrap().len(), 2);
    }

    #[test]
    fn obs_vec_all_is_ulid_ordered() {
        // §R5 — 군집 입력은 ULID 오름차순이어야 결정론이 성립한다 (T12).
        let db = Db::open_in_memory().unwrap();
        for id in ["01C", "01A", "01B"] {
            db.obs_vec_put(Space::Object, id, &[1.0, 0.0]).unwrap();
        }
        let ids: Vec<String> = db
            .obs_vec_all(Space::Object)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, ["01A", "01B", "01C"]);
    }

    #[test]
    fn space_for_reading_follows_layer() {
        // cultural reading의 대상은 비평문이므로 별도 공간이다 (§12.7).
        assert_eq!(Space::for_reading(Layer::Sensory), Space::Object);
        assert_eq!(Space::for_reading(Layer::Cultural), Space::Critique);
        assert_eq!(Space::Object.table(), "obs_vec");
        assert_eq!(Space::Critique.table(), "critique_vec");
    }

    #[test]
    fn corrupt_blob_errors_instead_of_panicking() {
        let db = Db::open_in_memory().unwrap();
        db.conn
            .execute(
                "INSERT INTO embed_cache(key, dims, vec) VALUES('k', 1, x'00')",
                [],
            )
            .unwrap();
        assert!(matches!(
            db.embed_get("k").unwrap_err(),
            SoulError::Invalid(_)
        ));
    }
}
