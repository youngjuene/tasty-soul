# tasty-soul

미디어를 투입하면 시스템이 **두 개의 글귀**를 만든다.

| 글귀 | 출처 | 묻는 것 |
|---|---|---|
| **감각 글귀** | API가 대상 **자체**를 분석한 감각적 특징점의 서술. 검색하지 않는다 | 그렇게 보이는가 |
| **문화 글귀** | 검색으로 수집한 정보를 큐레이션한 비평. 대상 **바깥**에서 온다 | 그래서 좋아하는가 |

각각에 **○ / ×** 로 답한다. 두 응답의 조합이 2×2 셀을 이루며, 그것이 취향 모델의 주 신호다.
모든 사건이 추가 전용 관측 로그에 쌓이고, 그 로그만으로 `SOUL.md`와 대시보드가 결정론적으로 재생성된다.

구현 구조는 [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), 명세가 미정으로 남긴 것에 대한
구현상의 선택은 [`docs/OPEN-DECISIONS.md`](docs/OPEN-DECISIONS.md)에 있다.

코드 주석은 2000곳 넘게 `§` 번호로 설계 명세를 참조한다. **원본 설계 문서는 공개하지
않으므로**, 각 절이 무엇에 관한 것인지는 [`docs/SPEC-REFERENCE.md`](docs/SPEC-REFERENCE.md)
에 정리해 두었다. 규칙의 전문은 해당 코드의 doc comment(`//!`)에 옮겨져 있고 그쪽이 진실이다.

---

## 데이터 경계 — 무엇이 기기를 떠나는가

이 시스템은 두 국면으로 나뉘고 **프라이버시 성질이 서로 다르다** (§D1).

| 국면 | 하는 일 | 데이터 |
|---|---|---|
| **구축** | 미디어 해석, 임베딩, 성찰 → `SOUL.md` 생성 | OpenAI API로 전송된다. 불가피하다 |
| **적용** | 로컬 에이전트가 `SOUL.md`와 관측 아카이브를 조회 | **기기를 떠나지 않는다** |

구축 국면의 원격 의존은 인정하고 정직하게 고지한다. 대신 **적용 국면은 완전히 로컬**이다.

**투입 하나가 곧 검색 몇 건이다** (§D6). 문화 글귀는 투입마다 자동으로 웹 검색을 일으키고,
검색어에 무엇에 관심이 있는지가 직접 드러난다. 검색어는 `context.queries`에 전문 기록되어
언제든 확인할 수 있고, `context_enabled = false`로 문화 층 전체를 끌 수 있다.

민감한 자료는 애초에 투입하지 않는 것이 유일하게 정직한 방법이다.

---

## 구조

```
soul-core   ── 네트워크 없음. 도메인·저장·파생·SOUL.md·git·sqlite
   ▲   ▲   ▲
   │   │   └──────────────── soul-mcp   (바이너리)  ※ soul-core 외 의존 금지
   │   └── soul-media  ── ffmpeg/image. 네트워크 없음
   └── soul-net ── reqwest. OpenAI · YouTube · 검색 · 조달
          ▲
     soul-agent → soul-pipeline → soul-cli · src-tauri
```

`soul-mcp` 바이너리에 HTTP 클라이언트를 **링크하지 않는 것**으로 "적용은 로컬"을 강제한다.
`ci/check-deps.sh`가 이 규칙을 검사한다.

---

## 환경 준비

```bash
./scripts/setup.sh            # macOS · Linux
pwsh -File scripts/setup.ps1  # Windows 10+
```

멱등하다. 없는 것만 설치하고, 이미 있는 것은 건너뛴다.

| 플래그 | 하는 일 |
|---|---|
| `--check` | 아무것도 설치하지 않고 진단만 한다 |
| `--no-keys` | 도구만. 키 저장소를 건드리지 않는다 |
| `--keys-only` | 키만 다시 넣는다 (키를 새로 발급했을 때) |

확인·설치하는 것: Rust 툴체인 · Node 20+ · **ffmpeg/ffprobe** · **yt-dlp** ·
플랫폼별 Tauri 의존성(Linux webkit2gtk, macOS Xcode CLT, Windows WebView2) ·
`npm install` · `cargo fetch`.

**없어도 되는 것과 없으면 안 되는 것을 구분해서 알려준다.** ffmpeg이 없으면 영상·오디오
경로만 죽고 이미지·텍스트는 동작한다. yt-dlp가 없는 것은 **오류가 아니다** —
§9.3 단계 6의 썸네일+메타데이터 경로로 내려가 `quality: minimal`로 기록된다 (T11).

### API 키

```bash
cp .env.example .env     # setup.sh 가 없으면 알아서 만든다
$EDITOR .env             # OPENAI_API_KEY 를 채운다
./scripts/setup.sh --keys-only
```

**`.env`는 키를 넣기 위한 투입구일 뿐이다.** §2에 따라 앱이 키를 읽는 유일한 출처는
**OS 키체인**이며, `setup.sh`가 `.env`의 값을 거기로 옮긴 뒤로는 이 파일을 쳐다보지 않는다.
옮긴 뒤에는 지워도 된다. `.env`는 `.gitignore`에 있다.

키 저장소를 둘 두면 어느 쪽이 진실인지 알 수 없게 되고 평문 파일 쪽이 조용히 이긴다.
그래서 한쪽만 진실로 둔다.

```bash
soul secrets status                                  # 설정 여부만. 값은 출력하지 않는다
printf '%s' "$OPENAI_API_KEY" | soul secrets set openai   # 셸 히스토리에 남기지 않는다
soul secrets delete openai
```

| 키 | 필요성 |
|---|---|
| `OPENAI_API_KEY` | **필수.** 없으면 모든 투입 경로가 비활성화된다 (§15) |
| `SEARCH_API_KEY` | 선택. `provider`가 기본값 `duckduckgo`면 필요 없다 |
| `YOUTUBE_API_KEY` | 선택. 없으면 `kind` 추정이 `video`로 고정된다 (§9.3) |

**macOS는 키체인 접근에 승인 대화상자를 띄운다.** `soul` 바이너리를 다시 빌드하면 서명이
바뀌어 다시 물어본다 — 정상이며, "항상 허용"을 누르면 그 빌드에 대해서는 다시 묻지 않는다.
비대화형 환경(CI·파이프·일부 에디터 터미널)에서는 아무도 그 창을 누를 수 없어 멈추므로,
`setup.sh`는 키체인을 만지는 모든 호출에 시간 상한을 걸고 무엇을 해야 하는지 알려준다.
키 설정은 **터미널에서 직접** 실행하는 것이 가장 확실하다.

헤드리스 CI처럼 키체인이 없는 환경에서는 `SOUL_ALLOW_ENV_SECRETS=1`로 환경변수를
직접 쓸 수 있다. **기본값은 꺼짐이고 그대로 두는 것이 맞다** — 켜면 평문 키가 프로세스
환경에 남는다. 켜져 있어도 **키체인이 먼저다**(셸에 남은 오래된 키가 설정 화면에서 넣은
키를 조용히 덮어쓰지 않게).

## 화면

인터페이스는 전부 [`ui/`](ui/) 안에 있다. 디자인 작업은 그 디렉토리만 보면 된다 —
브리프는 [`ui/README.md`](ui/README.md), 토큰은 [`ui/TOKENS.md`](ui/TOKENS.md).

```bash
npm run design      # Rust · Tauri · API 키 없이 화면만 띄운다 (localhost:1421)
```

`ui/preview/` 가 백엔드 커맨드 28개를 전부 가짜로 대신하므로 브라우저에서
모든 화면이 실제 데이터 모양 그대로 뜬다.

## 개발

```bash
cargo test --workspace          # 인수 테스트는 완전 오프라인이다 (§17)
./ci/check-deps.sh              # 크레이트 경계 (§19.4)
cargo clippy --workspace --all-targets -- -D warnings

npm run app:dev                 # 앱 (개발 서버 사용 · 핫 리로드)
npm run app:debug               # 자산이 박힌 디버그 바이너리 → target/debug/tasty-soul
npm run app:release             # .app + .dmg → target/release/bundle/
npm run icon                    # ui/icon.svg → 전 플랫폼 아이콘 재생성
cargo run -p soul-cli -- doctor # CLI 진단
cargo run -p soul-cli -- doctor --probe   # 모델 슬롯까지 (네트워크 사용, §9.9)

SOUL_E2E=1 cargo test --workspace -- --ignored   # 네트워크가 필요한 테스트

# API 키 없이 읽기 경로(파생 층·SOUL.md·아카이브·MCP)를 실데이터로 시험한다
cargo run -p soul-core --example seed -- /tmp/soul-demo 120
SOUL_ROOT=/tmp/soul-demo cargo run -p soul-cli -- render
```

> **`cargo build` 가 만든 `target/debug/tasty-soul` 을 직접 실행하면 창이 빈 채로 뜬다.**
> 버그가 아니다 — `cargo` 만 돌리면 프런트엔드가 빌드되지 않으므로 Tauri 는
> `tauri.conf.json` 의 `devUrl`(`localhost:1420`)로 떨어지고, 개발 서버가 없으면
> 아무것도 못 불러온다.
>
> `npm run app:debug` 는 프런트엔드를 먼저 빌드해 **같은 경로의 바이너리를 자립형으로
> 바꾼다.** 1420 포트가 닫혀 있어도 정상적으로 뜨므로, 디버그 심볼을 유지한 채
> 배포본과 같은 자산 해석 경로를 시험할 수 있다. 핫 리로드가 필요하면 `npm run app:dev`.

### 아이콘

원본은 [`ui/icon.svg`](ui/icon.svg) 하나다. 고치고 `npm run icon` 을 돌리면
`src-tauri/icons/` 전체(icns · ico · png · Windows 스토어 타일)가 다시 만들어진다.
생성물은 커밋되어 있으므로 빌드에 별도 준비가 필요하지 않다.

### 다른 기계로 옮길 때

`.app` 을 복사해도 **ffmpeg·yt-dlp 는 따라가지 않는다.** 번들하지 않기 때문이다
(§9.7 · §20.8). API 키도 마찬가지로 OS 키체인에 있지 바이너리에 없다.

빠진 것이 있으면 앱이 조용히 반쪽으로 동작하지 않고 **설정 → 진단**이 무엇이 멈추고
무엇은 멀쩡한지, 그리고 그 플랫폼의 설치 명령을 함께 적어 준다. ffmpeg 이 없으면
영상·오디오 경로만 죽고, yt-dlp 가 없는 것은 오류가 아니다 — YouTube 가 썸네일+메타데이터
경로로 처리되어 `quality: minimal` 로 기록될 뿐이다 (§9.3 단계 6).
새 기계라면 `scripts/setup.sh` 가 한 번에 채운다.

---

## 명령

```
soul doctor                          # 키·모델 ID·ffmpeg 검증
soul ingest <path|url|->
soul read <obs_id> <verdict> [prose]
soul context <ingest_id> [--redo]    # 문화 글귀 재생성. 평시엔 자동
soul recast <ingest_id> <kind>       # YouTube 항목의 kind 뒤집기
soul reflect [--force]
soul render
soul rebuild [--from-scratch] [--offline]
soul reanalyze <obs_id>
soul stats [--json]
soul mcp [--print-config]            # 로컬 MCP 서버
soul export --target=prompt
soul maintain                        # git gc 등 정리 작업
soul trace purge                     # runs/ 전체 삭제
soul secrets set|status|delete <openai|search|youtube>   # OS 키체인 (§2)
soul secrets import-env              # 환경변수 → 키체인 (setup.sh 가 쓴다)
```

`soul secrets`는 명세 §14의 목록에 없다. 키를 넣을 방법이 GUI밖에 없으면 스크립트로
환경을 세팅할 수 없어 추가했다 — `docs/OPEN-DECISIONS.md` #23.

---

## 로컬 에이전트에 붙이기

```bash
soul mcp --print-config
```

```json
{ "mcpServers": { "soul": { "command": "soul", "args": ["mcp"] } } }
```

**앱이 클라이언트 설정 파일을 직접 수정하지 않는다** (§19.7). 출력과 안내까지만 한다.

`soul-mcp`는 MCP 클라이언트이기만 하면 어느 에이전트 루프에도 붙는다.
그 아래의 추론 프로바이더와 모델 가중치는 이 앱의 구현 범위가 아니다 (§19.2).

`SOUL.md`를 시스템 프롬프트로 밀어넣는 방식은 쓰지 않는다 — 프롬프트는 압축이고,
로그는 자라지만 프롬프트는 못 자란다. 축적이 이 제품의 본질인데 적용 단계에서 사라진다 (§19.1).
