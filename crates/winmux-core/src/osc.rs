//! OSC(Operating System Command) 시퀀스 감지기.
//!
//! PTY 출력 스트림에서 OSC 0/7/9/777 시퀀스를 증분(incremental)으로 감지한다.
//! 감지 전용이다 — 입력 바이트를 변형하거나 소비 표시하지 않으며, 호출자는 입력을
//! 그대로 프론트엔드에 passthrough 한다. 계약: `docs/plans/spike-plan.md` 4.1장.
//! OSC 2(아이콘+창 제목)는 ConPTY 가 제목을 재인코딩할 가능성에 대비해 OSC 0 과 동일하게
//! `Osc0Title` 로 취급한다.

/// 감지된 OSC 이벤트. 문자열은 payload 를 UTF-8 lossy 변환한 결과다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscEvent {
    /// OSC 0 — 창 제목 설정.
    Osc0Title(String),
    /// OSC 7 — 현재 작업 디렉터리 (`file://host/path` URI).
    Osc7Cwd(String),
    /// OSC 9 — 단순 알림 (iTerm2/ConEmu 계열).
    Osc9Notify(String),
    /// OSC 777 — `notify;title;body` 형식 알림 (urxvt 계열).
    Osc777Notify { title: String, body: String },
}

/// payload 상한 (bytes). 초과하는 시퀀스는 통째로 폐기한다 — 악성/폭주 입력 방어.
const MAX_PAYLOAD_BYTES: usize = 4096;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
/// CAN — 실터미널(xterm/ConPTY)은 이 제어문자에서 진행 중인 OSC를 중단한다.
const CAN: u8 = 0x18;
/// SUB — CAN과 동일하게 시퀀스를 중단시킨다.
const SUB: u8 = 0x1a;

/// 스캐너 내부 상태. 상태가 `feed` 호출 사이에 유지되므로 한 시퀀스가
/// 여러 청크에 걸쳐 나뉘어 와도 인식된다.
enum State {
    /// 일반 텍스트 — ESC 를 기다린다.
    Ground,
    /// ESC 직후 — 다음 바이트로 시퀀스 종류를 판별한다.
    Esc,
    /// `ESC ]` 이후 payload 수집 중.
    Collect,
    /// payload 수집 중 ESC 를 봄 — 다음이 `\` 면 ST 종결.
    CollectEsc,
}

/// OSC 증분 상태 머신.
pub struct OscScanner {
    state: State,
    buf: Vec<u8>,
    /// payload 가 상한을 넘어 이번 시퀀스를 폐기하기로 결정한 상태.
    /// 종결자까지는 계속 소비하되 이벤트를 내지 않는다.
    overflow: bool,
}

impl OscScanner {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            buf: Vec::new(),
            overflow: false,
        }
    }

    /// bytes 를 순회하며 완성된 이벤트를 반환한다. 감지 전용 — 입력을 변형하지
    /// 않는다(passthrough). 미완성 시퀀스는 내부 상태로 유지되어 다음 `feed` 에서
    /// 이어서 처리된다.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<OscEvent> {
        let mut events = Vec::new();
        for &b in bytes {
            match self.state {
                State::Ground => {
                    if b == ESC {
                        self.state = State::Esc;
                    }
                }
                State::Esc => {
                    if b == b']' {
                        self.begin_collect();
                    } else if b == ESC {
                        // ESC 연속 — 마지막 ESC 기준으로 다시 판별한다.
                        self.state = State::Esc;
                    } else {
                        // OSC 가 아닌 ESC 시퀀스(CSI 등) — 감지 대상이 아니므로 무시.
                        self.state = State::Ground;
                    }
                }
                State::Collect => {
                    if b == BEL {
                        if let Some(ev) = self.finish() {
                            events.push(ev);
                        }
                    } else if b == ESC {
                        self.state = State::CollectEsc;
                    } else if b == CAN || b == SUB {
                        // 실터미널 동작에 맞춰 시퀀스를 중단한다 — 종결자 없는 깨진
                        // OSC 가 이후 일반 출력을 payload 로 계속 삼키는 것을 막는다.
                        self.discard();
                        self.state = State::Ground;
                    } else if self.buf.len() >= MAX_PAYLOAD_BYTES {
                        // 상한 초과 — 이번 시퀀스는 폐기 확정. 메모리를 잡아두지 않도록
                        // 지금까지 모은 payload 도 버린다.
                        self.overflow = true;
                        self.buf.clear();
                    } else {
                        self.buf.push(b);
                    }
                }
                State::CollectEsc => {
                    if b == b'\\' {
                        // ST(`ESC \`) 종결.
                        if let Some(ev) = self.finish() {
                            events.push(ev);
                        }
                    } else if b == b']' {
                        // OSC 도중 새 OSC 시작 — 기존 시퀀스는 미종결로 폐기.
                        self.begin_collect();
                    } else if b == ESC {
                        // 기존 시퀀스 폐기, 새 ESC 시퀀스 판별로 진입.
                        self.discard();
                        self.state = State::Esc;
                    } else {
                        // ESC + 기타 — OSC 가 중단되고 다른 시퀀스가 시작됐다. 폐기.
                        self.discard();
                        self.state = State::Ground;
                    }
                }
            }
        }
        events
    }

    /// 새 OSC payload 수집을 시작한다.
    fn begin_collect(&mut self) {
        self.buf.clear();
        self.overflow = false;
        self.state = State::Collect;
    }

    /// 미종결 시퀀스를 버리고 수집 상태를 초기화한다.
    fn discard(&mut self) {
        self.buf.clear();
        self.overflow = false;
    }

    /// 종결자를 만난 시점의 처리 — overflow 였으면 폐기, 아니면 파싱.
    fn finish(&mut self) -> Option<OscEvent> {
        let overflowed = self.overflow;
        let payload = std::mem::take(&mut self.buf);
        self.overflow = false;
        self.state = State::Ground;
        if overflowed {
            return None;
        }
        parse_payload(&payload)
    }
}

impl Default for OscScanner {
    fn default() -> Self {
        Self::new()
    }
}

/// 완성된 payload(`ESC ]` 와 종결자 사이 바이트)를 이벤트로 파싱한다.
/// 알 수 없는 코드·형식 불일치는 None (감지 대상 아님).
fn parse_payload(payload: &[u8]) -> Option<OscEvent> {
    let text = String::from_utf8_lossy(payload);
    let (code, rest) = match text.split_once(';') {
        Some((code, rest)) => (code, rest),
        None => (text.as_ref(), ""),
    };
    match code {
        // "2" 는 OSC 0(아이콘+제목)의 부분집합인 "창 제목만" 코드 — ConPTY 가 제목을
        // 이 코드로 재인코딩할 가능성에 대비해 별칭으로 취급한다(enum variant 는 불변).
        "0" | "2" => Some(OscEvent::Osc0Title(rest.to_string())),
        "7" => Some(OscEvent::Osc7Cwd(rest.to_string())),
        "9" => Some(OscEvent::Osc9Notify(rest.to_string())),
        "777" => {
            // urxvt 계열: `777;notify;title;body`. body 안의 `;` 는 body 에 포함.
            let mut parts = rest.splitn(3, ';');
            let kind = parts.next().unwrap_or("");
            if kind != "notify" {
                return None;
            }
            let title = parts.next().unwrap_or("").to_string();
            let body = parts.next().unwrap_or("").to_string();
            Some(OscEvent::Osc777Notify { title, body })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 편의 헬퍼 — 한 번에 feed 하고 이벤트만 반환.
    fn scan(bytes: &[u8]) -> Vec<OscEvent> {
        OscScanner::new().feed(bytes)
    }

    #[test]
    fn osc0_title_bel() {
        assert_eq!(
            scan(b"\x1b]0;my title\x07"),
            vec![OscEvent::Osc0Title("my title".into())]
        );
    }

    #[test]
    fn osc0_title_st() {
        assert_eq!(
            scan(b"\x1b]0;my title\x1b\\"),
            vec![OscEvent::Osc0Title("my title".into())]
        );
    }

    #[test]
    fn osc2_title_parsed_as_osc0title() {
        // OSC 2 는 OSC 0 과 동일하게 Osc0Title 로 파스된다(ConPTY 재인코딩 대비 별칭).
        assert_eq!(
            scan(b"\x1b]2;my title\x07"),
            vec![OscEvent::Osc0Title("my title".into())]
        );
    }

    #[test]
    fn osc0_title_still_parsed_after_osc2_alias_added() {
        // "2" 별칭 추가가 기존 OSC 0 처리에 회귀를 만들지 않는지 확인.
        assert_eq!(
            scan(b"\x1b]0;another title\x1b\\"),
            vec![OscEvent::Osc0Title("another title".into())]
        );
    }

    #[test]
    fn osc7_cwd_st() {
        assert_eq!(
            scan(b"\x1b]7;file://host/home/user\x1b\\"),
            vec![OscEvent::Osc7Cwd("file://host/home/user".into())]
        );
    }

    #[test]
    fn osc7_cwd_bel() {
        assert_eq!(
            scan(b"\x1b]7;file://h/p\x07"),
            vec![OscEvent::Osc7Cwd("file://h/p".into())]
        );
    }

    #[test]
    fn osc9_notify_bel() {
        assert_eq!(
            scan(b"\x1b]9;build done\x07"),
            vec![OscEvent::Osc9Notify("build done".into())]
        );
    }

    #[test]
    fn osc9_notify_st() {
        assert_eq!(
            scan(b"\x1b]9;hello\x1b\\"),
            vec![OscEvent::Osc9Notify("hello".into())]
        );
    }

    #[test]
    fn osc777_notify_bel() {
        assert_eq!(
            scan(b"\x1b]777;notify;Title;Body text\x07"),
            vec![OscEvent::Osc777Notify {
                title: "Title".into(),
                body: "Body text".into()
            }]
        );
    }

    #[test]
    fn osc777_notify_st() {
        assert_eq!(
            scan(b"\x1b]777;notify;T;B\x1b\\"),
            vec![OscEvent::Osc777Notify {
                title: "T".into(),
                body: "B".into()
            }]
        );
    }

    #[test]
    fn osc777_body_keeps_semicolons() {
        // body 안의 세미콜론은 분리하지 않고 body 에 포함해야 한다.
        assert_eq!(
            scan(b"\x1b]777;notify;T;a;b;c\x07"),
            vec![OscEvent::Osc777Notify {
                title: "T".into(),
                body: "a;b;c".into()
            }]
        );
    }

    #[test]
    fn osc777_non_notify_kind_ignored() {
        assert_eq!(scan(b"\x1b]777;other;T;B\x07"), vec![]);
    }

    #[test]
    fn split_across_two_feeds() {
        let mut s = OscScanner::new();
        assert_eq!(s.feed(b"\x1b]9;he"), vec![]);
        assert_eq!(
            s.feed(b"llo\x07"),
            vec![OscEvent::Osc9Notify("hello".into())]
        );
    }

    #[test]
    fn split_across_three_feeds() {
        let mut s = OscScanner::new();
        assert_eq!(s.feed(b"\x1b]77"), vec![]);
        assert_eq!(s.feed(b"7;notify;Ti"), vec![]);
        assert_eq!(
            s.feed(b"tle;Body\x07"),
            vec![OscEvent::Osc777Notify {
                title: "Title".into(),
                body: "Body".into()
            }]
        );
    }

    #[test]
    fn st_terminator_split_at_chunk_boundary() {
        // ST 의 ESC 와 `\` 가 서로 다른 청크로 나뉘는 경우.
        let mut s = OscScanner::new();
        assert_eq!(s.feed(b"\x1b]0;t\x1b"), vec![]);
        assert_eq!(s.feed(b"\\"), vec![OscEvent::Osc0Title("t".into())]);
    }

    #[test]
    fn esc_bracket_split_at_chunk_boundary() {
        // 시퀀스 도입부 `ESC ]` 가 청크 경계에서 나뉘는 경우.
        let mut s = OscScanner::new();
        assert_eq!(s.feed(b"\x1b"), vec![]);
        assert_eq!(s.feed(b"]9;n\x07"), vec![OscEvent::Osc9Notify("n".into())]);
    }

    #[test]
    fn oversized_payload_discarded() {
        let mut input = Vec::from(&b"\x1b]9;"[..]);
        input.extend(std::iter::repeat_n(b'a', 5000));
        input.push(0x07);
        let mut s = OscScanner::new();
        assert_eq!(s.feed(&input), vec![]);
        // 폐기 후에도 스캐너는 정상 동작해야 한다.
        assert_eq!(
            s.feed(b"\x1b]9;ok\x07"),
            vec![OscEvent::Osc9Notify("ok".into())]
        );
    }

    #[test]
    fn payload_at_exact_cap_kept() {
        // 정확히 4096 bytes 는 상한 이내 — 유지된다. payload 는 "9;" + 본문.
        let mut input = Vec::from(&b"\x1b]9;"[..]);
        input.extend(std::iter::repeat_n(b'a', 4094));
        input.push(0x07);
        let events = scan(&input);
        assert_eq!(events.len(), 1);
        match &events[0] {
            OscEvent::Osc9Notify(s) => assert_eq!(s.len(), 4094),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn non_osc_escape_sequences_ignored() {
        // SGR(색상) 등 CSI 시퀀스는 이벤트를 만들지 않는다.
        assert_eq!(scan(b"\x1b[31mred\x1b[0m\x1b(B"), vec![]);
    }

    #[test]
    fn osc_between_plain_text_and_csi() {
        assert_eq!(
            scan(b"hello \x1b[1mbold\x1b[0m \x1b]0;t\x07 world"),
            vec![OscEvent::Osc0Title("t".into())]
        );
    }

    #[test]
    fn multiple_events_in_one_feed() {
        assert_eq!(
            scan(b"\x1b]0;a\x07mid\x1b]9;b\x1b\\"),
            vec![
                OscEvent::Osc0Title("a".into()),
                OscEvent::Osc9Notify("b".into())
            ]
        );
    }

    #[test]
    fn unknown_osc_code_ignored() {
        assert_eq!(scan(b"\x1b]52;c;aGVsbG8=\x07"), vec![]);
    }

    #[test]
    fn invalid_utf8_lossy_converted() {
        assert_eq!(
            scan(b"\x1b]0;t\xff\x07"),
            vec![OscEvent::Osc0Title("t\u{FFFD}".into())]
        );
    }

    #[test]
    fn osc_aborted_by_new_osc() {
        // 미종결 OSC 도중 `ESC ]` 로 새 OSC 가 시작되면 앞 시퀀스는 폐기된다.
        assert_eq!(
            scan(b"\x1b]0;abandoned\x1b]9;n\x07"),
            vec![OscEvent::Osc9Notify("n".into())]
        );
    }

    #[test]
    fn osc_aborted_by_other_escape() {
        // OSC 도중 ESC + 기타(CSI 시작)면 시퀀스 중단 — 이벤트 없음.
        assert_eq!(
            scan(b"\x1b]0;abandoned\x1b[0m\x1b]9;n\x07"),
            vec![OscEvent::Osc9Notify("n".into())]
        );
    }

    #[test]
    fn osc_aborted_by_can() {
        // CAN(0x18) 은 진행 중인 OSC 를 중단시킨다 — 이후 텍스트를 삼키지 않는다.
        assert_eq!(
            scan(b"\x1b]0;abandoned\x18plain text\x1b]9;n\x07"),
            vec![OscEvent::Osc9Notify("n".into())]
        );
    }

    #[test]
    fn osc_aborted_by_sub() {
        assert_eq!(
            scan(b"\x1b]777;notify;t;b\x1aplain\x1b]9;n\x07"),
            vec![OscEvent::Osc9Notify("n".into())]
        );
    }

    #[test]
    fn empty_payload_fields() {
        assert_eq!(
            scan(b"\x1b]0;\x07"),
            vec![OscEvent::Osc0Title(String::new())]
        );
    }
}
