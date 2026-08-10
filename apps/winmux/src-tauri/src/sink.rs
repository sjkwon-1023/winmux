//! `SessionSink` 구현 — 세션 출력·이벤트를 프론트엔드로 나른다.
//!
//! - 터미널 출력: `tauri::ipc::Channel` 에 `[u64 LE offset][bytes]` 프레임을
//!   `InvokeResponseBody::Raw` 로 전송한다 (JSON 직렬화 금지 — 계획 v2 2·12장).
//!   프론트는 offset 으로 replay 스냅샷과의 겹침을 dedup 한다 (계획 2장).
//! - exit: Dispatcher 에 `SessionExited` 를 반영하고 `publish_state` 로
//!   `state-changed` emit + 저장 예약한다 (dispatch 와 함께 상태 변이의 유일한
//!   두 경로 — 계획 15단계 B-2 저장 훅).
//! - OSC: [`OscRouter`] 에 밀어넣기만 한다 — 모델 반영은 라우터 worker 가 flush
//!   창당 한 번 한다 (18단계 계획 glue 계약). 리더 스레드에서 Dispatcher lock 을
//!   잡지 않는 경계가 여기다.
//! - pane 간 전송(`OSC 777;winmux-send`)만 예외로 라우터를 타지 않는다 — 상태가
//!   아니라 **액션**이라 코얼레싱할 것이 없다 ([`deliver_send`]).

use std::sync::{Arc, Mutex};

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Manager};
use winmux_core::command::SessionEvent;
use winmux_core::osc::OscEvent;
use winmux_core::send::decode_send_text;
use winmux_core::session::{Delivery, SessionId, SessionSink};

use crate::router::OscRouter;
use crate::state::{publish_state, AppState};

/// 세션 1개 분의 sink. 레지스트리(`SinkRegistry`)와 리더 스레드([`SinkHandle`])가
/// `Arc` 로 공유한다 — attach_terminal 이 실행 중인 세션의 채널 슬롯을 갈아끼울
/// 수 있어야 하기 때문.
pub struct TerminalSink {
    session: SessionId,
    app: AppHandle,
    /// 프론트 출력 채널 슬롯. attach 시 장착되고, send 실패(webview 소멸 등)
    /// 시 해제된다 — 이후 출력은 Dropped(detach 모드)로 흐른다.
    channel: Mutex<Option<Channel<InvokeResponseBody>>>,
    /// 이 세션에 채널이 장착된 적이 있는가 — attach_terminal 이 "최초 attach"
    /// 판정에 쓴다. 최초 attach 의 replay 에 담긴 단말 질의(ConPTY 의 ESC[6n 등)
    /// 는 아직 아무도 응답하지 않은 **라이브 질의**라 xterm 이 응답해야 하고
    /// (미응답 시 conhost 가 CPR 을 기다리며 셸이 멈춘다 — 체크포인트 1 재시작
    /// 빈 화면 버그), 재-attach 의 replay 질의는 이전 프론트가 이미 응답한 낡은
    /// 질의라 응답을 억제해야 한다 (stray `R` 버그).
    attached_once: std::sync::atomic::AtomicBool,
    /// OSC 배치 라우터 — [`SessionSink::on_osc`] 가 이벤트를 흘려보내는 곳.
    router: Arc<OscRouter>,
}

impl TerminalSink {
    pub fn new(session: SessionId, app: AppHandle, router: Arc<OscRouter>) -> Self {
        Self {
            session,
            app,
            channel: Mutex::new(None),
            attached_once: std::sync::atomic::AtomicBool::new(false),
            router,
        }
    }

    /// attach 이력을 기록하고 **이전 값**(이미 attach 된 적 있었는지)을 돌려준다.
    pub fn mark_attached(&self) -> bool {
        self.attached_once
            .swap(true, std::sync::atomic::Ordering::Relaxed)
    }

    /// 새 출력 채널을 슬롯에 장착한다 (기존 채널은 대체·폐기).
    ///
    /// **호출 순서 불변식**: `attach_terminal` 은 반드시 이 장착을 먼저 하고
    /// `PtySession::reattach()` 를 나중에 호출해야 한다 — 순서를 바꾸면 그 사이
    /// 출력이 스냅샷에도 채널에도 담기지 않는 유실 창이 생긴다 (reattach rustdoc).
    pub fn attach(&self, channel: Channel<InvokeResponseBody>) {
        *self.channel.lock().unwrap() = Some(channel);
    }

    /// 채널 슬롯을 분리한다 — 뷰 dispose(탭 전환 등) 시 호출. 슬롯이 남아 있으면
    /// 이후 출력이 send 성공(Delivered)인데 프론트는 버리고 ack 하지 않아 pending
    /// 이 high_water 까지 쌓여 백그라운드 세션이 paused 에 고착된다. 분리하면
    /// 출력이 Dropped(detach 모드)로 보상 롤백돼 세션이 자유 진행한다.
    pub fn detach(&self) {
        *self.channel.lock().unwrap() = None;
    }
}

/// `SessionManager::create` 에 넘기는 `Box<dyn SessionSink>` 어댑터 —
/// 레지스트리와 같은 `TerminalSink` 를 공유한다.
pub struct SinkHandle(pub Arc<TerminalSink>);

impl SessionSink for SinkHandle {
    fn on_output(&self, offset: u64, bytes: &[u8]) -> Delivery {
        let mut slot = self.0.channel.lock().unwrap();
        let Some(channel) = slot.as_ref() else {
            // 채널 미장착(attach 전·해제 후) — 리더가 flow 를 보상 롤백하는
            // detach 모드. 출력은 replay buffer 에만 쌓인다.
            return Delivery::Dropped;
        };
        let mut frame = Vec::with_capacity(8 + bytes.len());
        frame.extend_from_slice(&offset.to_le_bytes());
        frame.extend_from_slice(bytes);
        match channel.send(InvokeResponseBody::Raw(frame)) {
            Ok(()) => Delivery::Delivered,
            Err(err) => {
                // send 실패(webview 소멸 등) — 죽은 채널을 슬롯에서 해제해 이후
                // chunk 가 send 시도 없이 Dropped 로 빠지게 한다. 실패 자체는
                // 삼키지 않고 stderr 에 남긴다.
                eprintln!(
                    "[winmux] output channel send failed (session={}): {err}",
                    self.0.session
                );
                *slot = None;
                Delivery::Dropped
            }
        }
    }

    fn on_osc(&self, event: &OscEvent) {
        if let OscEvent::Osc777Send { target, text_b64 } = event {
            // 전송은 상태 델타가 아니라 액션이다 — 배치에 넣으면 flush 창만큼
            // 늦어지고 코얼레싱(last-wins)이 두 번째 전송을 삼킨다.
            deliver_send(&self.0.app, self.0.session, target, text_b64);
            return;
        }
        // 리더 스레드 핫패스 — 배치에 합치고 깨우기만 한다. 모델 반영은 라우터
        // worker 가 flush 창당 한 번 하며, 여기서 Dispatcher lock 을 잡지 않는다
        // (router.rs 잠금 규율).
        self.0.router.push(self.0.session, event);
    }

    fn on_exit(&self, code: Option<u32>) {
        // 리더 스레드에서 Dispatcher lock 을 잡는다 — 교착 없음: dispatch 가
        // lock 아래에서 부르는 kill 은 join 없는 신호라(코어 rustdoc) 리더의
        // 종료를 기다리지 않는다. 미지 세션(CloseTab 선행)은 코어 apply_event
        // 가 무해한 no-op 으로 보장한다 (계획 0-5).
        let Some(state) = self.0.app.try_state::<AppState>() else {
            // 앱 teardown 중 관리 상태가 이미 내려간 경우뿐 — 기록만 남긴다.
            eprintln!(
                "[winmux] on_exit: managed state unavailable (session={})",
                self.0.session
            );
            return;
        };
        let mut dispatcher = state.dispatcher.lock().unwrap();
        dispatcher.apply_event(SessionEvent::SessionExited {
            session: self.0.session,
            code,
        });
        publish_state(&self.0.app, &dispatcher);
    }
}

/// pane 간 텍스트 전송 (에이전트 채널 — `scripts/wsl/skills/winmux-send/SKILL.md`).
/// `OSC 777;winmux-send;<target>;<base64>` 를 받은 세션이 호출한다.
///
/// # 왜 즉시 처리하나
///
/// 알림은 **상태**라 flush 창에 모아 last-wins 로 합치는 것이 옳지만, 전송은
/// **액션**이다. 배치에 넣으면 (1) 창만큼 늦어지고 (2) 같은 창의 두 번째 전송이
/// 첫 번째를 덮어써 사라진다. 그래서 코얼레싱을 건너뛴다. 상태 변이가 전혀
/// 없으므로 스냅샷 발행·저장 예약도 하지 않는다 (성공·실패 모두).
///
/// # 스레드·잠금
///
/// 호출자는 PTY **리더 스레드**다. 여기서 Dispatcher lock 을 잡거나 대상 PTY 에
/// write 하면 송신자 pane 의 출력이 그동안 멈춘다 (대상이 paused 면 write 는
/// 무기한 블록될 수도 있다). 그래서 실제 작업은 통째로 blocking 풀로 넘기고
/// (`commands.rs` 의 write 경로와 같은 규율), 그 안에서도 Dispatcher lock 은
/// 대상 해석에만 잡았다가 **write 전에 놓는다** (state.rs 잠금 규율).
///
/// # 실패
///
/// 디코드·대상 해석·write 실패는 전부 stderr 로만 남기고 **송신자 세션에는 아무
/// 것도 쓰지 않는다** — 진단 문자열이 남의 터미널 화면(그리고 replay 버퍼)에
/// 섞이면 그게 더 나쁜 오염이다. 송신 측에서 보면 전송은 무음 fire-and-forget 이다.
fn deliver_send(app: &AppHandle, sender: SessionId, target: &str, text_b64: &str) {
    let app = app.clone();
    let target = target.to_owned();
    let text_b64 = text_b64.to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = match decode_send_text(&text_b64) {
            Ok(bytes) => bytes,
            Err(err) => {
                eprintln!("[winmux] send: rejected from session {sender}: {err}");
                return;
            }
        };
        let Some(state) = app.try_state::<AppState>() else {
            // 앱 teardown 중 관리 상태가 이미 내려간 경우뿐 (on_exit 과 같은 규율).
            eprintln!("[winmux] send: managed state unavailable; dropped");
            return;
        };
        // 대상 해석은 순수 조회다 — lock 은 조회 동안만 잡고 곧바로 놓는다.
        let resolved = state
            .dispatcher
            .lock()
            .unwrap()
            .resolve_send_target(sender, &target);
        let session = match resolved {
            Ok(session) => session,
            Err(err) => {
                eprintln!("[winmux] send: {err} (target={target:?}, from session {sender})");
                return;
            }
        };
        let Some(handle) = state.sessions.get(session) else {
            // 해석과 write 사이에 탭이 닫힌 경우 — 드물지만 정상 순서다.
            eprintln!("[winmux] send: target session {session} is gone; dropped");
            return;
        };
        if let Err(err) = handle.write(&bytes) {
            eprintln!("[winmux] send: write to session {session} failed: {err:#}");
        }
    });
}
