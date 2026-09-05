# remote surface (LAN, poll-based) — 실행 계획

> 브리프(메인) → `plan-drafter`(Opus) 초안 → `plan-critic`(Opus) 반증 → 메인 취합 →
> **`/peer-review` codex(`gpt-6-astra`, effort ultra) 플랜 리뷰 → 메인 재취합**. 2026-09-05.
> 반증 판정 revise, codex 판정 "findings 있음". §6 에 두 리뷰의 findings 별 채택·기각과 근거.
> 메인이 새로 설계한 구조(어느 리뷰도 그 최종 형태를 보지 못한 것): §3.3 의 세션 토큰
> (`<epoch>:<id>`), §3.6 의 입력 인코더(xterm `disableStdin` + `term.modes`)와 입력 FIFO, §3.6 의
> reset 상태 전이. 구현 리뷰에서 이 셋을 먼저 본다.

## 0. 요지

같은 공유기의 휴대폰 브라우저에서 winmux 의 워크스페이스·pane·탭 상태를 보고, 터미널 탭의
현재 화면을 읽고, 그 탭에 텍스트를 보낸다. 폴링 request/response, 스트리밍·푸시 없음. winmux 는
**기본 꺼짐이고 켜지 않으면 아무것도 존재하지 않는다**(ADR-0014 규율).

구조: HTTP 서버는 winmux 프로세스 안(Windows 쪽)에 살되 **새 크레이트 `crates/winmux-remote`**(순수
Rust, Tauri 무의존)가 소켓·파싱·인증·라우팅을 소유하고 Linux 에서 `cargo test` 로 잠긴다. 글루는
설정·토큰 파일·서버 spawn·자산 콜백·커맨드 2개만. 폰 페이지는 두 번째 Vite 빌드(`dist/remote/`)를
winmux 가 직접 서빙한다. 버전 0.3.17 (0.3.16 은 `winmux send` 의 CR 분리 수정 — 별도 PR).

가장 큰 결정 두 가지:
- **HTTP 는 `std::net::TcpListener` + `httparse`** 로 크레이트가 커넥션 루프를 직접 소유한다.
  브리프가 후보로 든 `tiny_http` 는 요청 헤드 상한이 없고(`client.rs:79-101`) 본문 거절 후 drop 이
  선언 길이만큼 `vec![0; n]` 을 반복 할당하며 끝까지 읽어(`util/equal_reader.rs:66-86`, 메인이 소스로
  재확인) "읽기 전 413" 이 원리적으로 불가능하다. `httparse 1.10.1`·`getrandom 0.3.4`·`base64 0.22.1`
  은 `Cargo.lock` 에 이미 있어 서드파티 패키지 추가는 0 이다(`winmux-remote` 항목과 의존 엣지는 늘어난다).
- **정적 자산은 임베드 키 집합으로 게이트**한다. Tauri release 의 자산 조회는 없는 경로를
  `index.html` 로 폴백하므로(`tauri-2.11.5 manager/mod.rs:406-428`) 게이트 없이는 `/remote/오타` 가
  데스크톱 페이지를 200 으로 돌려준다. 임베드 키는 선행 `/` 가 붙어 있으므로(`tauri-utils-2.9.3
  assets.rs:44-55`) 떼고 비교한다.

## 1. 목표 · 범위 울타리

브리프 §1·§5 그대로. 이번 작업에서 **하지 않는 것**: TLS·자체서명, Tailscale 통합, 푸시 알림,
WebSocket/스트리밍/long-poll, 폰에서 resize, 폰에서 워크스페이스·탭 생성/전환/닫기, 데스크톱 active
상태 변경, 토큰 UI 재발급·다중 토큰·만료, IPv6, 네이티브 앱, `winmux ls`/`send` 변경, 설정 UI,
스크롤백 플레인 텍스트 추출, attach nudge 대행, mDNS, keep-alive, 인증된 클라이언트 rate limit,
IP 별 커넥션 상한, `kill()` 의 writer 대기 구조 변경. 필요해 보이면 §8 후속에 적고 구현하지 않는다.

## 2. 아키텍처 결정 (확정)

브리프 §2 의 9개 결정에 취합에서 바뀐 것만 표시한다.

1. 프로세스 내 HTTP 서버(Windows 쪽). 변경 없음.
2. 새 크레이트 `crates/winmux-remote`, `Arc<Mutex<Dispatcher>>` + `Arc<SessionManager>` 수신.
   **정정**: 코어 테스트의 `FakeSessionHost` 는 `#[cfg(test)] mod tests` 안의 비공개 타입이라
   (`command.rs:1694, 1716`) 외부 크레이트가 쓸 수 없다. 원격 크레이트 테스트는 **자체 fake host**
   를 둔다 — `SessionHost` 는 `spawn_shell`·`kill` 두 개만 구현하면 되고 `release_tabs` 는 기본
   구현이 있다(`command.rs:352-375`).
3. 동기 HTTP, 스레드 기반, async 런타임 없음. **확정: `httparse` 위에 커넥션 루프를 직접 쓴다**
   (≈250줄). 이유는 §0. 메인 승인 사항.
4. 폰 UI 는 웹, winmux 직접 서빙, Tauri `AssetResolver`. **정정 2건**: (a) dev/release 가정이
   반대다 — dev 는 `../dist/<path>` 를 `fs::read` 해 없으면 정직하게 `None`(`app.rs:347-369`),
   release 는 없는 경로에 절대 `None` 을 주지 않는다 → 임베드 키 게이트 필수. (b) 파일은
   **`apps/winmux/remote/index.html`** + 두 번째 Vite 설정 — 루트의 `remote.html` 은 `dist/remote.html`
   로 나와 `/remote/*` 규약과 어긋나고 공유 청크가 `dist/assets/*` 로 새어 데스크톱 코드까지 무인증
   표면에 열린다.
5. offset 델타 `screen_since`. **추가**: 세션 토큰을 응답에 싣고 요청에도 받는다(§3.3). 탭
   respawn(ADR-0010)은 새 `PtySession`(`bytes_out = 0`)이고 `SessionManager` 는 **프로세스마다 1부터**
   id 를 발급하므로(`session.rs:760`) id 만으로는 앱 재시작을 구분하지 못한다 — 서버 시작 시 뽑은
   랜덤 epoch 를 붙인 `<epoch>:<id>` 가 토큰이다.
6. 세션이 크기를 기억. **확정 형태**: `PtySession.size: AtomicU32`(`cols << 16 | rows`), **`master`
   guard 를 쥔 채** 갱신 — 비중첩 순차 갱신은 두 resize 가 master 와 기록에 서로 다른 순서로
   들어가는 역전 창이 있고, `inner`↔`master` 중첩은 이 파일에 처음 생기는 lock 순서 규약이라 피한다.
7. 입력은 raw 바이트. 변경 없음. **단 폰의 xterm 은 `disableStdin: true`** — 화면 전용이다. replay 에
   보존된 단말 질의(`ESC[6n` 등)에 xterm 이 내는 자동 응답(`ESC[..R`)이 PTY 로 새는 것을 데스크톱은
   `replayDone` 게이트로 막는데(`terminal-view.ts:238, 413`), 폰은 데스크톱과 **동시에** 같은 세션을
   보므로 델타의 질의에도 응답하면 안 된다(데스크톱이 답한다). `disableStdin` 은 xterm 의
   `triggerDataEvent` 를 통째로 끊는다. 입력은 전부 우리 인코더가 만든다(§3.6).
8. 탭 주소는 `TabId`. 변경 없음. **추가**: 입력에도 세션 토큰이 필요하다 — 폰이 이전 셸 화면을
   보고 보낸 텍스트+CR 이 Restart 된 새 셸에서 실행되면 안 된다.
9. attach nudge 대행 금지. 변경 없음.

## 3. 계약 (최종 — 구현은 이 절을 따른다)

### 3.1 설정

`settings.json` 의 `UiSettings`(camelCase, `deny_unknown_fields` 없음 유지)에
`remote: Option<RemoteSettings>`, `RemoteSettings { port: u16 }`.

- 키 존재 = 켜짐. `port` 는 serde 필수 필드이고 `read_ui_settings` 가 `1024..=65535` 밖을
  `fontSize` 와 같은 자리에서 `Err` 로 거부한다 → 기존 loud-fail 경로(`get_ui_settings` →
  `main.ts` `init()` catch → 상태 라인).
- 부팅 1회 읽기, 변경은 재시작. 꺼져 있으면 리스너·스레드·토큰 파일이 **전혀** 생기지 않는다.
- TS 미러(`backend.ts` `UiSettings`)에 `remote: { port: number } | null`.

### 3.2 토큰

- `<app_data_dir>/remote-token`(`state.json` 옆 — `app_data_dir()` 을 직접 부른다). 32B
  `getrandom::fill` → base64url 무패딩 43자, 개행 없음. **원자적 생성**: `remote-token.tmp` 에 쓰고
  rename. **읽을 때 검증**: 트림 후 43자·base64url 알파벳·32바이트로 디코딩되지 않으면 재생성하지
  않고 `failed("remote-token is corrupt; delete it to regenerate")` 로 loud-fail(빈 파일·잘린 파일이
  인증 비밀이 되는 것을 막는다).
- 토큰은 어떤 로그·응답에도 쓰지 않는다. 렌더러로는 **페어링 다이얼로그를 열 때만** 건너간다(§3.5).

### 3.3 HTTP API

요청 처리 순서(모든 커넥션 공통): ① accept 직후 소스 IP 로 rate limit 판정 — 차단 중이면 **아무것도
읽지 않고** 429 후 close. ② 요청 헤드 읽기 — 8 KiB·32 헤더 상한 초과 431, 파싱 실패 400. ③ 라우팅
— 미지 경로·`OPTIONS`·라우트에 맞지 않는 메서드 404. ④ dispatch 직전 rate limit 재판정(accept 뒤
차단된 IP 의 지연 요청도 429). ⑤ `/api/*` 는 `Authorization: Bearer <token>` — 없거나 불일치면 실패를
기록하고, 기록 결과 창 내 실패가 10 을 **넘으면 이 요청부터** 429, 아니면 401(고정 본문
`{"error":"unauthorized"}`). ⑥ 핸들러.

- `GET /api/state` → 200 `application/json`, body = `serde_json::to_vec(dispatcher.snapshot())`
  (데스크톱 `state-changed` 와 동일 — `rootPath`·`cwd` 절대경로가 실리는 것은 계약이다; §3.4 의
  "경로 금지" 는 **에러 본문 한정**).
- **세션 토큰**: `X-Winmux-Session: <epoch>:<id>` — `epoch` 은 `serve()` 가 `getrandom` 으로 뽑은
  u64(10진), `id` 는 `SessionId`. 클라이언트는 이 문자열을 **불투명 값**으로 되돌린다.
- `GET /api/tabs/{tabId}/screen[?since=<u64>&session=<token>]` → 200 `application/octet-stream`.
  응답 헤더 `X-Winmux-End-Offset`, `X-Winmux-Reset: 0|1`, `X-Winmux-Cols`, `X-Winmux-Rows`,
  `X-Winmux-Session`. 규칙: `since` 없음 → reset. `since` 있는데 `session` 이 없거나 현재 토큰과
  다름 → reset. 그 외 `screen_since(Some(since))`. `since` 파싱 실패 400. 탭 없음 404
  `{"error":"unknown tab"}`. **세션 없음 409** `{"error":"tab has no live session"}` — 판정은 탭의
  `TerminalStatus` 가 `Running` 이고 `pty_session: Some` 이며 레지스트리에 있을 때만 세션이 있는
  것으로 본다(`Exited`·`NotStarted` 는 `pty_session` 을 유지하므로 상태를 봐야 한다 —
  `command.rs:492, 523`).
- `POST /api/tabs/{tabId}/input?session=<token>` → body 를 그대로 `PtySession::write`. `session`
  누락·불일치 409 `{"error":"session changed"}`(write 하지 않는다). `Content-Length` 없음 411, 65 536
  초과 413 — 둘 다 **본문을 한 바이트도 읽기 전**에 판정. **프레이밍 규칙**: `Transfer-Encoding`
  이 있으면 411(chunked 를 지원하지 않는다 — chunk 프레이밍이 PTY 에 써지는 사고 방지),
  `Content-Length` 가 둘 이상이거나 값이 다르면 400, `Expect` 헤더가 있으면 417, 선언 길이 전에
  EOF/타임아웃이면 **write 하지 않고** 400. 본문은 헤드와 같은 read 에 딸려온 바이트부터 이어서
  정확히 선언 길이만큼 모은 뒤 한 번에 쓴다. 성공 200 빈 본문. write 실패 500
  `{"error":"write failed"}`(사유는 로그로만). 404/409 는 screen 과 동일.
- 정적: `GET /` → 자산 키 `remote/index.html`; `GET /remote/<seg>/…` → 키 `remote/<seg>/…`.
  세그먼트 규칙 `^[A-Za-z0-9][A-Za-z0-9._-]*$`(빈 세그먼트·선행 `.`·`%`·`\` 전부 404 — 퍼센트
  디코딩을 하지 않는다). 인증 없음. 자산 콜백이 `None` 이면 404.
- 어떤 응답에도 CORS 헤더·`Server` 헤더 없음. 모든 응답에 `Connection: close` 와 `Content-Length`.

### 3.4 보안 처방 (재량 없음)

- 바인드 `0.0.0.0:<port>` IPv4. 커넥션당 스레드, 전역 동시 커넥션 32(초과는 즉시 close), 스트림
  read/write 타임아웃 10초, 핸들러 전체 `catch_unwind`(패닉 → 로그 한 줄 + 소켓 close).
- 토큰 비교 constant-time(길이 일치 확인 후 XOR 누적). 쿼리스트링·쿠키의 토큰은 **무시**(401).
- rate limit: IP 당 인증 실패 10회/60초 초과 시 60초 동안 **그 IP 의 모든 요청**(정적 포함) 429
  (`Retry-After: 60`). `Mutex<RateLimiter>`, 항목 상한 256(초과 시 `last_seen` 최소 제거), poisoned
  여도 `into_inner()` 로 복구.
- 본문을 읽지 않고 거절할 때(400/401/411/413/417/429/431)의 마무리: 응답 write + flush →
  `shutdown(Write)` → 남은 입력을 **총 2초·1 MiB 한도**로 비운다(read 타임아웃 200 ms 단위) → close.
  목적은 미독 데이터가 남은 채 close 해서 나가는 RST 가 클라이언트의 응답 수신을 끊는 것을 줄이는
  것이며 **보장이 아니다** — 테스트는 "헤드만 보낸 클라이언트" 와 "128 KiB 본문을 이미 보낸
  클라이언트" 둘 다 응답을 읽는지 본다.
- **Dispatcher lock 규율**: lock 안에서 하는 일은 `serde_json::to_vec(snapshot)` 또는 탭 순회로
  (`pty_session`, `status`) 를 꺼내는 것뿐. `unwrap()`·인덱싱·패닉 가능 연산 금지. `lock()` 은
  `match` 로 받아 poisoned 면 500 — 이 lock 은 글루 전역이 `.lock().unwrap()` 으로 쓰므로 원격
  스레드가 poison 하면 데스크톱이 죽는다. `SessionManager::get`·`screen_since`·`write` 는 lock 밖.
- 로그 싱크 `Arc<dyn Fn(String) + Send + Sync>`: 부팅 바인드 주소, 인증 실패(IP·창 내 횟수),
  500 사유, 커넥션 패닉, 부팅 실패 사유만. `Head` 는 `Authorization`·`Content-Length`·
  `Transfer-Encoding`·`Expect` 의 존재/값 외의 헤더 값을 보관하지 않는다.
- 폰 페이지: 모델 문자열은 `textContent` 로만. `src/remote/` 에 `innerHTML` 이 등장하지 않는다.
- **알고 받아들이는 한계**(ADR 에 기록): 평문 HTTP 라 Wi-Fi 비밀번호 보유자는 토큰·입력을 볼 수
  있다; localStorage 는 `http://<ip>:<port>` origin 에 묶여 나중에 그 IP 를 받은 기기가 origin 을
  사칭할 수 있다; 32 커넥션 상한은 slowloris 를 막지 못하고 폰 접속을 거부당하게 할 수 있을 뿐
  데스크톱엔 닿지 않는다; **원격 write 는 `writer` mutex 를 데스크톱과 공유**하므로 자식이 stdin 을
  읽지 않으면 그 탭의 데스크톱 입력이 함께 기다리고, 그 상태에서 탭을 닫으면 `TauriHost::kill`
  (Dispatcher lock 아래, `host.rs:359`) → `PtySession::kill` 이 `writer` lock 을 기다려(`session.rs:499`)
  **구조 변이 전체가 멈출 수 있다** — 데스크톱 붙여넣기에도 이미 있는 경로이며(백로그 "입력이 셸에
  닿지 않는다" 와 같은 뿌리), 원격은 트리거를 하나 더한다. 실기 §5.2 에 검증 항목을 두고, `kill()`
  이 `master` 를 먼저 놓아 write 를 깨우는 구조 변경은 §8 후속.

### 3.5 데스크톱 커맨드 · 페어링

- `remote_status() -> RemoteStatus { state: "off" | "on" | "failed", port: Option<u16>,
  reason: Option<String> }` — 프론트 `init()` 이 **설정과 무관하게 항상** 한 번 부른다(설정 파일은
  webview 초기화마다 다시 읽히지만 서버는 부팅 때 고정이므로 설정으로 게이트하면 어긋난다).
  `failed` 면 상태 라인에 `reason`, `on` 이면 사이드바 페어링 버튼 노출. 토큰을 싣지 않는다.
- `remote_pairing() -> Result<Option<Pairing { url: String }>, String>` — 다이얼로그를 열 때만.
  `url = http://<lan-ip>:<port>/#t=<token>`; LAN IP 는 `UdpSocket::bind("0.0.0.0:0")` →
  `connect("192.0.2.1:9")` → `local_addr()`. 꺼짐 `Ok(None)`, 실패 `Err(사유)`.
- 진입점: 사이드바 `.sidebar-footer` 의 버튼(단축키 없음). 다이얼로그는 네이티브
  `<dialog>.showModal()`, `<canvas>` QR(`uqr` dynamic import) + `<code>` URL(`textContent`).

### 3.6 폰 페이지

- 위치: `apps/winmux/remote/index.html`(두 번째 Vite 설정의 root), 소스 `apps/winmux/src/remote/*`.
  빌드 `"build": "tsc && vite build && vite build --config vite.remote.config.ts"`(**`tsc` 유지**,
  메인 빌드가 `dist/` 를 비우므로 순서 고정), 산출물 `dist/remote/`.
- 부트: `#t=<token>` 을 localStorage 에 저장하고 `history.replaceState` 로 fragment 를 지운다.
- 목록 화면: `/api/state` 2초 폴링(가시성 게이트, 중복 발사 금지, 복귀 시 즉시 1회).
- **탭 화면 상태 전이**: `Terminal({ cols, rows, disableStdin: true })`.
  - `full` 상태: `since` 없이 요청 → 응답은 항상 reset=1 → 인스턴스 생성·`term.write(bytes, cb)` →
    cb 에서 `ready`(그 전에는 입력 컨트롤 비활성 — `term.modes` 는 write 완료 후에야 반영된다).
  - `ready` 상태: `since=<endOffset>&session=<token>` 델타 폴링 → reset=0 이면 write; **reset=1 이거나
    cols/rows/session 이 바뀌면** 받은 바이트를 버리고 인스턴스를 dispose 한 뒤 `full` 로 간다(다음
    폴이 `since` 없이 나간다). 이 전이는 `full → ready → (변화) → full → ready` 로 끝난다.
  - **뷰 세대**: 요청마다 (탭, 세대) 를 캡처하고, 탭 전환·재생성 뒤 도착한 이전 세대의 응답과 write
    콜백은 화면·offset·상태를 바꾸지 않는다.
- **입력 인코더**(xterm 이 아니라 우리가 만든다): Send = 텍스트를 `term.modes.bracketedPasteMode`
  면 `ESC[200~ … ESC[201~` 로 감싸 POST, 그 응답 뒤 **≥150 ms 후 별도 POST 로 `\r`** — 한 write 에
  텍스트와 CR 을 같이 넣으면 붙여넣기 감지(Claude Code 의 chunk 길이 규칙, Codex 의 paste burst)가
  CR 을 줄바꿈으로 삼킨다(`winmux send` 에서 실측된 고장, v0.3.16 에서 같은 방식으로 수정). 키 버튼:
  Esc `\x1b`, Tab `\t`, Ctrl+C `\x03`, Backspace `\x7f`, Enter `\r`, 화살표는
  `term.modes.applicationCursorKeysMode` 에 따라 `ESC[A` / `ESCOA`. 모든 입력 POST 는 **클라이언트
  FIFO** 로 직렬화한다(앞 요청의 응답을 받은 뒤 다음을 보낸다 — 서로 다른 커넥션 스레드는 순서를
  보장하지 않는다). 한 액션이 실패(413·401·네트워크)하면 그 액션의 후속(CR)은 보내지 않고 큐를 비운
  뒤 오류를 표시한다. `ready` 전에는 입력 컨트롤이 비활성이다.
- 실패 처리: **401 → 폴링 즉시 중단** + 재페어링 안내. 429 → 60초 중단 + 안내. 네트워크 오류 → 다음
  틱에 재시도.

## 4. 청크별 계획

각 청크는 트리 green + 단독 검토 가능해야 한다. 경로는 워크트리 `/home/kwon1/code/winmux-remote-surface` 기준.

### A — 코어 (`crates/winmux-core`) · 위험: 핫패스 low

- `src/replay.rs`: `pub fn bytes_from(&self, pos: usize) -> Vec<u8>` — 앞 `pos` 바이트를 건너뛴
  나머지, **head 트림 없음**. `pos >= total` 이면 빈 Vec.
  테스트: `bytes_from_zero_returns_everything_untrimmed`, `bytes_from_skips_across_chunk_boundaries`,
  `bytes_from_at_len_returns_empty`, `bytes_from_beyond_len_returns_empty`,
  `bytes_from_ignores_the_evicted_head_trim`.
- `src/session.rs`: `PtySession.size: AtomicU32`(spawn 에서 시드, `resize` 가 master guard 아래에서
  `store`). `pub struct Screen { end_offset, reset, cols, rows, bytes }`,
  `pub fn screen_since(&self, since: Option<u64>) -> Screen` — `inner` lock 한 번 안에서 `bytes_out`·
  `replay.len()`·`dec_modes` 를 읽고: `Some(s) == bytes_out` → 빈 델타; `retained_start <= s <
  bytes_out` → `bytes_from(s - retained_start)`; 그 외 → `reset` + `dec_mode_preamble ++ snapshot()`.
  flow·sink·dec_modes 불변, notify 없음. `reattach()`·`replay()` 불변.
  테스트(`tests/session_integration.rs`, `#![cfg(unix)]`, 기대 프리앰블은 바이트 하드코딩):
  `screen_since_none_returns_a_reset_snapshot_with_the_dec_mode_preamble`,
  `screen_since_at_the_current_offset_returns_an_empty_delta`,
  `screen_since_mid_stream_returns_only_the_new_bytes`,
  `screen_since_older_than_the_retained_window_falls_back_to_reset`,
  `screen_since_ahead_of_bytes_out_falls_back_to_reset`,
  `screen_since_carries_the_spawn_size_and_follows_resize`,
  `screen_since_leaves_flow_and_reattach_untouched`.
- green: `cargo test -p winmux-core` + 양 타깃 clippy/check.

### B1 — `crates/winmux-remote` 순수 모듈 · 위험: 보안 high

- 신규 크레이트(`Cargo.toml`: `winmux-core`(path), `httparse = "1"`, `getrandom = "0.3"`,
  `base64 = "0.22"`, `serde`/`serde_json`; dev `tempfile = "3"`), 루트 `Cargo.toml` `members` 추가.
- `src/http.rs`: `MAX_HEAD_BYTES = 8192`, `MAX_HEADERS = 32`, `MAX_BODY_BYTES = 65_536`;
  `Method { Get, Post, Other }`; `Head { method, path, query, authorization, content_length:
  Option<usize>, duplicate_content_length: bool, has_transfer_encoding: bool, has_expect: bool }`;
  `read_head<R: Read>(r, buf: &mut Vec<u8>) -> Result<(Head, usize), HeadError { TooLarge,
  Malformed, Eof }>` — 반환 `usize` 는 헤드 길이이고 **`buf[len..]` 는 이미 읽힌 본문 선두**로
  호출자가 이어 쓴다. 테스트: `parses_a_minimal_get_request_line`, `assembles_a_head_split_across_reads`,
  `keeps_body_bytes_that_arrived_with_the_head`, `rejects_a_head_larger_than_the_cap`,
  `rejects_more_headers_than_the_cap`, `parses_content_length_and_rejects_a_non_numeric_value`,
  `flags_duplicate_content_length_transfer_encoding_and_expect`,
  `keeps_no_header_value_other_than_authorization_and_content_length`.
- `src/routes.rs`: `Route { State, Screen { tab, since, session }, Input { tab, session },
  Static { key }, NotFound, BadRequest }`, `route(method, path, query)`. 테스트:
  `routes_state_screen_and_input`, `unknown_path_is_not_found`, `options_is_not_found`,
  `a_post_to_a_get_route_is_not_found`, `root_maps_to_the_remote_index`,
  `a_hashed_asset_name_with_dots_is_accepted`, `a_dotfile_segment_is_rejected`,
  `a_parent_segment_is_rejected`, `a_percent_escape_in_a_static_path_is_rejected`,
  `since_parses_and_garbage_is_a_bad_request`, `session_is_carried_as_an_opaque_string`.
- `src/token.rs`: `load_or_create_token(&Path) -> Result<String, TokenError { Io, Corrupt }>`
  (원자적 생성), `generate_token()`, `token_matches()`. 테스트: `a_generated_token_is_43_url_safe_characters`,
  `two_generated_tokens_differ`, `load_or_create_creates_then_reuses_the_same_value`,
  `a_trailing_newline_in_the_file_is_tolerated`, `an_empty_or_truncated_file_is_corrupt_not_regenerated`,
  `a_file_with_a_non_base64url_byte_is_corrupt`, `constant_time_compare_accepts_only_an_exact_match`,
  `constant_time_compare_rejects_a_length_mismatch`.
- `src/ratelimit.rs`: `RateLimiter::{check(ip, now) -> bool, record_failure(ip, now) -> bool
  /* now blocked */}`, `now` 주입. 테스트: `allows_ten_failures_inside_the_window`,
  `the_eleventh_failure_blocks_and_reports_it`, `the_block_expires_after_the_window`,
  `failures_age_out_of_the_window`, `evicts_the_least_recently_seen_entry_past_the_cap`,
  `a_blocked_ip_stays_blocked_until_the_window_ends`.
- green: `cargo test -p winmux-remote` + 양 타깃 clippy/check.

### B2 — `crates/winmux-remote` 서버 · 위험: 보안 high · 핫패스 med

- `src/server.rs`: `StaticAsset`, `AssetFn`, `LogFn`, `RemoteConfig { bind, token }`,
  `RemoteDeps { dispatcher, sessions, assets, log }`, `serve(cfg, deps) -> io::Result<RemoteServer>`
  (epoch 생성 포함), `RemoteServer::local_addr()`. 바인드는 `serve` 안에서 동기로. 스레드·타임아웃·
  drain·catch_unwind 는 §3.4.
- `src/handlers.rs`: state / screen / input / static. 탭 → (`pty_session`, `status`) 는 Dispatcher
  lock 안에서 꺼내고 나온다; `Running` + `Some(id)` + 레지스트리 존재 = 세션 있음.
- 테스트 파일 두 개. `tests/server.rs`(플랫폼 무관, 자체 `FakeHost`): `state_requires_a_bearer_token`,
  `state_body_equals_the_dispatcher_snapshot`, `a_wrong_token_is_401_with_a_fixed_body`,
  `a_token_in_the_query_string_is_ignored_and_401`, `the_eleventh_wrong_token_from_one_ip_is_429`,
  `a_blocked_ip_gets_429_on_static_assets_too`, `a_blocked_ip_is_refused_before_its_head_is_read`,
  `an_unknown_path_is_404`, `options_is_404`, `no_response_carries_a_cors_header`,
  `every_response_closes_the_connection`, `static_assets_need_no_token`, `a_static_miss_is_404`,
  `screen_for_an_unknown_tab_is_404_json`, `screen_for_a_tab_without_a_session_is_409_json`,
  `screen_for_an_exited_tab_is_409_json`, `input_without_content_length_is_411`,
  `input_with_transfer_encoding_is_411`, `input_with_conflicting_content_lengths_is_400`,
  `input_with_expect_is_417`, `input_over_the_body_cap_is_413_before_the_body_is_sent`,
  `a_client_that_already_sent_a_large_body_still_reads_the_413`, `an_oversized_request_head_is_431`,
  `a_garbage_since_is_400`, `input_without_a_session_token_is_409`, `no_response_body_contains_the_token`.
  `tests/server_pty.rs`(`#![cfg(unix)]`, `PtyHost` 가 실제 `sh`): `screen_returns_a_reset_snapshot_then_an_empty_delta_then_new_bytes`,
  `screen_delta_with_a_stale_session_token_is_a_reset`, `input_reaches_the_pty_and_appears_in_the_next_screen`,
  `input_arriving_split_across_reads_is_written_once_and_whole`, `input_with_a_stale_session_token_is_409_and_not_written`,
  `input_success_is_200_with_an_empty_body`, `input_to_a_killed_session_is_500`,
  `input_truncated_before_content_length_is_400_and_not_written`, `a_second_client_gets_its_own_reset_snapshot`.
- CI·Gates: `.github/workflows/ci.yml` gates job 에 `cargo test -p winmux-remote`, `CLAUDE.md` Gates
  블록 갱신(E 는 확인만).
- green: `cargo test -p winmux-remote` + 양 타깃 clippy/check.

### C — 글루 Rust 만 (`apps/winmux/src-tauri`) · 위험: 보안 med · 부팅 med

- `commands.rs`: `RemoteSettings`, `UiSettings.remote`, 범위 검증. `remote_status`, `remote_pairing`
  커맨드(+ `main.rs` `invoke_handler` 등록).
- 신규 `remote.rs`: `init(app, dispatcher, sessions) -> RemoteState` — setup 끝에서 호출, 항상
  `app.manage`. 순서: 설정(`Err`/`None` → off) → 토큰(`Corrupt` → failed) → 자산 키 집합(선행 `/`
  제거) → 부팅 검사(키 집합 非空인데 `remote/index.html` 없음 → failed) → `serve` → on/failed.
  자산 콜백·로그 싱크는 §3.4.
- **프론트 파일은 건드리지 않는다**(TS 미러·`main.ts`·사이드바는 D). 중간 상태: 서버가 뜨고
  `curl` 로 `/api/state` 가 나온다; `/` 는 dev 에서 404.
- green: 양 타깃 clippy/check + `apps/winmux` build/vitest(변화 없음).

### D — 폰 페이지 + 데스크톱 프론트 배선 · 위험: UI/XSS high

- **첫 행위는 스모크 빌드**: `remote/index.html` + `vite.remote.config.ts` + 빈 `src/remote/main.ts`
  로 `npm run build` 가 `dist/remote/index.html` 을 내는지 확인.
- `src/remote/protocol.ts`(+test): `parseScreenMeta`, `nextRequest(state, got)`, `needsRecreate`,
  `encodeInput(action, modes)`(bracketed paste 감싸기·DECCKM 화살표). 테스트: `nextSince follows the
  end offset`, `an empty delta keeps the offset`, `a reset reply to a full request is applied`,
  `a reset reply to a delta request goes back to full`, `a size change goes back to full`,
  `a session change goes back to full`, `malformed headers are rejected`, `paste is bracketed only
  when the mode is on`, `arrows follow application cursor mode`.
- `src/remote/poller.ts`(+test): `PollSchedule` + 뷰 세대. 테스트: `polls at the interval while
  visible`, `stops while hidden`, `polls once immediately on return to visible`, `does not overlap an
  in-flight request`, `a stale generation reply is ignored`, `a 401 stops the schedule`, `a 429 pauses
  for sixty seconds`.
- `src/remote/input-queue.ts`(+test): FIFO, 실패 시 후속 취소, CR 지연. 테스트: `sends actions in
  order one at a time`, `a failed paste cancels its enter`, `enter follows paste after the delay`.
- `src/remote/main.ts`, `api.ts`, `list-view.ts`, `tab-view.ts`, `remote.css`: §3.6.
- 데스크톱: `backend.ts` 미러·래퍼(`remoteStatus`, `remotePairing`), `src/pairing-dialog.ts`
  (+ `pairingMessage` test), `sidebar.ts` 생성자 `onPairing` + footer 버튼 + `setRemoteEnabled`,
  `main.ts` `init()` 에서 `remoteStatus()` 무조건 호출, `styles.css`. `uqr` 를 dynamic import 로만.
- green: `cd apps/winmux && npm run build && npx vitest run`(빌드 출력에 `dist/remote/index.html`,
  데스크톱 엔트리 청크에 QR 바이트 없음).

### E — 문서 · 버전 · 위험: low

- `docs/adr/0016-remote-surface-over-lan.md`(영문): 결정과 기각안, 결과·한계(§3.4 의 한계 전부, 폰
  첫 프레임 불완전, reset 남용 memcpy, 스크롤백 반출은 사용자 자신의 기기용 named opt-in 이라는
  ADR-0005 와의 관계).
- `CLAUDE.md`: Backlog 항목(열린 것: `kill()` writer 대기 구조, 인증된 클라이언트 rate limit),
  Layout 에 `crates/winmux-remote`, Gates 블록 확인. `README.md` Features 한 줄.
  `docs/WINDOWS-BUILD.md` `### v0.3.17 — verification`(§5.2).
- 버전 0.3.17 세 곳. **실행이 끝나면 이 plan 파일을 삭제한다**(레포 Docs 규칙: ADR 로 증류 후 삭제).
- green: 전체 게이트.

## 5. 검증

### 5.1 자동 게이트 (메인이 직접 실행; 커밋 전 전부 green)

```bash
export PATH="$HOME/.local/node/bin:$HOME/.cargo/bin:$PATH"
cargo test -p winmux-core
cargo test -p winmux-remote
cargo clippy --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings
cargo clippy --workspace --all-targets --target aarch64-pc-windows-msvc -- -D warnings
cargo check --workspace --target x86_64-pc-windows-msvc
(cd apps/spike && npm run build && npx vitest run)
(cd apps/winmux && npm run build && npx vitest run)
```

### 5.2 사용자 실기 (`docs/WINDOWS-BUILD.md` §10 v0.3.17)

1. 설정 없음: 포트 미리스닝, `remote-token` 없음, RAM 변화 없음.
2. 설정 후 부팅: 방화벽 프롬프트 개인 네트워크만 허용; 사이드바 버튼 → QR → 폰 → 목록;
   `remote-token` 생성.
3. 정적 게이트: `/` 200, `/remote/index.html` 200, `/remote/nope.js` 404, `/index.html` 404,
   `/assets/anything.js` 404.
4. 탭 화면 일치; 오래 돌린 Claude Code 탭에 두 줄 붙여넣기 → 둘 다 입력칸에 남음; Send+Enter 제출;
   Ctrl+C; **데스크톱 셸 입력줄에 stray `R`·`;1R` 이 남지 않는다**(자동 응답 억제).
5. 인증: 토큰 없이 401; 잘못된 토큰 11회 → 429(정적 포함); `?token=` 쿼리 401.
6. 탭 Restart 중 폰이 보고 있으면 새 셸로 리셋되고, 리셋 전 화면에서 보낸 입력은 409 로 거부된다.
7. 데스크톱 영향 없음(대량 출력 flow·스크롤·워크스페이스 전환·붙여넣기).
8. **blocked write**: 폰에서 stdin 을 읽지 않는 프로그램(예: `sleep 600`)이 도는 탭에 64 KiB 를
   보내고, 데스크톱에서 그 탭을 닫아 본다 — 앱이 멈추면 §3.4 한계가 실기에서 재현된 것이고 ADR 에
   측정값을 남긴다.
9. RAM 증가량 기록.

## 6. 리뷰 findings 처리

### 6.1 plan-critic (Opus) — 채택
F1 자체 fake host; F2 키 정규화; F3 부팅 검사 + 실기 3; F4 rate limit 전 요청; F5 재생성 → full;
F6 세션 토큰; F7 테스트 파일 분리; F8 Windows job 서술; F9 프리앰블 하드코딩; F10 writer 한계 문서화
(분할 write 는 블록을 없애지 못해 기각); F11 세그먼트 규칙; F12 `Mutex` + poison 복구; F13
`remote_status` 분리; F14 스모크 빌드 선행; F15 `AtomicU32`; F16/F17 테스트 추가; F18/F19 서술
정정; F21 에러 본문 한정; F22 ADR 기록; F23 자체 HTTP 루프 메인 승인. **기각(부분)** F20 IP 별
커넥션 상한(한계 문서화만).

### 6.2 codex (`gpt-6-astra`, ultra) — 채택
- **high** replay 자동 응답이 PTY 로 샘 → `disableStdin` + 자체 인코더(§2.7·§3.6). 데스크톱의
  `replayDone` 게이트(`terminal-view.ts:413`)와 같은 뿌리이며 폰은 델타에도 답하면 안 된다.
- **high** paste 와 CR 의 순서 미보장 → 클라이언트 FIFO + CR 별도 POST + 실패 시 후속 취소(§3.6).
- **high** write 완료 전 입력 허용 → `ready` 전 입력 비활성, 인코더는 write 완료 후 `term.modes` 읽음.
- **high** `SessionId` 가 앱 재시작을 구분 못 함(`session.rs:760`, 메인 확인) → epoch 토큰.
- **high** 토큰 파일 무검증 → 검증 + 손상 시 loud-fail + 원자적 생성(§3.2).
- **high(med)** reset 재요청 비종료 → `full`/`ready` 상태 전이 명시(§3.6) + 테스트.
- **high(med)** writer 한계가 Dispatcher 까지 번짐(`host.rs:359`·`session.rs:499`, 메인 확인) →
  §3.4 한계 서술 확장 + 실기 8 + 후속(§8).
- **med** Exited/NotStarted 409 누락(`command.rs:492, 523`, 메인 확인) → 상태 판정 + 테스트.
- **med** 입력에 세션 보호 없음 → `?session=` 필수(§3.3).
- **med** 본문 프레이밍 규칙 → §3.3 규칙(TE 411, 중복 CL 400, Expect 417, 조기 EOF 400·무write).
- **med** `read_head` 본문 선두 소유권 → `buf[len..]` 규약 + 분할/일괄 테스트.
- **med** drain 이 RST 를 보장 못 함 → 시간·바이트 한도 명시, "보장 아님" 명시, 큰 본문 클라이언트 테스트.
- **med** 11번째 실패의 429 타이밍 → 기록 후 판정, accept 와 dispatch 두 곳 검사.
- **med** `remote_status` 를 설정으로 게이트 → 무조건 호출.
- **med** C 가 D 의 `setRemoteEnabled` 에 의존 → 프론트 배선 전부 D 로.
- **med** 빌드 문자열에서 `tsc` 누락 → 유지.
- **med** 늦은 응답 폐기 규칙 → 뷰 세대.
- **med** 화살표 DECCKM → `term.modes.applicationCursorKeysMode`.
- **med** 테스트 목록이 초안을 참조 → 전부 인라인.
- **low** 게이트 `cd` 연쇄 → 서브셸. **low** E 에 plan 삭제 추가.

기각 없음. codex 가 검증하지 못했다고 적은 tiny_http `EqualReader::drop` 은 메인이 소스로 확인했다.

## 7. 리스크 · 알려진 한계

§3.4 마지막 항목 전부. 추가로: attach nudge 의 rows-1 중간값이 폰에 잠깐 보일 수 있다(다음 폴에서
`full` 로 복구); reset 경로의 1 MiB memcpy 는 `inner` lock 안이지만 리더는 read 를 lock 밖에서 하므로
교착은 없고 chunk 커밋만 지연된다; 폰 첫 프레임은 TUI 재그리기 전까지 불완전할 수 있다;
`settings.json` 파싱 실패 시 원격은 조용히 꺼진 채 부팅하고 사유는 상태 라인이 나른다.

## 8. 후속 (구현하지 않음)

keep-alive; 인증된 클라이언트 rate limit; IP 별 커넥션 상한; 토큰 UI 재발급·만료; TLS/Tailscale/
mDNS/푸시; 폰 resize·워크스페이스/탭 조작; nudge 대행; xterm 중복 번들 최적화; **`PtySession::kill`
이 `writer` 를 기다리지 않게 `master` 를 먼저 놓는 구조**(blocked write 가 Dispatcher 를 멈추는
경로의 근본 수정 — Windows ConPTY 에서 master drop 이 블록된 write 를 깨우는지 실측이 먼저).

## 9. 실행 방식

구현은 서브 에이전트, 판단·게이트·리뷰·커밋은 메인. Ultracode 가 켜져 있으므로 Workflow 로
단계별 오케스트레이션하고, 단계 사이마다 메인이 diff 를 읽고 §5.1 게이트를 직접 돌린 뒤 다음
단계를 띄운다.

| 단계 | 청크 | 모델 | 병렬 |
|---|---|---|---|
| 1 | A, B1 | Opus | A ∥ B1 |
| 2 | B2 | Opus | — |
| 3 | C, D | Opus | C ∥ D (C 는 Rust 만, D 는 프론트만 — 파일 겹침 없음) |
| 4 | E | Sonnet | — |

서브 에이전트는 커밋하지 않는다. 각 단계 green 후 메인이 청크 단위로 커밋하고, 전부 끝나면
`change-critic`(Opus) 최종 점검 → PR → squash 머지 → `v0.3.17` 태그 → 릴리스 빌드.
