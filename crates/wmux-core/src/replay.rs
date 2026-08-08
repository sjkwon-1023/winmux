//! Replay buffer — 최근 PTY 출력을 바이트 상한 내에서 보관한다.
//!
//! 프론트엔드 재접속·재렌더 시 최근 화면 내용을 다시 흘려보내기 위한 버퍼.
//! chunk 단위 VecDeque 와 총 바이트 계정으로 구현하며, 상한 초과 시 오래된
//! chunk 를 통째로 evict 한다. escape 시퀀스가 chunk 경계에서 잘릴 수 있는
//! 점은 Spike 한계로 허용한다. 계약: `docs/plans/spike-plan.md` 4.2장.

use std::collections::VecDeque;

pub struct ReplayBuffer {
    chunks: VecDeque<Vec<u8>>,
    /// 보관 중인 총 바이트 수 (chunks 내 길이 합계와 항상 일치).
    total: usize,
    cap: usize,
}

impl ReplayBuffer {
    /// `cap_bytes` — 보관 총량 상한. Spike 기본값은 호출자(session)에서 1MB.
    pub fn new(cap_bytes: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            total: 0,
            cap: cap_bytes,
        }
    }

    /// chunk 하나를 보관한다. 총량이 cap 을 넘으면 오래된 chunk 부터 통째로
    /// evict 한다. 단일 chunk 가 cap 자체를 넘는 극단 케이스에서는 그 chunk 도
    /// evict 되어 버퍼가 빌 수 있다 — cap 은 메모리 방어선이므로 위반하지 않는다.
    pub fn push(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.total += bytes.len();
        self.chunks.push_back(bytes.to_vec());
        while self.total > self.cap {
            match self.chunks.pop_front() {
                Some(evicted) => self.total -= evicted.len(),
                None => break,
            }
        }
    }

    /// 보관 중인 데이터를 오래된 것부터 순서대로 이어붙여 반환한다.
    pub fn snapshot(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.total);
        for chunk in &self.chunks {
            out.extend_from_slice(chunk);
        }
        out
    }

    /// 보관 중인 총 바이트 수.
    pub fn len(&self) -> usize {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_everything_under_cap() {
        let mut buf = ReplayBuffer::new(100);
        buf.push(b"hello ");
        buf.push(b"world");
        assert_eq!(buf.len(), 11);
        assert_eq!(buf.snapshot(), b"hello world");
    }

    #[test]
    fn evicts_oldest_chunk_first() {
        let mut buf = ReplayBuffer::new(10);
        buf.push(b"aaaa"); // 4
        buf.push(b"bbbb"); // 8
        buf.push(b"cccc"); // 12 → "aaaa" evict → 8
        assert_eq!(buf.snapshot(), b"bbbbcccc");
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn evicts_multiple_chunks_if_needed() {
        let mut buf = ReplayBuffer::new(10);
        buf.push(b"aa"); // 2
        buf.push(b"bb"); // 4
        buf.push(b"cccccccc"); // 12 → "aa" evict → 10 (정확히 cap, 유지)
        assert_eq!(buf.snapshot(), b"bbcccccccc");
        assert_eq!(buf.len(), 10);
        buf.push(b"d"); // 11 → "bb" evict → 9
        assert_eq!(buf.snapshot(), b"ccccccccd");
        assert_eq!(buf.len(), 9);
    }

    #[test]
    fn exact_cap_is_not_evicted() {
        let mut buf = ReplayBuffer::new(8);
        buf.push(b"aaaa");
        buf.push(b"bbbb"); // 정확히 cap — evict 없음
        assert_eq!(buf.len(), 8);
        assert_eq!(buf.snapshot(), b"aaaabbbb");
    }

    #[test]
    fn snapshot_preserves_push_order() {
        let mut buf = ReplayBuffer::new(1024);
        buf.push(b"1");
        buf.push(b"22");
        buf.push(b"333");
        assert_eq!(buf.snapshot(), b"122333");
    }

    #[test]
    fn single_chunk_larger_than_cap_leaves_buffer_empty() {
        // cap 을 혼자 넘는 chunk 는 보관하지 않는다 (cap 은 메모리 방어선).
        let mut buf = ReplayBuffer::new(4);
        buf.push(b"toolong");
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
        assert_eq!(buf.snapshot(), b"");
        // 이후 정상 push 는 계속 동작한다.
        buf.push(b"ok");
        assert_eq!(buf.snapshot(), b"ok");
    }

    #[test]
    fn empty_push_is_noop() {
        let mut buf = ReplayBuffer::new(4);
        buf.push(b"");
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn len_stays_accurate_across_evictions() {
        let mut buf = ReplayBuffer::new(6);
        for _ in 0..100 {
            buf.push(b"abc"); // 3 bytes — 항상 최근 2개(6 bytes)만 남아야 한다
        }
        assert_eq!(buf.len(), 6);
        assert_eq!(buf.snapshot(), b"abcabc");
    }
}
