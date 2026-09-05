//! winmux-remote: 폰 브라우저용 원격 표면(LAN, 폴링)의 HTTP 서버 로직.
//!
//! Tauri 에 의존하지 않는 순수 Rust 크레이트다. 서버를 Tauri 글루
//! (`apps/winmux/src-tauri`) 안에 두지 않는 이유는 테스트다 — 글루는 Linux 개발기에서
//! 컴파일되지 않고(webkit2gtk 부재, Windows 타깃 check 만 게이트) 거기 놓인 코드는
//! `cargo test` 로 한 줄도 돌릴 수 없다. 인증·rate limit·경로 판정처럼 틀리면 조용히
//! 위험해지는 로직이라 Linux 게이트에서 실제로 실행되는 자리에 둔다. 글루가 맡는 것은
//! 설정 읽기·토큰 파일 경로·서버 spawn·정적 자산 콜백·로그 싱크뿐이다.
//!
//! 계약: `docs/plans/remote-surface-plan.md` 3장.
//!
//! 이 크레이트가 밖으로 내보내는 것은 토큰 로딩뿐이다. HTTP 파싱·라우팅·rate limit 은
//! 서버(B2 의 `server`/`handlers`)만의 내부 부품이라 `pub(crate)` 로 닫아 둔다.

// B1 은 순수 모듈만 담는 청크라 이 세 모듈의 유일한 호출자(B2 의 server/handlers)가 아직
// 없다 — lib 타깃에서는 전부 미사용으로 잡힌다. 각 모듈의 계약은 자기 테스트가 지키고
// 있으며, B2 가 호출자를 들여오면 이 allow 는 사라진다.
#[allow(dead_code)]
mod http;
#[allow(dead_code)]
mod ratelimit;
#[allow(dead_code)]
mod routes;
mod token;

pub use token::{load_or_create_token, TokenError};
