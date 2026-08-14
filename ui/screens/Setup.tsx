/**
 * 최초 실행 · `doctor` · 설정 (§9.9 · §D7 · §19.7).
 *
 * - `needs_boundary_notice`면 §D2 고지를 먼저 보여준다 (§D7)
 * - 네 슬롯(vision/audio/text/reflect)을 드롭다운으로 고르고, 실패한 슬롯은 사유와 함께
 *   표시하며 **해당 투입 경로만** 비활성화된다는 것을 알린다 (§9.9 4단계)
 * - 목록 조회가 실패하면 자유 입력 필드로 폴백한다
 * - API 키는 `setSecret`으로 넣기만 한다. **다시 읽어오는 UI를 만들지 않는다** (§2).
 *   `secrets_status`의 불리언만 쓴다
 * - `mcp_config_json()`을 복사 버튼과 함께 보여주되 **앱이 설정 파일을 직접 고치지 않는다** (§19.7)
 */
import { useCallback, useEffect, useState } from "react";
import {
  doctor,
  errorText,
  getConfig,
  exportPrompt,
  mcpConfigJson,
  rebuild,
  tracePurge,
  secretsStatus,
  setConfig,
  setSecret,
  setupStatus,
} from "../lib/api";
import {
  dashText,
  EM_DASH,
  MODEL_SLOTS,
  type Config,
  type DoctorReport,
  type ModelSlot,
  type ModelsConfig,
  type SecretStatus,
  type SetupStatus,
} from "../lib/types";
import "../styles/doc.css";

type SlotName = ModelSlot;

/**
 * 슬롯 이름은 **무엇을 담당하는가**로 먼저 읽힌다. `vision` 같은 식별자와 API 이름은
 * 뒤로 물린다 — 모델을 고를 때 알아야 하는 것은 "이게 없으면 무엇이 안 되는가"이지
 * 그것이 Responses API 인지 Chat Completions 인지가 아니다.
 */
const SLOT_LABEL: Record<SlotName, string> = {
  vision: "사진을 보는 모델",
  audio: "소리를 듣는 모델",
  text: "글을 쓰고 합치는 모델",
  reflect: "쌓인 것을 되짚는 모델",
};

const SLOT_TECH: Record<SlotName, string> = {
  vision: "vision · Responses API",
  audio: "audio · input_audio (Chat Completions)",
  text: "text",
  reflect: "reflect",
};

/** §9.9 4단계 — 실패한 슬롯이 막는 경로. 다른 경로는 정상 동작한다 */
const SLOT_DISABLES: Record<SlotName, string> = {
  vision: "이미지를 넣는 것과 영상 화면을 읽는 것이 멈춥니다 (§9.4 · §9.6).",
  audio: "소리를 넣는 것과 YouTube 소리를 읽는 것이 멈춥니다 (§9.5 · §9.3).",
  text: "텍스트를 넣는 것과 여러 갈래를 하나로 합치는 것이 멈춥니다 (§9.2 · §10.3).",
  reflect: "«SOUL» 화면의 성찰 제안이 멈춥니다 (§11.2).",
};

const KNOWN_ACCOUNTS = ["openai_api_key", "search_api_key", "youtube_api_key"];

const ACCOUNT_LABEL: Record<string, string> = {
  openai_api_key: "OpenAI API 키",
  search_api_key: "검색 제공자 API 키",
  youtube_api_key: "YouTube Data API 키 (선택)",
};

function withModel(models: ModelsConfig, slot: SlotName, value: string): ModelsConfig {
  switch (slot) {
    case "vision":
      return { ...models, vision: value };
    case "audio":
      return { ...models, audio: value };
    case "text":
      return { ...models, text: value };
    case "reflect":
      return { ...models, reflect: value };
  }
}

/** §R10 — `null`은 `—`로 렌더한다. 0으로 대체하거나 항목을 생략하지 않는다 */
function Flag({ ok }: { ok: boolean | null }) {
  if (ok === null) return <span className="doc-muted">{EM_DASH}</span>;
  return <span className={ok ? "doc-ok" : "doc-bad"}>{ok ? "정상" : "실패"}</span>;
}

function Text({ v }: { v: string | null }) {
  const s = dashText(v);
  return <span className={s === EM_DASH ? "doc-muted" : undefined}>{s}</span>;
}

/** 설치 안내를 띄울 플랫폼. 표시용이므로 userAgent 로 충분하다. */
function guessPlatform(): "macos" | "windows" | "linux" {
  const ua = navigator.userAgent;
  if (ua.includes("Mac")) return "macos";
  if (ua.includes("Win")) return "windows";
  return "linux";
}

const INSTALL: Record<string, Record<"macos" | "windows" | "linux", string>> = {
  ffmpeg: {
    macos: "brew install ffmpeg",
    windows: "winget install Gyan.FFmpeg",
    linux: "sudo apt-get install ffmpeg",
  },
  "yt-dlp": {
    macos: "brew install yt-dlp",
    windows: "winget install yt-dlp.yt-dlp",
    linux: "sudo apt-get install yt-dlp",
  },
};

/**
 * 외부 도구 한 줄. **없을 때 무엇이 멈추는지와 어떻게 고치는지를 함께 적는다.**
 *
 * §9.7·§20.8 — 앱은 ffmpeg 과 yt-dlp 를 번들하지 않는다. 즉 이 앱을 다른 기계로
 * 옮기면 거기에는 없을 수 있다. 그때 표에 `—` 만 뜨면 사용자는 무엇이 왜 안 되는지
 * 알 수 없다. 진단 화면은 그것을 말해 주는 자리다.
 *
 * `missing` 은 **없어도 정상인가**를 뜻한다. yt-dlp 는 없어도 §9.3 단계 6 으로
 * 내려가 `quality: minimal` 로 기록될 뿐이라 오류가 아니다 (T11).
 */
function ToolRow({
  name,
  version,
  consequence,
  fatal,
}: {
  name: string;
  version: string | null;
  consequence: string;
  fatal: boolean;
}) {
  const missing = version === null || version === "";
  const cmd = INSTALL[name]?.[guessPlatform()];
  return (
    <tr>
      <th scope="row">{name}</th>
      <td>
        <Text v={version} />
        {missing && (
          <div className={fatal ? "doc-tool-warn" : "doc-muted doc-legend"}>
            {consequence}
            {cmd && (
              <>
                {" "}
                설치: <code>{cmd}</code>
              </>
            )}
          </div>
        )}
      </td>
    </tr>
  );
}

export function Setup() {
  const [status, setStatus] = useState<SetupStatus | null>(null);
  const [cfg, setCfg] = useState<Config | null>(null);
  const [savedCfg, setSavedCfg] = useState<string>("");
  const [secrets, setSecrets] = useState<SecretStatus[]>([]);
  const [keyInput, setKeyInput] = useState<Record<string, string>>({});
  const [report, setReport] = useState<DoctorReport | null>(null);
  const [manual, setManual] = useState<Record<string, boolean>>({});
  const [mcpJson, setMcpJson] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const [probing, setProbing] = useState(false);
  const [busy, setBusy] = useState(false);
  /** 유지보수 동작 중인 항목 id. 한 번에 하나만 돈다. */
  const [maint, setMaint] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const refreshSecrets = useCallback(async () => {
    try {
      const s = await secretsStatus();
      setSecrets(s);
    } catch {
      setSecrets([]);
    }
  }, []);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await setupStatus());
    } catch (e) {
      setError(errorText(e));
    }
  }, []);

  useEffect(() => {
    void refreshStatus();
    void refreshSecrets();
    getConfig()
      .then((c) => {
        setCfg(c);
        setSavedCfg(JSON.stringify(c));
      })
      .catch((e) => setError(errorText(e)));
    mcpConfigJson()
      .then(setMcpJson)
      .catch(() => setMcpJson(null));
  }, [refreshStatus, refreshSecrets]);

  const cfgDirty = cfg !== null && JSON.stringify(cfg) !== savedCfg;

  /**
   * 유지보수 동작 하나를 돌린다. 한 번에 하나만 — 재빌드 중에 트레이스를 지우는 것 같은
   * 겹침을 막는다 (쓰기 락은 Rust 가 잡지만, 그 실패를 사용자에게 보이느니 애초에 막는다).
   */
  async function runMaintenance(id: string, run: () => Promise<string | void>) {
    setMaint(id);
    setError(null);
    setNote(null);
    try {
      const msg = await run();
      setNote(typeof msg === "string" ? msg : "완료했습니다.");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setMaint(null);
    }
  }

  async function runDoctor(probeModels: boolean) {
    setProbing(true);
    setError(null);
    setNote(null);
    try {
      setReport(await doctor(probeModels));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setProbing(false);
    }
  }

  async function saveConfig() {
    if (cfg === null) return;
    setBusy(true);
    setError(null);
    try {
      await setConfig(cfg);
      setSavedCfg(JSON.stringify(cfg));
      setNote("설정을 저장했습니다.");
      await refreshStatus();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  async function saveKey(account: string) {
    const value = keyInput[account] ?? "";
    if (value === "") return;
    setBusy(true);
    setError(null);
    try {
      await setSecret(account, value);
      setKeyInput({ ...keyInput, [account]: "" });
      setNote(`${ACCOUNT_LABEL[account] ?? account}를 키체인에 저장했습니다.`);
      await refreshSecrets();
      await refreshStatus();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  async function copyMcp() {
    if (mcpJson === null) return;
    try {
      await navigator.clipboard.writeText(mcpJson);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setError("복사하지 못했습니다. 아래 상자에서 직접 선택해 복사하세요.");
    }
  }

  // §D7 고지는 셸(`App.tsx`)이 게이트로 세운다. 여기서 다시 띄우지 않는다.

  const accounts: string[] =
    secrets.length > 0 ? secrets.map(([a]) => a) : KNOWN_ACCOUNTS;
  const secretMap = new Map(secrets);
  const models = report?.models_available ?? [];
  const listOk = models.length > 0;
  const slotChecks = new Map((report?.slots ?? []).map((s) => [s.slot, s]));

  return (
    <section className="doc doc-setup">
      <header className="doc-head">
        <h1 className="doc-title">설정</h1>
        <div className="doc-actions">
          <button
            type="button"
            className="doc-btn"
            onClick={() => void runDoctor(false)}
            disabled={probing}
          >
            {probing ? "검사 중…" : "빠른 검사"}
          </button>
          <button
            type="button"
            className="doc-btn"
            onClick={() => void runDoctor(true)}
            disabled={probing}
          >
            모델 검증
          </button>
        </div>
      </header>

      {status !== null && status.models_unset && (
        <p className="doc-warn">
          쓸 모델이 정해지지 않아 <strong>아무것도 넣을 수 없습니다.</strong> 아래 네 자리를
          채워 주세요.
        </p>
      )}
      {error !== null && (
        <p className="doc-error" role="alert">
          {error}
        </p>
      )}
      {note !== null && (
        <p className="doc-result" role="status">
          {note}
        </p>
      )}

      {/* ── API 키 ─────────────────────────────────────────── */}
      <div className="doc-panel">
        <h2 className="doc-h2">API 키</h2>
        <p className="doc-muted doc-legend">
          키는 이 컴퓨터의 키체인에 저장됩니다. <strong>앱은 저장한 키를 다시 읽어 보여주지
          않습니다</strong> — 설정됐는지 아닌지만 표시합니다.
        </p>
        <ul className="doc-keys">
          {accounts.map((a) => (
            <li key={a} className="doc-key">
              <div className="doc-key-head">
                <span className="doc-key-label">{ACCOUNT_LABEL[a] ?? a}</span>
                <span className={secretMap.get(a) ? "doc-ok" : "doc-muted"}>
                  {secretMap.get(a) ? "설정됨" : "설정 안 됨"}
                </span>
              </div>
              <div className="doc-key-row">
                <input
                  className="doc-input"
                  type="password"
                  autoComplete="off"
                  placeholder="새 키 입력"
                  value={keyInput[a] ?? ""}
                  onChange={(e) => setKeyInput({ ...keyInput, [a]: e.target.value })}
                />
                <button
                  type="button"
                  className="doc-btn"
                  onClick={() => void saveKey(a)}
                  disabled={busy || (keyInput[a] ?? "") === ""}
                >
                  저장
                </button>
              </div>
            </li>
          ))}
        </ul>
      </div>

      {/* ── 모델 슬롯 ───────────────────────────────────────── */}
      <div className="doc-panel">
        <h2 className="doc-h2">어떤 모델을 쓸까</h2>
        <p className="doc-muted doc-legend">
          {listOk
            ? "쓰고 있는 계정에서 가져온 목록입니다. 고르고 저장한 다음 «모델 검증»을 누르면 자리마다 한 번씩 실제로 불러 보고 되는지 확인합니다."
            : "모델 목록을 아직 가져오지 못했습니다. 모델 이름을 직접 적어 주세요."}
        </p>
        {cfg === null ? (
          <p className="doc-empty">설정을 읽는 중…</p>
        ) : (
          <ul className="doc-slots">
            {MODEL_SLOTS.map((slot) => {
              const check = slotChecks.get(slot) ?? null;
              const value = cfg.models[slot];
              const useManual = !listOk || manual[slot] === true;
              return (
                <li key={slot} className="doc-slot">
                  <div className="doc-slot-head">
                    <span className="doc-slot-label">
                      {SLOT_LABEL[slot]}
                      <code className="ident">{SLOT_TECH[slot]}</code>
                    </span>
                    {check !== null && <Flag ok={check.ok} />}
                  </div>
                  <div className="doc-slot-row">
                    {useManual ? (
                      <input
                        className="doc-input"
                        type="text"
                        spellCheck={false}
                        placeholder="모델 ID"
                        value={value}
                        onChange={(e) =>
                          setCfg({ ...cfg, models: withModel(cfg.models, slot, e.target.value) })
                        }
                      />
                    ) : (
                      <select
                        className="doc-input"
                        value={value}
                        onChange={(e) =>
                          setCfg({ ...cfg, models: withModel(cfg.models, slot, e.target.value) })
                        }
                      >
                        <option value="">(선택 안 됨)</option>
                        {models.includes(value) || value === "" ? null : (
                          <option value={value}>{value}</option>
                        )}
                        {models.map((m) => (
                          <option key={m} value={m}>
                            {m}
                          </option>
                        ))}
                      </select>
                    )}
                    {listOk && (
                      <button
                        type="button"
                        className="doc-btn doc-btn-quiet"
                        onClick={() => setManual({ ...manual, [slot]: !useManual })}
                      >
                        {useManual ? "목록에서 고르기" : "직접 입력"}
                      </button>
                    )}
                  </div>
                  {check !== null && !check.ok && (
                    <p className="doc-warn">
                      <Text v={check.error} /> — {SLOT_DISABLES[slot]}{" "}
                      <span className="doc-muted">다른 경로는 정상 동작합니다.</span>
                    </p>
                  )}
                </li>
              );
            })}
          </ul>
        )}
        <div className="doc-actions">
          <button
            type="button"
            className="doc-btn doc-btn-primary"
            onClick={() => void saveConfig()}
            disabled={busy || !cfgDirty}
          >
            설정 저장
          </button>
        </div>
      </div>

      {/* ── 문화 층 ─────────────────────────────────────────── */}
      {cfg !== null && (
        <div className="doc-panel">
          <h2 className="doc-h2">문화 글귀</h2>
          <label className="doc-check">
            <input
              type="checkbox"
              checked={cfg.thresholds.context_enabled}
              onChange={(e) =>
                setCfg({
                  ...cfg,
                  thresholds: { ...cfg.thresholds, context_enabled: e.target.checked },
                })
              }
            />
            <span>
              <span className="doc-radio-title">왜 끌렸을지도 같이 짐작하게 한다</span>
              <span className="doc-muted">
                켜 두면 <strong>무언가 넣을 때마다 웹 검색이 일어납니다</strong> (§D6). 끄면 검색
                기록이 남을 일이 사라지는 대신, 문화 글귀와 «네 칸» 전체가 없어집니다.
              </span>
            </span>
          </label>
          <div className="doc-actions">
            <button
              type="button"
              className="doc-btn doc-btn-primary"
              onClick={() => void saveConfig()}
              disabled={busy || !cfgDirty}
            >
              설정 저장
            </button>
          </div>
        </div>
      )}

      {/* ── 진단 ───────────────────────────────────────────── */}
      <div className="doc-panel">
        <h2 className="doc-h2">진단 — 지금 무엇이 되고 무엇이 안 되나</h2>
        {report === null ? (
          <p className="doc-empty">아직 검사하지 않았습니다. 위의 «빠른 검사»를 누르세요.</p>
        ) : (
          <>
            <table className="doc-table doc-table-kv">
              <tbody>
                <tr>
                  <th scope="row">API 키</th>
                  <td>
                    <Flag ok={report.api_key_set} />
                  </td>
                </tr>
                <tr>
                  <th scope="row">git</th>
                  <td>
                    <Flag ok={report.git_ok} />
                  </td>
                </tr>
                <tr>
                  <th scope="row">SOUL.md</th>
                  <td>
                    <Flag ok={report.soul_md_ok} />
                  </td>
                </tr>
                <ToolRow
                  name="ffmpeg"
                  version={report.ffmpeg}
                  fatal
                  consequence="영상·오디오 투입 경로가 비활성화됩니다 (§15). 이미지·텍스트는 그대로 동작합니다."
                />
                <ToolRow
                  name="ffprobe"
                  version={report.ffprobe}
                  fatal
                  consequence="ffmpeg 과 함께 설치됩니다. 없으면 kind 판별이 확장자에 의존하게 됩니다 (§9.1)."
                />
                <ToolRow
                  name="yt-dlp"
                  version={report.ytdlp}
                  fatal={false}
                  consequence="없어도 정상입니다 — YouTube 는 썸네일+메타데이터 경로로 처리되어 quality: minimal 로 기록됩니다 (§9.3 단계 6)."
                />
                <tr>
                  <th scope="row">임베딩</th>
                  <td>
                    <Flag ok={report.embed_ok} />
                    {report.embed_error !== null && (
                      <span className="doc-muted"> · {report.embed_error}</span>
                    )}
                  </td>
                </tr>
                <tr>
                  <th scope="row">사진 여러 장 한 번에</th>
                  <td>
                    <Flag ok={report.multi_image_ok} />
                    {report.multi_image_ok === false && (
                      <span className="doc-muted">
                        {" "}
                        · <code className="doc-code">video_max_frames</code>를 낮춰 주세요 (§9.6)
                      </span>
                    )}
                  </td>
                </tr>
              </tbody>
            </table>
            {report.slots.length > 0 && (
              <div className="doc-tablewrap">
                <table className="doc-table">
                  <thead>
                    <tr>
                      <th scope="col">자리</th>
                      <th scope="col">모델</th>
                      <th scope="col">결과</th>
                      <th scope="col">왜</th>
                    </tr>
                  </thead>
                  <tbody>
                    {report.slots.map((s) => (
                      <tr key={s.slot}>
                        <td>
                          {SLOT_LABEL[s.slot as SlotName] ?? s.slot}
                          <code className="ident">{s.slot}</code>
                        </td>
                        <td>
                          <Text v={s.model} />
                        </td>
                        <td>
                          <Flag ok={s.ok} />
                        </td>
                        <td>
                          <Text v={s.error} />
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </>
        )}
      </div>

      {/* ── MCP ────────────────────────────────────────────── */}
      <div className="doc-panel">
        <h2 className="doc-h2">내 컴퓨터의 다른 AI 도구에 물리기 (MCP)</h2>
        <p className="doc-muted doc-legend">
          아래 내용을 쓰는 AI 도구의 설정 파일에 직접 붙여 넣으세요. 그러면 그 도구가 여기 쌓인
          것을 읽을 수 있습니다. 도구마다 파일 위치가 다르므로 <strong>앱이 남의 설정 파일을
          고치지 않습니다</strong> (§19.7).
        </p>
        {mcpJson === null ? (
          <p className="doc-empty">—</p>
        ) : (
          <>
            <textarea className="doc-textarea doc-mono" readOnly value={mcpJson} rows={6} />
            <div className="doc-actions">
              <button type="button" className="doc-btn" onClick={() => void copyMcp()}>
                {copied ? "복사했습니다" : "복사"}
              </button>
            </div>
          </>
        )}
      </div>

      {/*
        §11.3 · §14 — 명세는 이 셋을 CLI 명령으로 정의하지만, GUI 만 쓰는 사람에게는
        **트레이스를 지울 수단이 아예 없어진다.** §11.3 은 "사용자가 지울 수단을 반드시
        제공한다"고 못박으므로, 최소한 그 하나는 앱에도 있어야 한다.
        나머지 둘은 같은 성격(로컬 유지보수)이라 같은 자리에 둔다.
      */}
      <div className="doc-card">
        <h2 className="doc-h2">유지보수</h2>

        <div className="doc-maint">
          <div className="doc-maint-row">
            <div>
              <strong>에이전트 트레이스 삭제</strong>
              <p className="doc-muted doc-legend">
                모델에게 무엇을 물었고 무엇을 받았는지가 <strong>그대로 읽히는 글자로</strong>{" "}
                <code>runs/</code> 에 남습니다. 이 컴퓨터 안의 파일이지만 지울 수 있어야 하므로
                여기 둡니다 (§11.3). 쌓인 답과 SOUL.md 는 건드리지 않습니다.
              </p>
            </div>
            <button
              type="button"
              className="doc-btn"
              disabled={maint !== null}
              onClick={() => void runMaintenance("trace", async () => {
                const n = await tracePurge();
                return `트레이스 ${n}건을 지웠습니다.`;
              })}
            >
              {maint === "trace" ? "지우는 중…" : "지우기"}
            </button>
          </div>

          <div className="doc-maint-row">
            <div>
              <strong>재빌드</strong>
              <p className="doc-muted doc-legend">
                쌓인 답을 처음부터 순서대로 다시 훑어 숫자와 SOUL.md 를 새로 만듭니다 (§R2).
                직접 쓴 <code>soul:human</code> 부분은 그대로 옮겨집니다.
              </p>
            </div>
            <button
              type="button"
              className="doc-btn"
              disabled={maint !== null}
              onClick={() => void runMaintenance("rebuild", () => rebuild(false, false))}
            >
              {maint === "rebuild" ? "재빌드 중…" : "실행"}
            </button>
          </div>

          <div className="doc-maint-row">
            <div>
              <strong>프롬프트로 내보내기</strong>
              <p className="doc-muted doc-legend">
                <code>SOUL.md</code> 에서 표시용 주석만 떼어 <code>exports/</code> 에 파일로
                씁니다. 다른 AI 에 통째로 붙여 넣을 때 씁니다. <strong>다만 이건 줄어든
                형태입니다</strong> — 위의 MCP 로 물리는 편이 훨씬 낫습니다 (§19.1).
              </p>
            </div>
            <button
              type="button"
              className="doc-btn"
              disabled={maint !== null}
              onClick={() => void runMaintenance("export", async () => {
                const t = await exportPrompt();
                return `내보냈습니다 (${t.length}자). exports/SOUL.prompt.md`;
              })}
            >
              {maint === "export" ? "내보내는 중…" : "내보내기"}
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}

export default Setup;
