#!/usr/bin/env bash
# 크레이트 의존 규칙 강제 (§19.4 · T32 · T34).
#
# soul-core 와 soul-mcp 에 네트워크 클라이언트가 링크되면 "적용은 로컬"이라는
# 약속(§D1)이 구조적으로 깨진다. 이 검사는 그것을 막는다.
set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

fail=0
banned='^(reqwest|hyper|hyper-util|h2|isahc|ureq|curl|surf|attohttpc|tokio-tungstenite)$'

for crate in soul-core soul-mcp; do
  echo "== $crate =="
  tree=$(cargo tree -p "$crate" --edges normal --prefix none 2>/dev/null | awk '{print $1}' | sort -u)
  hits=$(echo "$tree" | grep -E "$banned" || true)
  if [ -n "$hits" ]; then
    echo "  FAIL — 금지된 네트워크 의존성:"
    echo "$hits" | sed 's/^/    /'
    fail=1
  else
    echo "  ok — 네트워크 클라이언트 없음"
  fi
done

# soul-mcp 는 soul-core 외의 앱 크레이트를 의존하지 않는다.
echo "== soul-mcp 앱 크레이트 의존 =="
bad=$(grep -oE '^soul-(media|net|agent|pipeline|cli)' crates/soul-mcp/Cargo.toml || true)
if [ -n "$bad" ]; then
  echo "  FAIL — soul-mcp 가 의존하면 안 되는 크레이트: $bad"
  fail=1
else
  echo "  ok"
fi

# prompts/*.md 는 UTF-8 · BOM 없음 · LF (§R11 · T24c)
echo "== prompts 인코딩 =="
enc_ok=1
for f in prompts/*.md; do
  if head -c3 "$f" | od -An -tx1 | grep -q "ef bb bf"; then echo "  FAIL BOM: $f"; fail=1; enc_ok=0; fi
  if LC_ALL=C grep -q $'\r' "$f"; then echo "  FAIL CRLF: $f"; fail=1; enc_ok=0; fi
  # macOS 의 iconv 는 리다이렉트 상황에서 오탐이 있어 python3 로 검사한다.
  if ! python3 -c "import sys; open(sys.argv[1],'rb').read().decode('utf-8')" "$f" 2>/dev/null; then
    echo "  FAIL not-utf8: $f"; fail=1; enc_ok=0
  fi
done
[ $enc_ok -eq 1 ] && echo "  ok"

exit $fail
