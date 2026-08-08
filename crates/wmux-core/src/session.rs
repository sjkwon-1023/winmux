//! PTY 세션 — 셸 프로세스를 PTY 로 띄우고 출력 파이프라인을 구동한다.
//!
//! `portable-pty` 를 사용해 Windows(ConPTY)/Unix(표준 PTY) 양쪽에서 동작한다.
//! 세션마다 리더 스레드 하나가 PTY 출력을 읽어
//! `OscScanner::feed` → `sink.on_osc` → `ReplayBuffer::push` → `FlowControl::on_sent`
//! → `sink.on_output` 순서로 처리한다. flow control 이 `Pause` 상태이면 전달만
//! 멈추는 게 아니라 **PTY read 자체를 중단**(condvar 대기)해 OS 파이프에
//! backpressure 를 넘긴다. 계약: `docs/plans/spike-plan.md` 4.4장.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
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

/// 세션 이벤트 수신자. Tauri 글루(채널 emit)나 테스트 하네스가 구현한다.
/// 리더 스레드에서 호출되므로 구현은 블로킹을 최소화해야 한다.
pub trait SessionSink: Send + 'static {
    /// PTY 출력 chunk. OSC 감지는 passthrough 라 원본 바이트가 그대로 온다.
    fn on_output(&self, bytes: &[u8]);
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

/// 세션 튜닝 옵션. `Default` 는 Spike 기본값 (replay 1MB, high 2MB, low 512KB).
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStats {
    pub id: u32,
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
    /// SessionManager 가 발급·기록하는 id. 단독 스폰 시엔 0.
    id: AtomicU32,
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
            id: AtomicU32::new(0),
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

    /// replay buffer 에 보관 중인 최근 출력 스냅샷.
    pub fn replay(&self) -> Vec<u8> {
        self.shared.inner.lock().unwrap().replay.snapshot()
    }

    /// 현재 상태 스냅샷.
    pub fn stats(&self) -> SessionStats {
        let inner = self.shared.inner.lock().unwrap();
        SessionStats {
            id: self.id.load(Ordering::Relaxed),
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
        // 자식 프로세스까지 backpressure 가 전파된다. kill 시에도 깨어난다.
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

        // 계약 순서: feed → on_osc → replay.push → flow.on_sent → on_output.
        let events = scanner.feed(chunk);
        for event in &events {
            sink.on_osc(event);
        }
        {
            let mut inner = shared.inner.lock().unwrap();
            inner.osc_count += events.len() as u64;
            if let Some(event) = events.last() {
                inner.last_osc = Some(summarize_osc(event));
            }
            inner.replay.push(chunk);
            // Pause 지시는 다음 루프 상단의 is_paused 검사로 집행되므로
            // 반환 액션에 별도 분기가 필요 없다.
            let _ = inner.flow.on_sent(n);
            inner.bytes_out += n as u64;
        }
        // sink 콜백은 lock 밖에서 — 콜백이 ack() 등을 재진입 호출해도 안전하다.
        sink.on_output(chunk);
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

/// 세션 레지스트리 — u32 id 를 발급하고 세션을 보관한다. 내부 동기화는 std Mutex.
pub struct SessionManager {
    next_id: Mutex<u32>,
    sessions: Mutex<HashMap<u32, Arc<PtySession>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            next_id: Mutex::new(1),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// 세션을 스폰하고 새 id 를 발급해 등록한다.
    pub fn create(
        &self,
        spec: SpawnSpec,
        sink: Box<dyn SessionSink>,
        opts: SessionOptions,
    ) -> anyhow::Result<u32> {
        let session = PtySession::spawn(spec, sink, opts)?;
        let id = {
            let mut next = self.next_id.lock().unwrap();
            let id = *next;
            *next += 1;
            id
        };
        session.id.store(id, Ordering::Relaxed);
        self.sessions.lock().unwrap().insert(id, Arc::new(session));
        Ok(id)
    }

    pub fn get(&self, id: u32) -> Option<Arc<PtySession>> {
        self.sessions.lock().unwrap().get(&id).cloned()
    }

    /// 세션을 레지스트리에서 제거하고 kill 한다. 존재했으면 true.
    pub fn remove(&self, id: u32) -> bool {
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

    /// 전체 세션의 stats 목록 (id 오름차순).
    pub fn stats(&self) -> Vec<SessionStats> {
        // 세션별 stats lock 을 잡는 동안 레지스트리 lock 을 들고 있지 않도록
        // 핸들만 먼저 복사한다.
        let sessions: Vec<Arc<PtySession>> =
            self.sessions.lock().unwrap().values().cloned().collect();
        let mut stats: Vec<SessionStats> = sessions.iter().map(|s| s.stats()).collect();
        stats.sort_by_key(|s| s.id);
        stats
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
