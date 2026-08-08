//! Flow control(backpressure) 상태 머신.
//!
//! 프론트엔드로 보냈지만 아직 소비(ack)되지 않은 바이트 수(pending)를 추적해
//! PTY 읽기를 멈출지(Pause)/재개할지(Resume) 결정한다. 판단만 하는 순수 상태
//! 머신이며 실제 읽기 중단은 세션 리더 스레드가 수행한다.
//! 계약: `docs/plans/spike-plan.md` 4.3장.

/// `on_sent`/`on_acked` 가 호출자에게 지시하는 동작.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowAction {
    None,
    Pause,
    Resume,
}

pub struct FlowControl {
    high_water: usize,
    low_water: usize,
    /// 보냈지만 아직 ack 되지 않은 바이트 수.
    pending: usize,
    paused: bool,
}

impl FlowControl {
    /// Spike 기본값은 호출자(session)에서 high 2MB / low 512KB.
    /// low > high 는 Resume 이 불가능한 설정 오류이므로 즉시 실패시킨다.
    pub fn new(high_water: usize, low_water: usize) -> Self {
        assert!(
            low_water <= high_water,
            "low_water ({low_water}) must be <= high_water ({high_water})"
        );
        Self {
            high_water,
            low_water,
            pending: 0,
            paused: false,
        }
    }

    /// 프론트로 n bytes 를 보냈다. pending ≥ high 가 되는 순간 한 번만 `Pause`.
    /// 이미 paused 면 (리더가 멈추기 전 잔여 전송이 있어도) `None`.
    pub fn on_sent(&mut self, n: usize) -> FlowAction {
        self.pending = self.pending.saturating_add(n);
        if !self.paused && self.pending >= self.high_water {
            self.paused = true;
            FlowAction::Pause
        } else {
            FlowAction::None
        }
    }

    /// 프론트가 n bytes 소비를 완료했다. paused 상태에서 pending ≤ low 가 되는
    /// 순간 한 번만 `Resume`. 초과 ack 이 와도 pending 은 0 밑으로 내려가지
    /// 않는다(saturating).
    pub fn on_acked(&mut self, n: usize) -> FlowAction {
        self.pending = self.pending.saturating_sub(n);
        if self.paused && self.pending <= self.low_water {
            self.paused = false;
            FlowAction::Resume
        } else {
            FlowAction::None
        }
    }

    pub fn pending(&self) -> usize {
        self.pending
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_high_water_no_action() {
        let mut fc = FlowControl::new(100, 20);
        assert_eq!(fc.on_sent(99), FlowAction::None);
        assert!(!fc.is_paused());
        assert_eq!(fc.pending(), 99);
    }

    #[test]
    fn exactly_high_water_pauses() {
        // 경계값: pending == high 에서 Pause (초과가 아니라 도달 기준).
        let mut fc = FlowControl::new(100, 20);
        assert_eq!(fc.on_sent(100), FlowAction::Pause);
        assert!(fc.is_paused());
    }

    #[test]
    fn no_duplicate_pause_while_paused() {
        let mut fc = FlowControl::new(100, 20);
        assert_eq!(fc.on_sent(150), FlowAction::Pause);
        // 리더가 멈추기 전 잔여 전송 — 이미 paused 이므로 None.
        assert_eq!(fc.on_sent(50), FlowAction::None);
        assert!(fc.is_paused());
        assert_eq!(fc.pending(), 200);
    }

    #[test]
    fn exactly_low_water_resumes() {
        // 경계값: pending == low 에서 Resume (미만이 아니라 도달 기준).
        let mut fc = FlowControl::new(100, 20);
        fc.on_sent(100);
        assert_eq!(fc.on_acked(79), FlowAction::None); // pending 21 > low
        assert_eq!(fc.on_acked(1), FlowAction::Resume); // pending 20 == low
        assert!(!fc.is_paused());
    }

    #[test]
    fn no_resume_when_not_paused() {
        let mut fc = FlowControl::new(100, 20);
        fc.on_sent(50);
        // paused 가 아니면 pending ≤ low 여도 Resume 을 내지 않는다.
        assert_eq!(fc.on_acked(40), FlowAction::None);
        assert_eq!(fc.pending(), 10);
    }

    #[test]
    fn no_duplicate_resume_after_resume() {
        let mut fc = FlowControl::new(100, 20);
        fc.on_sent(100);
        assert_eq!(fc.on_acked(90), FlowAction::Resume);
        assert_eq!(fc.on_acked(5), FlowAction::None);
        assert_eq!(fc.pending(), 5);
    }

    #[test]
    fn over_ack_saturates_at_zero() {
        // ack 초과 — pending 이 음수로 내려가지 않고 0 에서 멈춘다.
        let mut fc = FlowControl::new(100, 20);
        fc.on_sent(50);
        assert_eq!(fc.on_acked(200), FlowAction::None); // not paused → None
        assert_eq!(fc.pending(), 0);
    }

    #[test]
    fn over_ack_while_paused_resumes_once() {
        let mut fc = FlowControl::new(100, 20);
        fc.on_sent(100);
        assert_eq!(fc.on_acked(500), FlowAction::Resume);
        assert_eq!(fc.pending(), 0);
        assert_eq!(fc.on_acked(10), FlowAction::None);
    }

    #[test]
    fn pause_resume_cycle_repeats() {
        // Resume 후 다시 high 에 도달하면 다시 Pause — 상태 머신이 순환한다.
        let mut fc = FlowControl::new(100, 20);
        assert_eq!(fc.on_sent(100), FlowAction::Pause);
        assert_eq!(fc.on_acked(80), FlowAction::Resume);
        assert_eq!(fc.on_sent(80), FlowAction::Pause); // pending 100
        assert!(fc.is_paused());
        assert_eq!(fc.on_acked(100), FlowAction::Resume);
        assert_eq!(fc.pending(), 0);
    }

    #[test]
    fn pending_accumulates_across_sends() {
        let mut fc = FlowControl::new(1000, 100);
        fc.on_sent(300);
        fc.on_sent(300);
        assert_eq!(fc.pending(), 600);
        fc.on_acked(100);
        assert_eq!(fc.pending(), 500);
    }

    #[test]
    #[should_panic(expected = "low_water")]
    fn low_above_high_is_rejected() {
        // 설정 오류는 조용히 넘어가지 않고 즉시 실패시킨다.
        let _ = FlowControl::new(100, 200);
    }

    #[test]
    fn equal_high_and_low_allowed() {
        // high == low 도 유효 — high 도달 시 Pause, 같은 값 이하로 ack 시 Resume.
        let mut fc = FlowControl::new(50, 50);
        assert_eq!(fc.on_sent(50), FlowAction::Pause);
        assert_eq!(fc.on_acked(0), FlowAction::Resume); // pending 50 == low
    }
}
