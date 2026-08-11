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
    /// OSC 777 — `winmux-send;<target>;<base64>` 형식의 **pane 간 텍스트 전송**
    /// 요청 (에이전트 채널 — `scripts/wsl/skills/winmux-send/SKILL.md`).
    ///
    /// 알림과 달리 상태 델타가 아니라 **액션**이다: 글루가 코얼레싱 배치에 넣지
    /// 않고 즉시 대상 세션의 stdin 에 쓴다. 여기서는 필드를 나누기만 하고,
    /// base64 디코드·상한 검사는 [`crate::send`] 가, 대상 해석은
    /// [`crate::command::Dispatcher::resolve_send_target`] 가 맡는다.
    ///
    /// 세 번째 필드가 아예 없는 payload(`777;winmux-send;build`)는 형식 불일치라
    /// 이벤트가 되지 않는다 — 빈 전송을 조용히 성공시키지 않는다.
    ///
    /// # 보안
    ///
    /// 같은 머신에서 이 PTY 로 바이트를 흘릴 수 있는 **어떤 터미널 프로그램이든**
    /// 이 시퀀스로 다른 pane 에 입력을 넣을 수 있다. 본인 머신·협력 에이전트를
    /// 전제한 의도된 기능이며, 오발사 가드는 상한(32KiB)·유일 매치·자기 제외·
    /// 워크스페이스 격리 넷뿐이다 — 권한 경계가 아니다.
    Osc777Send { target: String, text_b64: String },
    /// OSC 777 — `winmux-query;<kind>;<base64>` 형식의 **질의** 요청 (에이전트
    /// 채널). `kind` 는 질의 종류(예: `list-tabs`), 세 번째 필드는 **회신 파일
    /// 경로**의 base64 다 — 경로 검증·디코드는 [`crate::send::decode_reply_path`]
    /// 가 맡는다.
    ///
    /// 전송(`winmux-send`)과 같은 규율이다: 상태 델타가 아니라 **액션**이라
    /// 코얼레싱 배치에 담기지 않고, 두 필드가 모두 있어야만 이벤트가 된다
    /// (`777;winmux-query;list-tabs` 는 형식 불일치로 떨어진다 — 회신 주소 없는
    /// 질의를 조용히 성공시키지 않는다).
    Osc777Query { kind: String, reply_b64: String },
    /// OSC 10/11 — 전경/배경색 **질의**(`ESC ] 10 ; ? ST`). `code` 는 10 = 전경,
    /// 11 = 배경이다. 앱(글루)이 우리 테마 값으로 **직접 응답**한다
    /// (`apps/winmux/src-tauri/src/sink.rs`).
    ///
    /// # 왜 우리가 답하나 (판단 반전의 근거)
    ///
    /// 종전 판단은 "응답기는 역효과"였다 — conhost 가 질의를 가로채 자기 색
    /// 테이블로 먼저 답하므로(ms/terminal#17729) 우리가 또 답하면 중복 응답이
    /// 되고, 그래서 `host.rs` 의 THEME_SYNC 로 conhost 의 테이블을 **미리 우리
    /// 값으로 세팅**하는 쪽만 택했다. **2026-08-11 실기 probe 가 그 전제를
    /// 뒤집었다: OSC 11 질의에 아무도 응답하지 않는다** (conhost 도, xterm 도).
    /// Codex 는 배경색을 못 받으면 입력창 배경을 아예 그리지 않으므로, 그 미응답이
    /// 실기 스크린샷의 "입력칸 구분 없음"으로 남는다.
    ///
    /// 그래서 계약을 뒤집었다: **질의가 conhost 를 통과해 우리 출력 스트림까지
    /// 도달하면** 그때 우리가 답한다. 도달하지 않으면(= conhost 가 삼키면) 이
    /// 이벤트 자체가 발생하지 않아 아무 일도 일어나지 않는다 — 무해한 조건부
    /// 응답이다. 어느 쪽이었는지는 글루의 진단 로그로 판별한다 (sink.rs).
    ///
    /// # set 형태는 감지하지 않는다
    ///
    /// `rest` 가 정확히 `"?"` 일 때만 이벤트다. 색값을 싣는 set 형태
    /// (`10;#cccccc`)는 종전대로 미감지 — 그대로 xterm 까지 passthrough 된다.
    /// `host.rs` 의 THEME_SYNC 가 내보내는 set 이 이 경로로 흘러들어도 값이
    /// TERMINAL_THEME 와 **같으므로** xterm 이 자기 테마를 같은 값으로 다시
    /// 세팅할 뿐 무해하다.
    OscColorQuery { code: u8 },
}

/// payload 상한 (bytes). 초과하는 시퀀스는 통째로 폐기한다 — 악성/폭주 입력 방어.
/// 64KiB 인 이유: `winmux-send` 의 텍스트 계약이 32KiB(디코드 후)이고 base64
/// 팽창(4/3) + 헤더를 더하면 payload 가 ~44KiB 까지 자란다 — 4096 이면 문서화된
/// 상한이 실효 ~3KB 로 무음 축소된다 (리뷰 finding). 버퍼는 세션당 진행 중
/// 시퀀스 1개뿐이라 메모리 상한은 세션 수 × 64KiB 로 유계다.
const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

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
        // 색상 **질의**만 이벤트다 (`10;?`·`11;?`) — 근거는 OscColorQuery rustdoc.
        // set 형태(`10;#cccccc`)는 여기서 걸러져 종전대로 passthrough 된다.
        "10" if rest == "?" => Some(OscEvent::OscColorQuery { code: 10 }),
        "11" if rest == "?" => Some(OscEvent::OscColorQuery { code: 11 }),
        "777" => {
            // urxvt 계열: `777;notify;title;body`. body 안의 `;` 는 body 에 포함.
            // `winmux-send` 는 winmux 자체 확장(pane 간 전송)이고, 그 외 kind 는
            // 감지 대상이 아니다 — 기존 `notify` 계약은 그대로다.
            let mut parts = rest.splitn(3, ';');
            match parts.next().unwrap_or("") {
                "notify" => {
                    let title = parts.next().unwrap_or("").to_string();
                    let body = parts.next().unwrap_or("").to_string();
                    Some(OscEvent::Osc777Notify { title, body })
                }
                "winmux-send" => {
                    // 두 필드가 모두 있어야 한다 (`?`) — 텍스트 필드가 없는 요청은
                    // 형식 불일치로 떨어뜨린다. base64 표준 알파벳에는 `;` 가 없어
                    // 세 번째 필드가 그대로 payload 다.
                    let target = parts.next()?.to_string();
                    let text_b64 = parts.next()?.to_string();
                    Some(OscEvent::Osc777Send { target, text_b64 })
                }
                "winmux-query" => {
                    // `winmux-send` 와 동일한 규율 — 두 필드 필수(`?`). 회신 경로
                    // 필드가 없는 질의는 답을 받을 곳이 없으므로 이벤트가 되지 않는다.
                    let kind = parts.next()?.to_string();
                    let reply_b64 = parts.next()?.to_string();
                    Some(OscEvent::Osc777Query { kind, reply_b64 })
                }
                _ => None,
            }
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
    fn osc777_send_parsed() {
        assert_eq!(
            scan(b"\x1b]777;winmux-send;build;Y2FyZ28gdGVzdAo=\x07"),
            vec![OscEvent::Osc777Send {
                target: "build".into(),
                text_b64: "Y2FyZ28gdGVzdAo=".into()
            }]
        );
    }

    #[test]
    fn osc777_send_st_terminated_and_split() {
        // ST 종결 + 청크 분할에서도 같은 계약 (알림과 같은 스캐너 경로).
        let mut s = OscScanner::new();
        assert_eq!(s.feed(b"\x1b]777;winmux-se"), vec![]);
        assert_eq!(
            s.feed(b"nd;my Tab;aGk=\x1b\\"),
            vec![OscEvent::Osc777Send {
                target: "my Tab".into(),
                text_b64: "aGk=".into()
            }]
        );
    }

    #[test]
    fn osc777_send_without_text_field_ignored() {
        // 대상만 있고 텍스트 필드가 없으면 이벤트가 되지 않는다 (빈 전송 금지).
        assert_eq!(scan(b"\x1b]777;winmux-send;build\x07"), vec![]);
        assert_eq!(scan(b"\x1b]777;winmux-send\x07"), vec![]);
    }

    #[test]
    fn osc777_send_keeps_trailing_semicolons_in_text_field() {
        // 세 번째 필드는 끝까지 텍스트다 — base64 에 `;` 는 없으므로 이 경우는
        // 디코드 단계에서 거부된다(파서는 자르지 않는다).
        assert_eq!(
            scan(b"\x1b]777;winmux-send;t;aGk=;x\x07"),
            vec![OscEvent::Osc777Send {
                target: "t".into(),
                text_b64: "aGk=;x".into()
            }]
        );
    }

    #[test]
    fn osc777_query_parsed() {
        assert_eq!(
            scan(b"\x1b]777;winmux-query;list-tabs;L3RtcC9yZXBseS5qc29u\x07"),
            vec![OscEvent::Osc777Query {
                kind: "list-tabs".into(),
                reply_b64: "L3RtcC9yZXBseS5qc29u".into()
            }]
        );
    }

    #[test]
    fn osc777_query_st_terminated_and_split() {
        // ST 종결 + 청크 분할에서도 같은 계약 (전송과 같은 스캐너 경로).
        let mut s = OscScanner::new();
        assert_eq!(s.feed(b"\x1b]777;winmux-qu"), vec![]);
        assert_eq!(
            s.feed(b"ery;list-tabs;aGk=\x1b\\"),
            vec![OscEvent::Osc777Query {
                kind: "list-tabs".into(),
                reply_b64: "aGk=".into()
            }]
        );
    }

    #[test]
    fn osc777_query_without_reply_field_ignored() {
        // 회신 경로 필드가 없으면 이벤트가 되지 않는다 (답을 보낼 곳이 없다).
        assert_eq!(scan(b"\x1b]777;winmux-query;list-tabs\x07"), vec![]);
        assert_eq!(scan(b"\x1b]777;winmux-query\x07"), vec![]);
    }

    #[test]
    fn osc777_query_keeps_unknown_kind_verbatim() {
        // 질의 종류의 해석은 상위 계층 몫이다 — 파서는 필드만 나눈다.
        assert_eq!(
            scan(b"\x1b]777;winmux-query;whatever;aGk=\x07"),
            vec![OscEvent::Osc777Query {
                kind: "whatever".into(),
                reply_b64: "aGk=".into()
            }]
        );
    }

    #[test]
    fn osc777_notify_contract_unchanged_by_send_kind() {
        // winmux-send 추가가 기존 notify 계약을 건드리지 않는지 (회귀 잠금).
        assert_eq!(
            scan(b"\x1b]777;notify;winmux:idle;done\x07"),
            vec![OscEvent::Osc777Notify {
                title: "winmux:idle".into(),
                body: "done".into()
            }]
        );
        // 비슷하지만 다른 kind 는 여전히 무시된다.
        assert_eq!(scan(b"\x1b]777;winmux-sendx;t;aGk=\x07"), vec![]);
        assert_eq!(scan(b"\x1b]777;send;t;aGk=\x07"), vec![]);
        assert_eq!(scan(b"\x1b]777;winmux-queryx;t;aGk=\x07"), vec![]);
        assert_eq!(scan(b"\x1b]777;query;t;aGk=\x07"), vec![]);
    }

    #[test]
    fn osc10_and_osc11_queries_detected() {
        // 전경(10)·배경(11) 질의 — BEL·ST 종결 양쪽.
        assert_eq!(
            scan(b"\x1b]10;?\x07"),
            vec![OscEvent::OscColorQuery { code: 10 }]
        );
        assert_eq!(
            scan(b"\x1b]11;?\x1b\\"),
            vec![OscEvent::OscColorQuery { code: 11 }]
        );
    }

    #[test]
    fn osc_color_query_split_across_feeds() {
        // 청크 경계에서 나뉘어도 같은 계약 (다른 OSC 와 같은 스캐너 경로).
        let mut s = OscScanner::new();
        assert_eq!(s.feed(b"\x1b]11"), vec![]);
        assert_eq!(
            s.feed(b";?\x07"),
            vec![OscEvent::OscColorQuery { code: 11 }]
        );
    }

    #[test]
    fn osc_color_set_not_detected() {
        // set 형태는 감지 대상이 아니다 — 그대로 xterm 까지 passthrough 된다
        // (host.rs THEME_SYNC 가 내보내는 값도 이 경로다).
        assert_eq!(scan(b"\x1b]10;#cccccc\x1b\\"), vec![]);
        assert_eq!(scan(b"\x1b]11;#1e1e1e\x1b\\"), vec![]);
        assert_eq!(scan(b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07"), vec![]);
        // `?` 가 섞여 있어도 payload 전체가 정확히 "?" 여야 질의다.
        assert_eq!(scan(b"\x1b]11;??\x07"), vec![]);
        assert_eq!(scan(b"\x1b]11;?;10;?\x07"), vec![]);
        assert_eq!(scan(b"\x1b]11\x07"), vec![]);
    }

    #[test]
    fn osc12_query_not_detected() {
        // 커서 색(12) 등 다른 색 슬롯은 응답 대상이 아니다 — 답할 값이 없다.
        assert_eq!(scan(b"\x1b]12;?\x07"), vec![]);
        assert_eq!(scan(b"\x1b]4;1;?\x07"), vec![]);
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
        input.extend(std::iter::repeat_n(b'a', 64 * 1024 + 1));
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
