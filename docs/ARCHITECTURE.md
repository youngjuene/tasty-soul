# tasty-soul — 아키텍처 계약

이 문서는 설계 명세를 코드 구조로 옮긴 것이다. 명세의 절 번호(§)를 그대로 참조한다.
**명세와 충돌하면 명세가 이긴다.**

명세 원본은 저장소에 포함하지 않는다. 각 § 가 무엇에 관한 절인지는
[`SPEC-REFERENCE.md`](SPEC-REFERENCE.md) 를 보라.

---

## 1. 크레이트 그래프

```
soul-core   ── 네트워크 없음. 도메인·저장·파생·SOUL.md·git·sqlite
   ▲   ▲   ▲
   │   │   └──────────────── soul-mcp   (바이너리)  ※ soul-core 외 의존 금지
   │   │
   │   └── soul-media  ── ffmpeg/image. 네트워크 없음
   │            ▲
   └── soul-net ──┤       ── reqwest. OpenAI · YouTube · 검색 · 조달
          ▲       │
     soul-agent ──┤       ── 비평/성찰 에이전트 루프 (§11)
          ▲       │
     soul-pipeline ┘      ── 수집 오케스트레이션 (§9)
          ▲
     ┌────┴────┐
  soul-cli   src-tauri     ── 명령 표면 (§14) · 앱 셸 (§13)
```

### 절대 규칙 — 이것이 깨지면 T32/T34가 실패한다

| 규칙 | 강제 방법 |
|---|---|
| `soul-core`는 `reqwest`/`hyper`/`tokio-net`을 의존하지 않는다 | `cargo tree -p soul-core \| grep reqwest` 가 비어야 함 (CI) |
| `soul-mcp`는 `soul-core` 외 앱 크레이트를 의존하지 않는다 | `Cargo.toml` 검사 + CI 스크립트 |
| `soul-mcp` 바이너리에 HTTP 클라이언트가 링크되지 않는다 (§19.4) | `xcrun nm`/`strings` 대신 `cargo tree` 로 검증 (`ci/check-deps.sh`) |
| `soul mcp` 서브커맨드는 별도 `soul-mcp` 바이너리를 exec 한다 | `soul-cli/src/cmd/mcp.rs` |

`soul mcp`(§14/§19.7)는 CLI 안에서 서버를 구동하지 않는다. 같은 디렉토리의
`soul-mcp` 실행 파일을 찾아 `exec`(unix) / `spawn+wait`(windows) 한다.
이렇게 해야 §2의 "별도 바이너리 · 네트워크 의존성 미링크"와 §14의 명령 표면이 동시에 성립한다.

---

## 2. `soul-core` 모듈 지도

| 모듈 | 명세 | 책임 |
|---|---|---|
| `error` | §15 | `SoulError` 단일 에러 타입 |
| `paths` | §3 | 앱 데이터 루트, `soul/`·`cache/`·`runs/`·`exports/`·`bin/` 해석 |
| `config` | §9.8 §19.9 | `config.toml` 로드/저장, 기본값 |
| `ids` | §6 | ULID 생성(단조), 파싱, 순서 |
| `time` | §R1 | `Ts` (RFC3339 밀리초 UTC), `T_ref` |
| `canon` | §R6 | 정준 JSON 직렬화 — 키 정렬·2칸·LF·부동소수 6자리 |
| `obs::model` | §6 | 관측 타입 전부 |
| `obs::store` | §3 §20.6 | 월별 샤딩 읽기/쓰기, ULID 순 재생 이터레이터 |
| `lock` | §R7 | `soul/.write.lock` flock |
| `git` | §R8 | `git init`, 쓰기 1회 = 커밋 1개 |
| `soulmd::parse` | §8.3 | 줄 단위 상태 기계 파서, 해시 정규화 |
| `soulmd::render` | §8.2 | 템플릿 렌더 |
| `soulmd::save` | §8.4 | 저장 시퀀스 8단계 |
| `db::schema` | §12 §20.3 | sqlite 스키마·마이그레이션 |
| `db::embed_cache` | §R3 §20.2-3 | 임베딩 f16 캐시. 키 = sha256(provider‖0‖model‖0‖dims‖0‖text) |
| `db::queue` | §9.10 | `critique_queue` 영속 큐 |
| `db::derived_cache` | §12.5 §20.7 | `state_at` 월별 캐시, 군집 캐시, PCA 캐시 |
| `vecmath` | §20.3 | f16 인코딩, L2 정규화, 코사인 |
| `derived::axes` | §12.1-2 | `computed`/`offset`/`final`, 90일 변화 |
| `derived::cluster` | §12.3 §R5 | k-means++ 고정 시드 42 |
| `derived::surprisal` | §12.4 | 투입 시점 계산 (동결) |
| `derived::state` | §12.5 | `state_at(month)`, 실루엣, stride 표본화 |
| `derived::divergence` | §12.6 | 2×2 셀, layer별 divergence, coherence |
| `derived::pca` | §13-6 | 결정론적 2성분 PCA (거듭제곱법) |
| `derived::stats` | §11.2 §19.3 | `query_stats` 페이로드 |
| `rebuild` | §R2 §14 | 관측 재생 엔진 |
| `prompts` | §R11 | 프롬프트 파일 로드 + 렌더 후 sha256 |
| `soulblocks` | §D4 | **목적지별** 블록 조립. `SOUL.md` 통째 읽기 금지 |

---

## 3. 데이터 흐름 — 투입 1건 (§9.1)

```
입력
 └ kind 판별 (§9.1: magic bytes, 확장자 불신)      soul-media::probe
 └ YouTube면 해석 (§9.3)                            soul-net::youtube
 └ sha256 + 썸네일 (§20.4)                          soul-media::thumb
 └ 모달리티별 서술 (§9.2/9.4/9.5/9.6)               soul-pipeline::describe
 └ machine.prose 확정
 └ embed(machine.prose)                             soul-net::embed → soul-core::db::embed_cache
 └ min_dist·surprisal (§12.4, 캐시된 중심 사용)     soul-core::derived::surprisal
 └ ingest 관측 기록 + 커밋                          soul-core::obs::store + git
 ├─ 즉시 → 화면 2
 └─ critique_queue INSERT (§9.10)                   soul-core::db::queue
        └ 워커 → 비평 에이전트 (§11.1)              soul-agent::critique
              └ context 관측 기록 + 커밋 → 화면 2.1
```

**재렌더는 여기서 일어나지 않는다** (§R8). `ingest`/`reading`은 `SOUL.md`를 건드리지 않는다.

---

## 4. 결정론 체크리스트 (§5)

구현 중 아래를 어기면 T1·T12·T14가 실패한다. 리뷰 시 이 목록으로 훑는다.

- [ ] 파생값의 시간 기준은 `T_ref`(최대 `ts`)이며 `now()`가 아니다 (R1)
- [ ] `now()`는 **새 관측 기록**과 **git 커밋 타임스탬프**에만 쓴다 (R1/R8)
- [ ] LLM 출력은 관측에 기록되고 재빌드는 재호출이 아니다 (R2)
- [ ] `soul:human`은 재생 대상이 아니라 기존 파일에서 이월된다 (R2)
- [ ] 임베딩만이 유일한 네트워크 예외. `--offline`은 캐시 미스에서 에러 (R3)
- [ ] `min_dist`/`surprisal`은 절대 재계산하지 않는다 (R4)
- [ ] k-means++ 시드 42 · 100회 · 1e-6 · 코사인 · ULID 정렬 입력 (R5)
- [ ] JSON: UTF-8, BOM 없음, LF, 2칸, 키 정렬, 부동소수 6자리 (R6)
- [ ] 모든 쓰기는 `.write.lock` 안에서 (R7)
- [ ] 쓰기 1회 = 커밋 1개. 재렌더 계기는 4개뿐 (R8)
- [ ] supersede된 `ingest`는 §12의 **모든** 계산에서 제외 (R9)
- [ ] `null` → `—` (R10)
- [ ] `prompt_sha256`은 **렌더 완료된 최종 시스템 프롬프트**의 해시 (R11)

---

## 5. 프런트엔드

| 경로 | 화면 | 명세 |
|---|---|---|
| `src/screens/Ingest.tsx` | 1 — 투입 | §13-1 |
| `src/screens/SensoryCard.tsx` | 2 — 감각 글귀 ○/× | §13-2 |
| `src/screens/CulturalCard.tsx` | 2.1 — 문화 글귀 ○/× | §13-2.1 |
| `src/screens/SoulDoc.tsx` | 3 — SOUL.md | §13-3 |
| `src/screens/ApproveDiff.tsx` | 4 — 승인 diff | §13-4 |
| `src/screens/Dashboard.tsx` | 5 — 대시보드 3패널 | §13-5 |
| `src/screens/Archive.tsx` | 6 — 아카이브 탐색 | §13-6 |
| `src/screens/Setup.tsx` | 최초 실행 · 경계 고지 · doctor | §D7 §9.9 |

차트는 전부 손으로 쓴 SVG다 (§2, §20.8). 라이브러리를 추가하지 않는다.
프런트는 **순수 뷰**다. 모든 판단·I/O·API 호출은 Rust에 있다 (§2).
API 키를 프런트로 반환하는 커맨드를 만들지 않는다 (§2).

---

## 6. 테스트 배치

| 위치 | 내용 |
|---|---|
| `crates/*/src/**` `#[cfg(test)]` | 단위 테스트 |
| `crates/soul-core/tests/` | T1–T10, T12, T14–T22, T40–T41, T45–T46, T49–T50, T55–T57, T69 |
| `crates/soul-media/tests/` | T11f–T11h, T24–T25d, T43, T60–T66 |
| `crates/soul-mcp/tests/` | T30–T38 |
| `crates/soul-pipeline/tests/` | T11–T11e, T26–T28, T47–T48, T51–T54, T58 |
| `fixtures/obs-100/` | 관측 100건 픽스처 (§17) |
| `fixtures/obs-5000/` | 성능 픽스처 (T39·T67) — 생성 스크립트로 만든다 |
| `ci/check-deps.sh` | 크레이트 의존 규칙 강제 |

네트워크가 필요한 테스트는 `#[ignore]` + `SOUL_E2E=1` 게이트를 건다.
CI 기본 경로는 **완전 오프라인**이며 임베딩은 픽스처 캐시로 워밍한다.
