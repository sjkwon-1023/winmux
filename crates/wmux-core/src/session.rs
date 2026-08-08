//! PTY 세션 — 셸 프로세스를 PTY 로 띄우고 출력 파이프라인을 구동한다.
//!
//! `portable-pty` 를 사용해 Windows(ConPTY)/Unix(표준 PTY) 양쪽에서 동작한다.
//!
//! # 출력 파이프라인 계약
//!
//! 세션마다 리더 스레드 하나가 PTY 출력을 chunk 단위로 읽어 다음 순서로 처리한다.
//!
//! 1. `OscScanner::feed` 로 OSC 이벤트를 감지하고(passthrough — 원본 바이트는
//!    그대로 흘러간다) `sink.on_osc` 로 전달한다 (lock 밖).
//! 2. 상태 lock **안**에서 OSC 계정 → `offset = bytes_out` 캡처 → `replay.push`
//!    → `flow.on_sent(n)` → `bytes_out += n` 까지 끝낸다. offset·replay·flow
//!    계정이 한 lock 에서 일관되게 확정된다.
//! 3. lock 을 놓은 **뒤** `sink.on_output(offset, chunk)` 를 호출한다 — 콜백이
//!    `ack()` 등을 재진입 호출해도 안전하다. `Dropped` 반환 시 lock 재취득 후
//!    `flow.on_acked(n)` 보상 롤백 (순서 근거는 리더 루프 주석 참조).
//!
//! flow control 이 Pause 상태이면 전달만 멈추는 게 아니라 **PTY read 자체를
//! 중단**(condvar 대기)해 OS 파이프에 backpressure 를 넘긴다.
//!
//! # offset 스트림과 reattach
//!
//! `offset` 은 세션 시작부터의 누적 출력 오프셋(u64)이다. 각 chunk 는 자기 시작
//! offset 을 달고 나가며, 연속 전달이라면 다음 chunk 의 offset 은 직전
//! offset + len 과 같다. 프론트 재접속은 [`PtySession::reattach`] 로 한다 —
//! "채널 먼저 장착, reattach 나중" 불변식과 offset 기반 dedup 규칙은 해당
//! rustdoc 이 호출자 계약으로 명시한다.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Condvar, Mutex};

use anyhow::{anyhow, ensure};
use portable_pty::{
    native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtyPair, PtySize,
};

use crate::flow::{FlowAction, FlowControl};
use crate::osc::{OscEvent, OscScanner};
use crate::replay::ReplayBuffer;

/// 리더 스레드의 1회 read 버퍼 크기.
const READ_BUF_BYTES: usize = 16 * 1024;

/// PTY 세션의 휘발성 런타임 식별자. `SessionManager` 가 발급하며 프로세스 수명
/// 안에서 재사용하지 않는다. persistence·MCP 대상인 안정 ID(u64 newtype)와는
/// 별개의 공간이다.
pub type SessionId = u32;

/// `SessionSink::on_output` 의 결과 — sink 가 chunk 를 실제로 전달했는지 여부.
/// 리더 스레드의 flow 계정 처리(유지 vs 보상 롤백)를 가른다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// 프론트로 전달됐다 — flow 계정을 유지한다 (이후 ack 으로 회수).
    Delivered,
    /// 전달되지 않았다 (채널 부재·전송 실패 등) — 리더가 flow 계정을 보상
    /// 롤백한다.
    Dropped,
}

/// 세션 이벤트 수신자. Tauri 글루(채널 emit)나 테스트 하네스가 구현한다.
/// 리더 스레드에서 호출되므로 구현은 블로킹을 최소화해야 한다.
pub trait SessionSink: Send + 'static {
    /// PTY 출력 chunk. OSC 감지는 passthrough 라 원본 바이트가 그대로 온다.
    /// `offset` = 이 chunk 시작 시점의 누적 스트림 오프셋. `Dropped` 반환 시
    /// 리더는 이 chunk 의 flow 계정을 보상 롤백한다 (detach 모드).
    ///
    /// 주의 — detach 가 무조건 자유 진행을 뜻하지는 않는다: (a) 이미 paused 인
    /// 상태에서 프론트가 사라지면(ack 두절) 리더는 read 자체를 안 하므로 Dropped
    /// 경로가 실행되지 않고 휴면이 지속되며, (b) Dropped 전환 시점의 잔여 pending
    /// 이 low_water 를 웃돌면 롤백 후에도 paused 가 남는다. 두 경우 모두
    /// `reattach()`(flow reset) 또는 `kill()` 이 회복 경로다.
    fn on_output(&self, offset: u64, bytes: &[u8]) -> Delivery;
    /// 감지된 OSC 이벤트. 같은 chunk 의 `on_output` 보다 먼저 호출된다.
    fn on_osc(&self, event: &OscEvent);
    /// 프로세스 종료. 자연 종료·kill 어느 쪽이든 세션당 정확히 1회 호출된다.
    /// `code` 는 exit code (signal 종료 등으로 알 수 없으면 None).
    fn on_exit(&self, code: Option<u32>);
}

/// 스폰할 프로세스 사양.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

/// 세션 튜닝 옵션. `Default` 는 replay 1MB, high 2MB, low 512KB.
#[derive(Debug, Clone, Copy)]
pub struct SessionOptions {
    pub replay_cap: usize,
    pub high_water: usize,
    pub low_water: usize,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            replay_cap: 1024 * 1024,
            high_water: 2 * 1024 * 1024,
            low_water: 512 * 1024,
        }
    }
}

/// 세션 상태 스냅샷. `last_osc` 는 사람이 읽을 요약 문자열 (예: "777:title").
/// id 는 담지 않는다 — 세션의 id 는 `SessionManager` 레지스트리가 소유한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStats {
    pub bytes_out: u64,
    pub pending: usize,
    pub paused: bool,
    pub osc_count: u64,
    pub last_osc: Option<String>,
    pub alive: bool,
}

/// 리더 스레드와 세션 핸들이 공유하는 상태.
struct Shared {
    inner: Mutex<Inner>,
    /// paused → resume/kill 전환을 리더 스레드에 알리는 신호.
    cond: Condvar,
}

struct Inner {
    flow: FlowControl,
    replay: ReplayBuffer,
    bytes_out: u64,
    osc_count: u64,
    last_osc: Option<String>,
    /// 리더 스레드가 프로세스 종료를 관측하면 false 로 내린다.
    alive: bool,
    /// `kill()` 이 눌렸다 — 리더 스레드는 이를 보면 즉시 루프를 빠져나간다.
    killed: bool,
}

/// PTY 세션 핸들. 스레드 간 공유 가능(`&self` API + 내부 Mutex).
pub struct PtySession {
    shared: Arc<Shared>,
    /// PTY 입력 writer. kill 후에는 None (fd 회수).
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    /// PTY master. resize 에 사용하며 kill 시 drop 해 fd 를 회수한다.
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    /// 리더 스레드가 `Child` 본체(wait 용)를 가져가므로 kill 신호는 분리된 killer 로 보낸다.
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

impl PtySession {
    /// 프로세스를 PTY 로 스폰하고 리더 스레드를 시작한다.
    pub fn spawn(
        spec: SpawnSpec,
        sink: Box<dyn SessionSink>,
        opts: SessionOptions,
    ) -> anyhow::Result<Self> {
        ensure!(
            opts.low_water <= opts.high_water,
            "low_water ({}) must be <= high_water ({})",
            opts.low_water,
            opts.high_water
        );

        let PtyPair { master, slave } = native_pty_system().openpty(PtySize {
            rows: spec.rows,
            cols: spec.cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.cwd(cwd);
        }
        let child = slave.spawn_command(cmd)?;
        // slave fd 는 자식 프로세스가 물려받았으므로 우리 쪽 핸들은 바로 닫는다.
        // (이걸 잡고 있으면 자식 종료 후에도 master read 가 EOF 를 받지 못한다.)
        drop(slave);

        let killer = child.clone_killer();
        let reader = master.try_clone_reader()?;
        let writer = master.take_writer()?;

        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                flow: FlowControl::new(opts.high_water, opts.low_water),
                replay: ReplayBuffer::new(opts.replay_cap),
                bytes_out: 0,
                osc_count: 0,
                last_osc: None,
                alive: true,
                killed: false,
            }),
            cond: Condvar::new(),
        });

        let thread_shared = Arc::clone(&shared);
        // JoinHandle 은 보관하지 않는다(detach). 리더 스레드는 EOF/에러/kill 로
        // 반드시 종료되고, 종료하면서 자신이 가진 reader fd 와 child 핸들을 놓는다.
        std::thread::Builder::new()
            .name("wmux-pty-reader".into())
            .spawn(move || reader_loop(reader, child, sink, thread_shared))?;

        Ok(Self {
            shared,
            writer: Mutex::new(Some(writer)),
            master: Mutex::new(Some(master)),
            killer: Mutex::new(killer),
        })
    }

    /// PTY 입력(stdin)으로 bytes 를 쓴다. kill 이후에는 에러.
    pub fn write(&self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut guard = self.writer.lock().unwrap();
        let writer = guard
            .as_mut()
            .ok_or_else(|| anyhow!("session already killed"))?;
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    /// PTY 창 크기를 변경한다 (자식에게 SIGWINCH 전달). kill 이후에는 에러.
    pub fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        let guard = self.master.lock().unwrap();
        let master = guard
            .as_ref()
            .ok_or_else(|| anyhow!("session already killed"))?;
        master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// 프론트엔드가 n bytes 소비를 완료했다. Resume 전환 시 리더를 깨운다.
    pub fn ack(&self, n: usize) {
        let mut inner = self.shared.inner.lock().unwrap();
        if inner.flow.on_acked(n) == FlowAction::Resume {
            self.shared.cond.notify_all();
        }
    }

    /// 프론트 재접속 — 한 lock 안에서 flow 계정 리셋 + replay 스냅샷 + 스냅샷 끝
    /// 오프셋(= 현재 `bytes_out`)을 일관되게 확정해 반환하고, lock 해제 후
    /// paused 로 대기 중이던 리더를 깨운다.
    ///
    /// 반환 `(end_offset, replay_bytes)`: `replay_bytes` 는 replay buffer 의 최근
    /// 출력으로, 스트림 오프셋 구간 `[end_offset - len, end_offset)` 에 해당한다.
    ///
    /// # 호출자 계약
    ///
    /// - **채널 먼저 장착, reattach 나중.** 새 출력 수신 경로(sink 채널)를 먼저
    ///   연결한 뒤 이 함수를 호출해야 한다. 순서를 어기면 reattach 와 채널 장착
    ///   사이의 출력이 스냅샷에도 채널에도 담기지 않는 유실 창이 생긴다.
    /// - **dedup 규칙.** 채널을 먼저 장착했으므로 스냅샷 구간과 겹치는 chunk 가
    ///   채널로 도착할 수 있다. 호출자는 `offset < end_offset` 인 chunk 를
    ///   폐기하되, **폐기분 포함 받은 전량을 ack** 한다 — "받은 만큼 ack" 단일
    ///   규칙이라야 어떤 인터리빙에서도 flow 계정이 맞고, 계정이 리셋된 에폭에
    ///   대한 초과 ack 은 saturating 으로 무해하다 (`FlowControl::reset` 참조).
    pub fn reattach(&self) -> (u64, Vec<u8>) {
        let (end_offset, replay) = {
            let mut inner = self.shared.inner.lock().unwrap();
            inner.flow.reset();
            (inner.bytes_out, inner.replay.snapshot())
        };
        // lock 을 놓은 뒤 notify — paused 로 대기하던 리더가 즉시 재개된다.
        self.shared.cond.notify_all();
        (end_offset, replay)
    }

    /// replay buffer 에 보관 중인 최근 출력 스냅샷.
    pub fn replay(&self) -> Vec<u8> {
        self.shared.inner.lock().unwrap().replay.snapshot()
    }

    /// 현재 상태 스냅샷.
    pub fn stats(&self) -> SessionStats {
        let inner = self.shared.inner.lock().unwrap();
        SessionStats {
            bytes_out: inner.bytes_out,
            pending: inner.flow.pending(),
            paused: inner.flow.is_paused(),
            osc_count: inner.osc_count,
            last_osc: inner.last_osc.clone(),
            alive: inner.alive,
        }
    }

    /// 세션을 종료한다: 자식 프로세스 kill + 리더 스레드 정리 + PTY fd 회수.
    /// 멱등 — 두 번째 호출부터는 아무것도 하지 않는다.
    pub fn kill(&self) {
        let was_alive;
        {
            let mut inner = self.shared.inner.lock().unwrap();
            if inner.killed {
                return;
            }
            inner.killed = true;
            was_alive = inner.alive;
        }
        // paused 로 condvar 대기 중인 리더 스레드를 깨워 종료 경로로 보낸다.
        self.shared.cond.notify_all();

        if was_alive {
            // 자식이 방금 자연 종료해 신호 전송이 실패(ESRCH 등)할 수 있다.
            // "프로세스가 죽어 있어야 한다"는 의도는 이미 충족된 상태이므로 이
            // 에러는 무시한다 (멱등 kill — 에러 은폐가 아님).
            let _ = self.killer.lock().unwrap().kill();
        }

        // writer/master 를 drop 해 우리가 쥔 PTY fd 를 회수한다. 리더 스레드의
        // 복제 reader 는 스레드 종료 시 함께 drop 된다.
        *self.writer.lock().unwrap() = None;
        *self.master.lock().unwrap() = None;
    }
}

impl Drop for PtySession {
    /// 핸들이 사라질 때 자원(자식 프로세스·PTY fd)을 남기지 않는다.
    fn drop(&mut self) {
        self.kill();
    }
}

/// 리더 스레드 본체. 종료 시(EOF·에러·kill) `sink.on_exit` 를 정확히 1회 호출한다.
fn reader_loop(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn Child + Send + Sync>,
    sink: Box<dyn SessionSink>,
    shared: Arc<Shared>,
) {
    let mut scanner = OscScanner::new();
    let mut buf = [0u8; READ_BUF_BYTES];
    loop {
        // Pause 상태면 read 를 하지 않고 여기서 대기한다 — OS 파이프가 차면서
        // 자식 프로세스까지 backpressure 가 전파된다. kill·reattach 시에도 깨어난다.
        {
            let mut inner = shared.inner.lock().unwrap();
            while inner.flow.is_paused() && !inner.killed {
                inner = shared.cond.wait(inner).unwrap();
            }
            if inner.killed {
                break;
            }
        }

        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            // 자식 종료 시 Unix 에서는 EOF 대신 EIO 가 오는 경우가 많다 —
            // 어느 쪽이든 "출력 끝"이므로 종료 처리로 넘어간다.
            Err(_) => break,
        };
        let chunk = &buf[..n];

        // 계약 순서: feed → on_osc(lock 밖) → [lock 안: osc 계정 → offset 캡처 →
        // replay.push → flow.on_sent → bytes_out += n] → on_output(lock 밖).
        let events = scanner.feed(chunk);
        for event in &events {
            sink.on_osc(event);
        }
        let offset = {
            let mut inner = shared.inner.lock().unwrap();
            inner.osc_count += events.len() as u64;
            if let Some(event) = events.last() {
                inner.last_osc = Some(summarize_osc(event));
            }
            // 이 chunk 의 시작 오프셋 — bytes_out 가산 전에 캡처해야 한다.
            let offset = inner.bytes_out;
            inner.replay.push(chunk);
            // Pause 지시는 다음 루프 상단의 is_paused 검사로 집행되므로
            // 반환 액션에 별도 분기가 필요 없다.
            let _ = inner.flow.on_sent(n);
            inner.bytes_out += n as u64;
            offset
        };
        // sink 콜백은 lock 밖에서 — 콜백이 ack() 등을 재진입 호출해도 안전하다.
        if sink.on_output(offset, chunk) == Delivery::Dropped {
            // 전달되지 않은 chunk 는 flow 계정을 보상 롤백해 미전달 바이트로
            // 계상한다 (detach 모드 — 잔여 pending 에 따라 paused 휴면이 남을 수
            // 있으며 회복은 reattach/kill, trait 문서 참조). on_sent 를 on_output
            // 뒤로 미루는 재배열 대신 롤백을 쓰는 이유: on_output 이 먼저면
            // 콜백 경로에서 유발된 ack 이 on_sent 보다 먼저 도착하는 경합에서
            // saturating_sub 가 그 ack 을 소실시켜 pending 이 영구 누수(래칫)
            // 된다. on_sent 선행을 유지한 채 미전달분만 롤백하면 그 경합이
            // 원천적으로 없다. 롤백이 Resume 을 반환해도 리더 자신이 루프
            // 상단에서 is_paused 를 재검사하므로 별도 notify 는 불필요하다.
            let mut inner = shared.inner.lock().unwrap();
            let _ = inner.flow.on_acked(n);
        }
    }

    // 자식을 회수(reap)해 exit code 를 얻는다. kill 경로에서는 신호가 이미
    // 전송됐으므로 곧 반환된다. wait 실패 시 code 는 알 수 없음(None).
    let code = child.wait().ok().map(|status| status.exit_code());
    {
        let mut inner = shared.inner.lock().unwrap();
        inner.alive = false;
    }
    sink.on_exit(code);
}

/// stats 표시용 OSC 요약 문자열 (예: "777:title").
fn summarize_osc(event: &OscEvent) -> String {
    match event {
        OscEvent::Osc0Title(title) => format!("0:{title}"),
        OscEvent::Osc7Cwd(uri) => format!("7:{uri}"),
        OscEvent::Osc9Notify(message) => format!("9:{message}"),
        OscEvent::Osc777Notify { title, .. } => format!("777:{title}"),
    }
}

/// 세션 레지스트리 — `SessionId` 를 발급하고 세션을 보관한다. 내부 동기화는
/// std Mutex (외부 lock 없이 스레드 간 공유 가능).
pub struct SessionManager {
    next_id: Mutex<SessionId>,
    sessions: Mutex<HashMap<SessionId, Arc<PtySession>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            next_id: Mutex::new(1),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// 세션을 생성한다: id 를 먼저 발급하고, 그 id 로 `make_sink` 를 호출해 sink 를
    /// 만든 뒤 스폰·등록한다. sink 가 자기 세션의 id 를 알아야 하는 경우(이벤트에
    /// id 를 실어 보내는 글루)를 순환 없이 지원하기 위한 factory 시그니처다.
    ///
    /// 스폰이 실패하면 발급된 id 는 그대로 버려진다 — id 는 재사용하지 않는
    /// 휘발성 카운터라 구멍이 나도 무해하다.
    pub fn create(
        &self,
        spec: SpawnSpec,
        opts: SessionOptions,
        make_sink: impl FnOnce(SessionId) -> Box<dyn SessionSink>,
    ) -> anyhow::Result<SessionId> {
        let id = {
            let mut next = self.next_id.lock().unwrap();
            let id = *next;
            *next += 1;
            id
        };
        let sink = make_sink(id);
        let session = PtySession::spawn(spec, sink, opts)?;
        self.sessions.lock().unwrap().insert(id, Arc::new(session));
        Ok(id)
    }

    pub fn get(&self, id: SessionId) -> Option<Arc<PtySession>> {
        self.sessions.lock().unwrap().get(&id).cloned()
    }

    /// 세션을 레지스트리에서 제거하고 kill 한다. 존재했으면 true.
    pub fn remove(&self, id: SessionId) -> bool {
        // kill 은 sessions lock 을 놓은 뒤 수행한다 — sink 콜백·wait 지연이
        // 레지스트리 전체를 잡아두지 않게.
        let removed = self.sessions.lock().unwrap().remove(&id);
        match removed {
            Some(session) => {
                session.kill();
                true
            }
            None => false,
        }
    }

    /// 전체 세션의 (id, stats) 목록 — id 오름차순.
    pub fn stats(&self) -> Vec<(SessionId, SessionStats)> {
        // 세션별 stats lock 을 잡는 동안 레지스트리 lock 을 들고 있지 않도록
        // (id, 핸들) 쌍만 먼저 복사한다.
        let sessions: Vec<(SessionId, Arc<PtySession>)> = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .map(|(id, session)| (*id, Arc::clone(session)))
            .collect();
        let mut stats: Vec<(SessionId, SessionStats)> = sessions
            .into_iter()
            .map(|(id, session)| (id, session.stats()))
            .collect();
        stats.sort_by_key(|&(id, _)| id);
        stats
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
