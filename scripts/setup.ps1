# tasty-soul 개발 환경 부트스트랩 (Windows 10+).
#
#   pwsh -File scripts\setup.ps1
#   pwsh -File scripts\setup.ps1 -Check      # 설치 없이 진단만
#   pwsh -File scripts\setup.ps1 -KeysOnly   # 키만 다시 넣는다
#
# 멱등하다. `scripts/setup.sh` 의 Windows 대응물이며 같은 것을 확인한다.
#
# §9.7·§20.8 — 앱은 ffmpeg 을 번들하지 않는다. 이 스크립트는 §9.7 단계 1(PATH)을
# 미리 채워 두는 것이지 그 규칙을 우회하는 것이 아니다.

[CmdletBinding()]
param(
    [switch]$Check,
    [switch]$NoKeys,
    [switch]$KeysOnly
)

$ErrorActionPreference = 'Continue'
Set-Location (Join-Path $PSScriptRoot '..')

$script:Failed = 0
$script:Warned = 0

function Step($t) { Write-Host "`n── $t" -ForegroundColor White }
function Ok($t)   { Write-Host "  ✓ $t" -ForegroundColor Green }
function Warn($t) { Write-Host "  ! $t" -ForegroundColor Yellow; $script:Warned++ }
function Fail($t) { Write-Host "  ✗ $t" -ForegroundColor Red;    $script:Failed++ }
function Note($t) { Write-Host "    $t" -ForegroundColor DarkGray }
function Have($c) { $null -ne (Get-Command $c -ErrorAction SilentlyContinue) }

# winget 이 없으면 설치하지 않고 무엇을 하라고 알려준다.
function TryInstall($tool, $wingetId) {
    if ($Check) { Note "설치: winget install $wingetId"; return $false }
    if (-not (Have 'winget')) {
        Note "winget 이 없습니다. 수동 설치: $tool"
        return $false
    }
    Note "winget install $wingetId …"
    winget install --id $wingetId --silent --accept-source-agreements --accept-package-agreements 2>&1 | Out-Null
    # winget 은 현재 세션의 PATH 를 갱신하지 않는다.
    $env:PATH = [Environment]::GetEnvironmentVariable('PATH', 'Machine') + ';' +
                [Environment]::GetEnvironmentVariable('PATH', 'User')
    return (Have $tool)
}

if (-not $KeysOnly) {
    # ═══════════════════════════════════════════════════════════════════════
    Step '1/6  Rust 툴체인'
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
    if (-not (Have 'rustc')) {
        if ($Check) {
            Fail 'rustc 없음'
            Note '설치: https://rustup.rs (rustup-init.exe)'
        } else {
            Note 'rustup-init 을 내려받아 실행합니다 …'
            $tmp = Join-Path $env:TEMP 'rustup-init.exe'
            try {
                Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile $tmp -UseBasicParsing
                & $tmp -y --default-toolchain stable --profile default | Out-Null
                $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
                Ok 'rustup 설치 완료'
            } catch { Fail "rustup 설치 실패: $_" }
        }
    }
    if (Have 'rustc') {
        Ok "rustc $((rustc --version) -split ' ')[1]"
        if (Have 'rustup') { rustup component add rustfmt clippy 2>&1 | Out-Null }
    } else {
        Fail 'rustc 를 찾지 못했습니다 — 새 터미널을 여십시오'
    }

    # ═══════════════════════════════════════════════════════════════════════
    Step '2/6  Node.js'
    if (Have 'node') {
        $major = [int](((node --version) -replace '^v','') -split '\.')[0]
        if ($major -ge 20) { Ok "node $(node --version)" }
        else { Warn "node $(node --version) — Vite 6 은 20 이상이 필요합니다" }
    } else {
        if (TryInstall 'node' 'OpenJS.NodeJS.LTS') { Ok "node $(node --version)" }
        else { Fail 'node 없음' }
    }

    # ═══════════════════════════════════════════════════════════════════════
    Step '3/6  미디어 도구 (§9.7 단계 1)'
    if ((Have 'ffmpeg') -and (Have 'ffprobe')) {
        Ok 'ffmpeg · ffprobe'
    } elseif (TryInstall 'ffmpeg' 'Gyan.FFmpeg') {
        Ok 'ffmpeg · ffprobe'
    } else {
        Fail 'ffmpeg/ffprobe 없음 — 영상·오디오 투입 경로가 비활성화됩니다 (§15)'
        Note '이미지·텍스트 투입은 ffmpeg 없이도 동작합니다'
    }

    if (Have 'yt-dlp') {
        Ok "yt-dlp $(yt-dlp --version)"
        Note 'youtube.download_enabled 는 기본 false 다 — 직접 켜야 한다 (§21-4)'
    } elseif (TryInstall 'yt-dlp' 'yt-dlp.yt-dlp') {
        Ok "yt-dlp $(yt-dlp --version)"
    } else {
        Warn 'yt-dlp 없음 — YouTube 는 썸네일+메타데이터 경로로 처리됩니다 (quality: minimal)'
        Note '이것은 정상 동작이며 오류가 아닙니다 (T11)'
    }

    # Tauri 는 WebView2 를 쓴다. Windows 11 에는 기본 포함이다.
    $wv = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\*' -ErrorAction SilentlyContinue |
          Where-Object { $_.pv -and $_.name -like '*WebView2*' }
    if ($wv) { Ok 'WebView2 런타임' }
    else { Warn 'WebView2 런타임을 확인하지 못했습니다 — Tauri 앱 실행에 필요합니다' }

    # ═══════════════════════════════════════════════════════════════════════
    Step '4/6  프로젝트 의존성'
    if ($Check) {
        if (Test-Path node_modules) { Ok 'node_modules 있음' } else { Warn 'node_modules 없음' }
    } else {
        if (Have 'npm')   { npm install --silent 2>&1 | Out-Null; Ok 'node_modules' }
        if (Have 'cargo') { cargo fetch --quiet 2>&1 | Out-Null; Ok 'cargo 의존성' }
    }
}

# ═══════════════════════════════════════════════════════════════════════════
Step '5/6  API 키 (§2 — Windows 자격 증명 관리자)'

if ((-not (Test-Path .env)) -and (Test-Path .env.example) -and (-not $Check)) {
    Copy-Item .env.example .env
    Ok '.env 를 만들었습니다 (.env.example 복사)'
    Note '키를 채운 뒤 이 스크립트를 다시 실행하십시오'
}

if ($NoKeys) {
    Note '-NoKeys — 자격 증명 저장소를 건드리지 않습니다'
} elseif (-not (Test-Path .env)) {
    Warn '.env 가 없습니다'
} else {
    # .env 를 이 프로세스에만 반영한다.
    Get-Content .env | ForEach-Object {
        if ($_ -match '^\s*([A-Z_][A-Z0-9_]*)\s*=\s*(.*)$') {
            $v = $Matches[2].Trim().Trim('"').Trim("'")
            if ($v) { Set-Item -Path "Env:$($Matches[1])" -Value $v }
        }
    }
    if (-not $env:OPENAI_API_KEY) {
        Warn 'OPENAI_API_KEY 가 비어 있습니다 — 모든 투입 경로가 비활성화됩니다 (§15)'
    } elseif ($Check) {
        Ok 'OPENAI_API_KEY 가 .env 에 있습니다'
    } else {
        if (-not (Test-Path 'target\debug\soul.exe')) {
            Note 'soul 을 빌드합니다 …'
            cargo build -p soul-cli --quiet 2>&1 | Out-Null
        }
        if (Test-Path 'target\debug\soul.exe') {
            & target\debug\soul.exe secrets import-env
            if ($LASTEXITCODE -eq 0) {
                Ok '키를 자격 증명 관리자에 저장했습니다'
                Note '.env 의 키는 이제 지워도 됩니다 — 앱은 자격 증명 저장소만 읽습니다'
            } else { Fail '키 저장 실패' }
        }
    }
    Remove-Item Env:OPENAI_API_KEY, Env:SEARCH_API_KEY, Env:YOUTUBE_API_KEY -ErrorAction SilentlyContinue
}

# ═══════════════════════════════════════════════════════════════════════════
Step '6/6  검증'
if ((-not $Check) -and (Have 'cargo')) {
    cargo build -p soul-cli -p soul-mcp --quiet 2>&1 | Out-Null
}
if (Test-Path 'target\debug\soul.exe') {
    Ok 'soul 빌드됨'
    Write-Host ''
    & target\debug\soul.exe doctor
    Write-Host ''
    & target\debug\soul.exe secrets status
} else {
    Warn 'soul 이 아직 빌드되지 않았습니다'
}

Write-Host "`n────────────────────────────────────────" -ForegroundColor White
if ($script:Failed -gt 0) {
    Write-Host "실패 $($script:Failed)건 · 경고 $($script:Warned)건" -ForegroundColor Red
    exit 1
}
if ($script:Warned -gt 0) {
    Write-Host "경고 $($script:Warned)건 — 해당 경로만 비활성화되고 나머지는 동작합니다." -ForegroundColor Yellow
} else {
    Write-Host '환경이 준비되었습니다.' -ForegroundColor Green
}
@'

다음:
  cargo test --workspace        오프라인 인수 테스트 (§17)
  npm run tauri dev             앱
  cargo run -p soul-cli -- doctor --probe    모델 슬롯까지 검증 (네트워크 사용, §9.9)
'@ | Write-Host
