//! 세션 레지스트리 상태.
//!
//! Tauri managed state로 올라가는 `AppState`가 id 발급과 id→`PtySession` 매핑을
//! 하나의 뮤텍스 아래에서 관리한다. wmux-core 쪽 내부 id 부여 방식과 무관하게,
//! 프론트엔드 계약(이벤트·커맨드의 `id`)은 이 레지스트리가 발급한 id를 쓴다.
//!
//! 세션은 `Arc`로 보관한다 — 커맨드는 레지스트리 뮤텍스 아래에서는 핸들 복사만
//! 하고, 블로킹 가능성이 있는 세션 호출(PTY write 등)은 반드시 뮤텍스를 놓은 뒤
//! 수행한다. 레지스트리 락을 쥔 채 블로킹하면 ack_output까지 막혀 flow control이
//! 영구 교착된다 (paused → 자식 stdout 블록 → write 블록 → ack 불가 체인).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use wmux_core::session::PtySession;

/// Tauri managed state. 내부는 뮤텍스로 동기화한다.
#[derive(Default)]
pub struct AppState {
    inner: Mutex<Registry>,
}

impl AppState {
    /// 레지스트리 잠금 획득. poison은 리더 스레드/커맨드 쪽 패닉이 원인이므로
    /// 조용히 복구하지 않고 시끄럽게 실패시킨다.
    pub fn registry(&self) -> MutexGuard<'_, Registry> {
        self.inner.lock().expect("session registry mutex poisoned")
    }
}

/// id 발급기 + id→세션 맵. BTreeMap이라 stats 나열이 id 순으로 안정적이다.
#[derive(Default)]
pub struct Registry {
    next_id: u32,
    sessions: BTreeMap<u32, Arc<PtySession>>,
}

impl Registry {
    /// 새 터미널 id를 발급한다 (1부터 시작). sink가 spawn 전에 id를 알아야 하므로
    /// 발급과 등록(insert)이 분리돼 있다 — spawn 실패 시 id는 그냥 버려진다.
    pub fn allocate_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    pub fn insert(&mut self, id: u32, session: PtySession) {
        self.sessions.insert(id, Arc::new(session));
    }

    /// 세션 핸들 복사본을 돌려준다. 호출자는 레지스트리 락을 놓은 뒤 사용한다.
    pub fn get(&self, id: u32) -> Option<Arc<PtySession>> {
        self.sessions.get(&id).cloned()
    }

    pub fn remove(&mut self, id: u32) -> Option<Arc<PtySession>> {
        self.sessions.remove(&id)
    }

    /// (id, 핸들) 스냅샷 — stats 계산은 락 밖에서 하도록 복사해 준다.
    pub fn handles(&self) -> Vec<(u32, Arc<PtySession>)> {
        self.sessions
            .iter()
            .map(|(id, session)| (*id, Arc::clone(session)))
            .collect()
    }
}
