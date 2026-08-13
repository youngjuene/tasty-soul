#!/usr/bin/env bash
#
# tasty-soul 개발 환경 부트스트랩 (macOS · Linux).
#
#   ./scripts/setup.sh              # 없는 것만 설치하고 확인한다
#   ./scripts/setup.sh --check      # 아무것도 설치하지 않고 진단만
#   ./scripts/setup.sh --no-keys    # 도구만. 키체인은 건드리지 않는다
#   ./scripts/setup.sh --keys-only  # 키만 다시 넣는다 (키를 새로 발급했을 때)
#
# 멱등하다. 여러 번 돌려도 안전하며, 이미 있는 것은 건너뛴다.
#
# ─────────────────────────────────────────────────────────────────────────────
#  이 스크립트가 앱의 조달(§9.7)을 대신하지 않는다
# ─────────────────────────────────────────────────────────────────────────────
#
#  §9.7·§20.8 — 앱은 ffmpeg 을 **번들하지 않는다.** 런타임에 PATH 를 먼저 보고,
#  없으면 <root>/bin/ 으로 조달한다. §9.3 — yt-dlp 도 번들하지 않는다("몇 주면 낡는다").
#
#  이 스크립트는 그 규칙을 우회하는 것이 아니라 **§9.7 단계 1(PATH)을 미리 채워** 두는
#  것이다. 개발 기계를 세팅하는 일과 배포되는 앱이 무엇을 담느냐는 별개다.
#  여기서 설치해 두면 앱이 조달 흐름에 들어갈 일이 없고, 조달 경로의 SHA-256 값이
#  아직 비어 있다는 문제(docs/OPEN-DECISIONS.md C)도 개발 중에는 비껴간다.
#
set -uo pipefail

cd "$(dirname "$0")/.."

CHECK_ONLY=0
DO_KEYS=1
KEYS_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --check)     CHECK_ONLY=1 ;;
    --no-keys)   DO_KEYS=0 ;;
    --keys-only) KEYS_ONLY=1 ;;
    -h|--help)   sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "알 수 없는 인자: $arg (사용법: --check | --no-keys | --keys-only)" >&2; exit 1 ;;
  esac
done
if [ "$KEYS_ONLY" = 1 ] && [ "$DO_KEYS" = 0 ]; then
  echo "--keys-only 와 --no-keys 를 함께 쓸 수 없습니다" >&2; exit 1
fi
# --keys-only 는 도구 점검·설치를 통째로 건너뛴다.
skip_tools() { [ "$KEYS_ONLY" = 1 ]; }

# ── 출력 ─────────────────────────────────────────────────────────────────────
if [ -t 1 ]; then B=$'\033[1m'; G=$'\033[32m'; Y=$'\033[33m'; R=$'\033[31m'; D=$'\033[2m'; N=$'\033[0m'
else B=""; G=""; Y=""; R=""; D=""; N=""; fi

FAILED=0
WARNED=0
step()  { printf '\n%s── %s%s\n' "$B" "$1" "$N"; }
ok()    { printf '  %s✓%s %s\n' "$G" "$N" "$1"; }
warn()  { printf '  %s!%s %s\n' "$Y" "$N" "$1"; WARNED=$((WARNED+1)); }
fail()  { printf '  %s✗%s %s\n' "$R" "$N" "$1"; FAILED=$((FAILED+1)); }
note()  { printf '    %s%s%s\n' "$D" "$1" "$N"; }
have()  { command -v "$1" >/dev/null 2>&1; }

# 명령을 초 단위 상한과 함께 실행한다.
#
# **왜 필요한가 — macOS 키체인은 대화형 승인을 요구할 수 있다.**
# 바이너리를 다시 빌드하면 서명이 달라져 macOS 가 "soul 이 키체인 항목에 접근하려 합니다"
# 대화상자를 띄운다. 비대화형 실행(CI·`| tee`·에디터 터미널)에서는 아무도 그 창을 누를 수
# 없어 **영원히 멈춘다.** 실제로 이 스크립트를 검증하다 그렇게 멈췄다.
#
# macOS 에는 coreutils 의 `timeout` 이 없으므로 직접 만든다.
run_guarded() {
  local secs="$1"; shift
  "$@" &
  local pid=$!
  local waited=0
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$waited" -ge "$secs" ]; then
      # 셸이 "Terminated: 15" 를 비동기로 찍는다. 사용자에게는 오류처럼 보이므로
      # 죽이는 구간의 stderr 를 통째로 막는다 (진짜 stderr 는 위에서 이미 흘렀다).
      { kill -TERM "$pid"; sleep 1; kill -KILL "$pid"; wait "$pid"; } 2>/dev/null
      return 124
    fi
    sleep 1
    waited=$((waited + 1))
  done
  wait "$pid"
}

# 키체인 접근이 멈추면 무엇을 해야 하는지 알려준다. 조용히 실패시키지 않는다.
keychain_timeout_note() {
  warn "키체인 접근이 ${1}초 안에 끝나지 않았습니다"
  if [ "$PLATFORM" = macos ]; then
    note "macOS 가 승인 대화상자를 띄웠을 수 있습니다 — 화면을 확인하고 '항상 허용'을 누르십시오."
    note "바이너리를 다시 빌드하면 서명이 바뀌어 다시 물어봅니다. 정상입니다."
    note "비대화형 환경이라면 터미널에서 직접 실행하십시오: ./scripts/setup.sh --keys-only"
  else
    note "Secret Service(gnome-keyring 등)가 떠 있는지 확인하십시오."
    note "헤드리스라면 SOUL_ALLOW_ENV_SECRETS=1 로 환경변수를 직접 쓸 수 있습니다."
  fi
}

OS="$(uname -s)"
case "$OS" in
  Darwin) PLATFORM=macos ;;
  Linux)  PLATFORM=linux ;;
  *)      printf '%s\n' "지원하지 않는 OS: $OS" >&2
          printf '%s\n' "Windows 는 WSL2 에서 이 스크립트를 쓰거나 README 의 수동 절차를 따르십시오." >&2
          exit 1 ;;
esac

# ── 설치기 ───────────────────────────────────────────────────────────────────
# 패키지 매니저가 없으면 설치하지 않고 **무엇을 하라고 알려준다.**
# 사용자 기계에 매니저를 임의로 설치하는 것은 이 스크립트의 권한 밖이다.
install_hint() {
  local brew_pkg="$1" apt_pkg="$2"
  if [ "$PLATFORM" = macos ]; then
    note "설치: brew install $brew_pkg"
  else
    note "설치: sudo apt-get install -y $apt_pkg   (또는 배포판 패키지 매니저)"
  fi
}

try_install() {
  local brew_pkg="$1" apt_pkg="$2"
  if [ "$CHECK_ONLY" = 1 ]; then
    install_hint "$brew_pkg" "$apt_pkg"
    return 1
  fi
  if [ "$PLATFORM" = macos ]; then
    if have brew; then
      note "brew install $brew_pkg …"
      brew install "$brew_pkg" >/dev/null 2>&1 && return 0
      return 1
    fi
    note "Homebrew 가 없습니다: https://brew.sh"
    return 1
  fi
  if have apt-get; then
    note "sudo apt-get install -y $apt_pkg …"
    sudo apt-get update -qq >/dev/null 2>&1
    sudo apt-get install -y -qq "$apt_pkg" >/dev/null 2>&1 && return 0
    return 1
  fi
  install_hint "$brew_pkg" "$apt_pkg"
  return 1
}

# ═════════════════════════════════════════════════════════════════════════════
if ! skip_tools; then
step "1/6  Rust 툴체인"

if ! have rustc || ! have cargo; then
  # rustup 은 홈 디렉토리에만 쓰므로 sudo 가 필요 없다.
  if [ "$CHECK_ONLY" = 1 ]; then
    fail "rustc/cargo 없음"
    note "설치: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  else
    note "rustup 으로 stable 툴체인을 설치합니다 (~$HOME/.cargo, sudo 불필요)"
    if curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
         | sh -s -- -y --default-toolchain stable --profile default --no-modify-path >/dev/null 2>&1; then
      ok "rustup 설치 완료"
    else
      fail "rustup 설치 실패"
    fi
  fi
fi

# 이 스크립트 안에서는 항상 보이게 한다 (셸 프로필은 건드리지 않는다).
export PATH="$HOME/.cargo/bin:$PATH"

if have rustc; then
  ok "rustc $(rustc --version | awk '{print $2}')"
  # rust-toolchain.toml 이 채널을 고정하므로 rustup 이 자동으로 맞춘다.
  if have rustup; then
    rustup component add rustfmt clippy >/dev/null 2>&1
    ok "rustfmt · clippy"
  fi
else
  fail "rustc 를 찾지 못했습니다"
  note "새 셸을 열거나: source \$HOME/.cargo/env"
fi

fi

# ═════════════════════════════════════════════════════════════════════════════
if ! skip_tools; then
step "2/6  Node.js (프런트엔드 빌드)"

if have node; then
  NODE_MAJOR="$(node --version | sed 's/^v//' | cut -d. -f1)"
  if [ "${NODE_MAJOR:-0}" -ge 20 ] 2>/dev/null; then
    ok "node $(node --version)"
  else
    warn "node $(node --version) — Vite 6 은 20 이상이 필요합니다"
    note "nvm install 22   (또는 brew upgrade node)"
  fi
else
  fail "node 없음"
  install_hint node nodejs
  note "또는 nvm: https://github.com/nvm-sh/nvm"
fi

fi

# ═════════════════════════════════════════════════════════════════════════════
if ! skip_tools; then
step "3/6  미디어 도구 (§9.7 단계 1 — PATH 를 미리 채운다)"

# ffmpeg — §9.6 영상, §9.5 오디오, §9.4 의 HEIC/AVIF 폴백에 필요하다.
if have ffmpeg && have ffprobe; then
  ok "ffmpeg $(ffmpeg -version 2>/dev/null | head -1 | awk '{print $3}')"
  ok "ffprobe $(ffprobe -version 2>/dev/null | head -1 | awk '{print $3}')"
else
  if try_install ffmpeg ffmpeg && have ffmpeg; then
    ok "ffmpeg $(ffmpeg -version 2>/dev/null | head -1 | awk '{print $3}')"
  else
    fail "ffmpeg/ffprobe 없음 — 영상·오디오 투입 경로가 비활성화됩니다 (§15)"
    note "이미지·텍스트 투입은 ffmpeg 없이도 동작합니다"
  fi
fi

# yt-dlp — §9.3 단계 5. 기본 설정에서는 꺼져 있다(download_enabled = false).
# 없어도 §9.3 단계 6(썸네일+메타데이터, quality: minimal)으로 정상 동작한다.
if have yt-dlp; then
  ok "yt-dlp $(yt-dlp --version 2>/dev/null)"
  note "youtube.download_enabled 는 기본 false 다 — 이용약관 관련이므로 직접 켜야 한다 (§21-4)"
else
  if try_install yt-dlp yt-dlp && have yt-dlp; then
    ok "yt-dlp $(yt-dlp --version 2>/dev/null)"
  else
    warn "yt-dlp 없음 — YouTube 는 썸네일+메타데이터 경로로 처리됩니다 (quality: minimal, §9.3 단계 6)"
    note "이것은 정상 동작이며 오류가 아닙니다 (T11)"
  fi
fi

# Linux 의 Tauri 시스템 의존성. macOS 는 Xcode CLT 로 충분하다.
if [ "$PLATFORM" = linux ]; then
  if pkg-config --exists webkit2gtk-4.1 2>/dev/null; then
    ok "webkit2gtk-4.1 (Tauri)"
  else
    warn "webkit2gtk-4.1 없음 — Tauri 앱 빌드가 실패합니다 (CLI·MCP 는 영향 없음)"
    note "sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf"
  fi
elif ! xcode-select -p >/dev/null 2>&1; then
  warn "Xcode Command Line Tools 없음 — 링크가 실패합니다"
  note "xcode-select --install"
fi

fi

# ═════════════════════════════════════════════════════════════════════════════
if ! skip_tools; then
step "4/6  프로젝트 의존성"

if [ "$CHECK_ONLY" = 1 ]; then
  if [ -d node_modules ]; then ok "node_modules 있음"; else warn "node_modules 없음 (npm install)"; fi
else
  if have npm; then
    note "npm install …"
    if npm install --silent >/dev/null 2>&1; then ok "node_modules"; else fail "npm install 실패"; fi
  fi
  if have cargo; then
    note "cargo fetch … (첫 실행은 몇 분 걸립니다)"
    if cargo fetch --quiet >/dev/null 2>&1; then ok "cargo 의존성"; else warn "cargo fetch 실패 (오프라인?)"; fi
  fi
fi

fi

# ═════════════════════════════════════════════════════════════════════════════
step "5/6  API 키 (§2 — OS 키체인)"

if [ ! -f .env ] && [ -f .env.example ] && [ "$CHECK_ONLY" = 0 ]; then
  cp .env.example .env
  ok ".env 를 만들었습니다 (.env.example 복사)"
  note "키를 채운 뒤 이 스크립트를 다시 실행하십시오"
fi

if [ "$DO_KEYS" = 0 ]; then
  note "--no-keys — 키체인을 건드리지 않습니다"
elif [ ! -f .env ]; then
  warn ".env 가 없습니다"
  note "cp .env.example .env 후 OPENAI_API_KEY 를 채우십시오"
else
  # .env 를 읽되 **이 프로세스 밖으로 내보내지 않는다.**
  # `set -a` 로 export 한 뒤 soul 만 실행하고 즉시 unset 한다.
  set -a
  # shellcheck disable=SC1091
  . ./.env
  set +a

  if [ -z "${OPENAI_API_KEY:-}" ]; then
    warn "OPENAI_API_KEY 가 비어 있습니다 — 모든 투입 경로가 비활성화됩니다 (§15)"
    note ".env 를 열어 채우십시오: https://platform.openai.com/api-keys"
  elif [ "$CHECK_ONLY" = 1 ]; then
    ok "OPENAI_API_KEY 가 .env 에 있습니다 (키체인 반영은 --check 에서 생략)"
  else
    # 빌드가 되어 있어야 soul 을 쓸 수 있다. 없으면 조용히 빌드한다.
    if [ ! -x target/debug/soul ]; then
      note "soul 바이너리를 빌드합니다 …"
      cargo build -p soul-cli --quiet >/dev/null 2>&1
    fi
    if [ -x target/debug/soul ]; then
      # 키를 **키체인으로 옮긴다.** 이후 앱은 .env 를 보지 않는다 (§2).
      run_guarded 60 target/debug/soul secrets import-env
      case $? in
        0)   ok "키를 OS 키체인에 저장했습니다"
             note ".env 의 키는 이제 지워도 됩니다 — 앱은 키체인만 읽습니다" ;;
        124) keychain_timeout_note 60 ;;
        *)   fail "키체인 저장 실패"
             note "헤드리스 환경이라면 SOUL_ALLOW_ENV_SECRETS=1 로 환경변수를 직접 쓸 수 있습니다" ;;
      esac
    else
      warn "soul 바이너리가 없어 키를 옮기지 못했습니다"
    fi
  fi

  # 이 셸에서 키를 지운다. 이후 단계가 실수로 키를 로그에 흘리지 않게 한다.
  unset OPENAI_API_KEY SEARCH_API_KEY YOUTUBE_API_KEY
fi

# ═════════════════════════════════════════════════════════════════════════════
step "6/6  검증"

if [ "$CHECK_ONLY" = 0 ] && have cargo; then
  note "cargo build -p soul-cli -p soul-mcp …"
  cargo build -p soul-cli -p soul-mcp --quiet >/dev/null 2>&1 || warn "빌드 실패"
fi

if [ -x target/debug/soul ]; then
  ok "soul 빌드됨"
  printf '\n'
  # doctor 는 네트워크 없이 로컬 점검만 한다 (--probe 를 주지 않았다).
  # 키체인을 읽으므로 승인 대화상자에 걸릴 수 있다 — 가드를 씌운다.
  DOCTOR_OUT="$(run_guarded 45 target/debug/soul doctor 2>&1)"
  if [ $? -eq 124 ]; then
    keychain_timeout_note 45
  else
    printf '%s\n' "$DOCTOR_OUT" | sed 's/^/  /'
  fi
  printf '\n'
  STATUS_OUT="$(run_guarded 30 target/debug/soul secrets status 2>&1)"
  if [ $? -eq 124 ]; then
    keychain_timeout_note 30
  else
    printf '%s\n' "$STATUS_OUT" | sed 's/^/  /'
  fi
else
  warn "soul 이 아직 빌드되지 않았습니다 (cargo build -p soul-cli)"
fi

# ═════════════════════════════════════════════════════════════════════════════
printf '\n%s────────────────────────────────────────%s\n' "$B" "$N"
if [ "$FAILED" -gt 0 ]; then
  printf '%s실패 %d건%s · 경고 %d건\n' "$R" "$FAILED" "$N" "$WARNED"
  printf '위의 ✗ 항목을 해결한 뒤 다시 실행하십시오.\n'
  exit 1
fi
if [ "$WARNED" -gt 0 ]; then
  printf '%s경고 %d건%s — 해당 경로만 비활성화되고 나머지는 동작합니다.\n' "$Y" "$WARNED" "$N"
else
  printf '%s환경이 준비되었습니다.%s\n' "$G" "$N"
fi
cat <<'NEXT'

다음:
  cargo test --workspace        오프라인 인수 테스트 (§17)
  ./ci/check-deps.sh            크레이트 경계 검사 (§19.4)
  npm run tauri dev             앱
  cargo run -p soul-cli -- doctor --probe    모델 슬롯까지 검증 (네트워크 사용, §9.9)
NEXT
