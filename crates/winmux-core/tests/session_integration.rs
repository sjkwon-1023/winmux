//! PtySession unix 통합 테스트 — 실제 `sh` 를 PTY 로 띄워 계약을 검증한다.
//! 계약은 `winmux_core::session` 모듈 rustdoc 이 정의한다. Windows 에서는
//! 컴파일되지 않는다.
#![cfg(unix)]

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use winmux_core::osc::OscEvent;
use winmux_core::session::{
    Delivery, PtySession, SessionId, SessionManager, SessionOptions, SessionSink, SpawnSpec,
};

/// 개별 대기 상한 — hang 방지 가드. WSL 부하를 감안해 넉넉히 잡는다.
const TIMEOUT: Duration = Duration::from_secs(10);

/// sink 콜백을 채널로 옮겨 테스트 본문에서 순서대로 검사한다.
#[derive(Debug)]
enum Event {
    Output { offset: u64, bytes: Vec<u8> },
    Osc(OscEvent),
    Exit(Option<u32>),
}

struct ChannelSink {
    tx: Sender<Event>,
}

impl SessionSink for ChannelSink {
    fn on_output(&self, offset: u64, bytes: &[u8]) -> Delivery {
        // 테스트가 먼저 끝나 수신자가 drop 된 뒤의 send 실패는 무해하다.
        let _ = self.tx.send(Event::Output {
            offset,
            bytes: bytes.to_vec(),
        });
        Delivery::Delivered
    }
    fn on_osc(&self, event: &OscEvent) {
        let _ = self.tx.send(Event::Osc(event.clone()));
    }
    fn on_exit(&self, code: Option<u32>) {
        let _ = self.tx.send(Event::Exit(code));
    }
}

/// 항상 `Dropped` 를 반환하는 sink — 수신자 없는 detach 모드 시뮬레이션.
struct DropSink;

impl SessionSink for DropSink {
    fn on_output(&self, _offset: u64, _bytes: &[u8]) -> Delivery {
        Delivery::Dropped
    }
    fn on_osc(&self, _event: &OscEvent) {}
    fn on_exit(&self, _code: Option<u32>) {}
}

fn sh_spec() -> SpawnSpec {
    SpawnSpec {
        program: "sh".into(),
        args: vec![],
        cwd: None,
        cols: 80,
        rows: 24,
    }
}

fn spawn_sh(opts: SessionOptions) -> (PtySession, Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    let session = PtySession::spawn(sh_spec(), Box::new(ChannelSink { tx }), opts)
        .expect("failed to spawn sh in pty");
    (session, rx)
}

/// deadline 까지 남은 시간. 이미 지났으면 무엇을 기다리다 초과했는지 알리며 실패.
fn remaining(deadline: Instant, waiting_for: &str) -> Duration {
    deadline
        .checked_duration_since(Instant::now())
        .unwrap_or_else(|| panic!("timed out waiting for {waiting_for}"))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

// 1) sh spawn → "echo hello" write → on_output 으로 hello 수신.
#[test]
fn spawn_sh_and_receive_echo_output() {
    let (session, rx) = spawn_sh(SessionOptions::default());
    session.write(b"echo hello\n").expect("write echo command");

    let deadline = Instant::now() + TIMEOUT;
    let mut acc: Vec<u8> = Vec::new();
    while !contains(&acc, b"hello") {
        match rx.recv_timeout(remaining(deadline, "echo output")) {
            Ok(Event::Output { bytes, .. }) => acc.extend_from_slice(&bytes),
            Ok(_) => {}
            Err(err) => panic!(
                "no output containing 'hello' ({err}); received {} bytes: {:?}",
                acc.len(),
                String::from_utf8_lossy(&acc)
            ),
        }
    }
    session.kill();
}

// 2) printf 로 OSC 777 방출 → on_osc 이벤트 수신 (BEL 종결·ST 종결 각 1회).
#[test]
fn osc777_events_received_for_bel_and_st() {
    let (session, rx) = spawn_sh(SessionOptions::default());
    // BEL(\007) 종결. sh 의 printf 가 \033/\007 을 실제 제어 바이트로 변환한다.
    session
        .write(b"printf '\\033]777;notify;T1;B1\\007'\n")
        .expect("write printf (BEL)");
    // ST(ESC \) 종결. 작은따옴표 안 `\\` 는 printf 가 `\` 하나로 변환한다.
    session
        .write(b"printf '\\033]777;notify;T2;B2\\033\\\\'\n")
        .expect("write printf (ST)");

    let deadline = Instant::now() + TIMEOUT;
    let mut oscs: Vec<OscEvent> = Vec::new();
    while oscs.len() < 2 {
        match rx.recv_timeout(remaining(deadline, "2 OSC events")) {
            Ok(Event::Osc(event)) => oscs.push(event),
            Ok(_) => {}
            Err(err) => panic!("expected 2 OSC events, got {oscs:?} ({err})"),
        }
    }
    assert_eq!(
        oscs,
        vec![
            OscEvent::Osc777Notify {
                title: "T1".into(),
                body: "B1".into()
            },
            OscEvent::Osc777Notify {
                title: "T2".into(),
                body: "B2".into()
            },
        ]
    );
    session.kill();
}

// 3) 대량 출력 + 무ack → paused 전환, pending 이 high_water 부근에서 멈춤,
//    전량 ack → 재개되어 추가 출력 수신.
#[test]
fn flood_without_ack_pauses_then_full_ack_resumes() {
    const HIGH: usize = 64 * 1024;
    const LOW: usize = 16 * 1024;
    let opts = SessionOptions {
        replay_cap: 64 * 1024,
        high_water: HIGH,
        low_water: LOW,
    };
    let (session, rx) = spawn_sh(opts);
    session.write(b"seq 200000\n").expect("write seq command");

    // ack 없이 paused 전환을 기다린다.
    let deadline = Instant::now() + TIMEOUT;
    while !session.stats().paused {
        let _ = remaining(deadline, "paused transition");
        thread::sleep(Duration::from_millis(10));
    }

    // pending 은 high_water 도달 직후에서 멈춰야 한다 — 마지막 read chunk
    // 하나만큼의 overshoot 만 허용한다.
    let paused_stats = session.stats();
    assert!(
        paused_stats.pending >= HIGH,
        "pending {} below high_water {HIGH} while paused",
        paused_stats.pending
    );
    assert!(
        paused_stats.pending < HIGH + 64 * 1024,
        "pending {} ran away past high_water {HIGH}",
        paused_stats.pending
    );

    // read 자체가 멈췄는지 확인: pause 직전 chunk 의 in-flight 송신을 정리한 뒤
    // 조용한지, pending 이 변하지 않는지 본다.
    thread::sleep(Duration::from_millis(200));
    while rx.try_recv().is_ok() {}
    thread::sleep(Duration::from_millis(300));
    assert!(
        rx.try_recv().is_err(),
        "output kept flowing while session was paused"
    );
    assert_eq!(
        session.stats().pending,
        paused_stats.pending,
        "pending changed while paused without ack"
    );

    // 전량 ack → Resume → 추가 출력이 도착해야 한다.
    session.ack(paused_stats.pending);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match rx.recv_timeout(remaining(deadline, "output after resume")) {
            Ok(Event::Output { .. }) => break,
            Ok(_) => {}
            Err(err) => panic!("no output arrived after full ack ({err})"),
        }
    }
    session.kill();
}

// 4) exit → on_exit 호출 (exit code 전달, stats.alive false).
#[test]
fn exit_invokes_on_exit_with_code() {
    let (session, rx) = spawn_sh(SessionOptions::default());
    session.write(b"exit 3\n").expect("write exit command");

    let deadline = Instant::now() + TIMEOUT;
    let code = loop {
        match rx.recv_timeout(remaining(deadline, "exit event")) {
            Ok(Event::Exit(code)) => break code,
            Ok(_) => {}
            Err(err) => panic!("no exit event received ({err})"),
        }
    };
    assert_eq!(code, Some(3));
    // on_exit 직전에 alive 가 내려가므로 이벤트 수신 후에는 false 가 보장된다.
    assert!(
        !session.stats().alive,
        "stats.alive must be false after exit"
    );
}

// 5) SessionManager — id 발급, create/get/remove/stats.
#[test]
fn manager_create_get_remove_stats() {
    let manager = SessionManager::new();
    let (tx, _rx) = mpsc::channel();

    let tx1 = tx.clone();
    let id1 = manager
        .create(sh_spec(), SessionOptions::default(), move |_| {
            Box::new(ChannelSink { tx: tx1 })
        })
        .expect("create session 1");
    let tx2 = tx.clone();
    let id2 = manager
        .create(sh_spec(), SessionOptions::default(), move |_| {
            Box::new(ChannelSink { tx: tx2 })
        })
        .expect("create session 2");
    assert_ne!(id1, id2);
    assert!(manager.get(id1).is_some());
    assert!(manager.get(id2).is_some());

    let stats = manager.stats();
    assert_eq!(stats.len(), 2);
    let ids: Vec<SessionId> = stats.iter().map(|&(id, _)| id).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "stats must be sorted by id ascending");
    assert!(ids.contains(&id1) && ids.contains(&id2));
    assert!(stats.iter().all(|(_, s)| s.alive));

    assert!(manager.remove(id1));
    assert!(manager.get(id1).is_none());
    assert!(!manager.remove(id1), "second remove must be a no-op");
    assert_eq!(manager.stats().len(), 1);
    assert!(manager.remove(id2));
    assert!(manager.stats().is_empty());
}

// 신규 (a) manager.create 의 sink factory 가 받는 id == create 가 반환하는 id.
//    동기 경로만 검증하므로 대기(타임아웃 가드 대상)가 없다.
#[test]
fn manager_create_passes_issued_id_to_sink_factory() {
    let manager = SessionManager::new();
    let captured: Arc<Mutex<Option<SessionId>>> = Arc::new(Mutex::new(None));
    let captured_in_factory = Arc::clone(&captured);
    let id = manager
        .create(sh_spec(), SessionOptions::default(), move |issued| {
            *captured_in_factory.lock().unwrap() = Some(issued);
            // 출력은 버린다 — 이 테스트는 id 전달만 본다.
            Box::new(DropSink)
        })
        .expect("create session");
    assert_eq!(
        *captured.lock().unwrap(),
        Some(id),
        "sink factory must receive the same id create returns"
    );
    assert!(manager.remove(id));
}

// 신규 (b) flood → paused → reattach() → paused 해제 + end_offset·offset 연속성.
#[test]
fn reattach_resumes_paused_session_with_continuous_offsets() {
    // HIGH 를 넉넉히 잡아 reattach 직후 재-pause 까지의 여유(재-pause 에는 256KB
    // 재유입 필요)를 확보한다. seq 200000 출력(약 1.4MB)이면 pause 는 확실히 온다.
    const HIGH: usize = 256 * 1024;
    const LOW: usize = 64 * 1024;
    let opts = SessionOptions {
        replay_cap: 64 * 1024,
        high_water: HIGH,
        low_water: LOW,
    };
    let (session, rx) = spawn_sh(opts);
    session.write(b"seq 200000\n").expect("write seq command");

    // ack 없이 paused 전환을 기다린다.
    let deadline = Instant::now() + TIMEOUT;
    while !session.stats().paused {
        let _ = remaining(deadline, "paused transition");
        thread::sleep(Duration::from_millis(10));
    }

    // paused 동안 리더는 read 를 멈추므로 bytes_out 은 고정이다. pause 직전까지
    // 계정된 chunk 의 in-flight 송신을 전부 수신해 reattach 경계를 결정적으로
    // 만든다 (offset 연속성 덕에 마지막 chunk 끝 == bytes_out 이 수렴 조건).
    let frozen = session.stats().bytes_out;
    let mut chunks: Vec<(u64, usize)> = Vec::new();
    // 수신 바이트도 누적한다 — replay 스냅샷이 이 스트림의 tail 과 바이트 단위로
    // 일치하는지(내용 검증)까지 보기 위해서다.
    let mut stream: Vec<u8> = Vec::new();
    let deadline = Instant::now() + TIMEOUT;
    while chunks.last().map(|&(offset, len)| offset + len as u64) != Some(frozen) {
        match rx.recv_timeout(remaining(deadline, "in-flight chunks before reattach")) {
            Ok(Event::Output { offset, bytes }) => {
                chunks.push((offset, bytes.len()));
                stream.extend_from_slice(&bytes);
            }
            Ok(_) => {}
            Err(err) => panic!("in-flight chunks did not drain up to bytes_out {frozen} ({err})"),
        }
    }
    let pre_reattach_count = chunks.len();

    let (end_offset, replay) = session.reattach();
    assert_eq!(
        end_offset, frozen,
        "end_offset must equal bytes_out at reattach"
    );
    assert!(
        !replay.is_empty(),
        "replay snapshot must not be empty after flood"
    );
    // 내용 검증: replay 는 전달된 스트림의 정확한 tail — 구간
    // [end_offset - replay.len(), end_offset) 과 바이트 단위로 일치해야 한다.
    // (offset dedup 계약의 나머지 절반 — 위치만이 아니라 내용의 정합.)
    assert_eq!(
        stream.len() as u64,
        frozen,
        "accumulated stream must cover exactly [0, end_offset)"
    );
    let tail_start = stream.len() - replay.len();
    assert_eq!(
        replay.as_slice(),
        &stream[tail_start..],
        "replay snapshot must be the byte-exact tail of the delivered stream"
    );
    // reattach 가 lock 안에서 paused 를 내렸다. 리더가 이 관측 전에 재-pause
    // 하려면 HIGH(256KB) 를 무ack 로 다시 채워야 하므로 즉시 관측은 안정적이다.
    assert!(!session.stats().paused, "reattach must clear paused");

    // 재개 증명: 새 chunk 가 도착해야 한다. 재-pause 를 막기 위해 받는 즉시
    // 전량 ack 한다 (reattach 호출자 계약과 동일한 "받은 만큼 ack" 규칙).
    let deadline = Instant::now() + TIMEOUT;
    while chunks.len() < pre_reattach_count + 3 {
        match rx.recv_timeout(remaining(deadline, "post-reattach output")) {
            Ok(Event::Output { offset, bytes }) => {
                session.ack(bytes.len());
                chunks.push((offset, bytes.len()));
            }
            Ok(_) => {}
            Err(err) => panic!("no output arrived after reattach ({err})"),
        }
    }

    // 첫 post-reattach chunk 는 스냅샷 이후 구간이어야 한다 (dedup 경계 계약).
    let (first_offset, _) = chunks[pre_reattach_count];
    assert!(
        first_offset >= end_offset,
        "first post-reattach chunk offset {first_offset} precedes snapshot end {end_offset}"
    );
    // 전 구간 offset 연속성: 각 chunk 의 offset == 직전 offset + 직전 len.
    // (reattach 경계를 포함한 전체 스트림이 끊김 없이 이어져야 한다.)
    for pair in chunks.windows(2) {
        let (prev_offset, prev_len) = pair[0];
        let (next_offset, _) = pair[1];
        assert_eq!(
            next_offset,
            prev_offset + prev_len as u64,
            "offset stream must be continuous"
        );
    }
    session.kill();
}

// 신규 (c) 항상 Dropped 를 반환하는 sink — pending 무누적(pause 없음)·bytes_out
//    증가·replay 기록을 검증한다 (보상 롤백 경로).
#[test]
fn dropped_sink_keeps_session_free_running() {
    const HIGH: usize = 64 * 1024;
    let opts = SessionOptions {
        replay_cap: 64 * 1024,
        high_water: HIGH,
        low_water: 16 * 1024,
    };
    let session =
        PtySession::spawn(sh_spec(), Box::new(DropSink), opts).expect("failed to spawn sh in pty");
    session.write(b"seq 200000\n").expect("write seq command");

    // 보상 롤백이 없다면 pending 이 HIGH 에 도달해 pause 로 굳는다 — bytes_out 이
    // HIGH 를 훌쩍 넘겨 계속 증가하는 것이 자유 진행(pause 없음)의 증거다.
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let stats = session.stats();
        assert!(!stats.paused, "session paused despite Dropped rollback");
        // 순간 샘플은 롤백 직전 in-flight chunk 1개 분량(≤ read 버퍼)일 수 있으나
        // 누적은 없어야 한다 — HIGH 미만이면 누수가 아니다.
        assert!(
            stats.pending < HIGH,
            "pending {} accumulated despite Dropped rollback",
            stats.pending
        );
        if stats.bytes_out >= (4 * HIGH) as u64 {
            break;
        }
        let _ = remaining(deadline, "bytes_out growth with Dropped sink");
        thread::sleep(Duration::from_millis(10));
    }

    // 출력이 조용해진 뒤에는 in-flight chunk 가 없으므로 pending 은 정확히 0.
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let before = session.stats().bytes_out;
        thread::sleep(Duration::from_millis(200));
        let after = session.stats();
        if after.bytes_out == before {
            assert_eq!(after.pending, 0, "pending must settle to 0 after quiesce");
            assert!(!after.paused, "session must not be paused after quiesce");
            break;
        }
        let _ = remaining(deadline, "output quiesce with Dropped sink");
    }

    assert!(
        !session.replay().is_empty(),
        "replay must record output even when the sink drops it"
    );
    session.kill();
}

// 신규 (d) kill() → on_exit 정확히 1회 (waiter 단독 호출)·stats.alive=false·
//    멱등 kill 재호출에도 추가 on_exit 없음.
#[test]
fn kill_invokes_on_exit_exactly_once() {
    let (session, rx) = spawn_sh(SessionOptions::default());
    session.kill();

    let deadline = Instant::now() + TIMEOUT;
    loop {
        match rx.recv_timeout(remaining(deadline, "exit event after kill")) {
            Ok(Event::Exit(_)) => break,
            Ok(_) => {}
            Err(err) => panic!("no exit event after kill ({err})"),
        }
    }
    // waiter 가 on_exit 직전에 alive 를 내리므로 수신 후에는 false 가 보장된다.
    assert!(
        !session.stats().alive,
        "stats.alive must be false after kill"
    );

    // 멱등성: 두 번째 kill 은 no-op — 두 번째 Exit 이 오면 안 된다.
    session.kill();
    thread::sleep(Duration::from_millis(300));
    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(event, Event::Exit(_)),
            "on_exit must fire exactly once per session"
        );
    }
}
