//! runtrol 단일 바이너리. argv 디스패치만 한다.
//!
//! 여기에 로직을 넣지 않는다. 이 파일이 자라면 계층 게이트가 볼 수 없는 곳에서
//! 아키텍처가 무너진다. 명령 처리는 `runtrol-cli`, 데몬은 `runtrol-daemon` 이 소유한다.

fn main() {
    // M0 골격. 실제 디스패치는 `runtrol-cli` 와 `runtrol-daemon` 이 채워지면 붙인다.
    // 두 crate 를 지금 명시적으로 참조해 두는 이유: 참조 없는 의존은 `cargo shear` 가
    // 미사용으로 잡는데, 골격 단계에서 의존 방향 표를 먼저 세우는 것이 목적이기 때문이다.
    runtrol_cli::placeholder();
    runtrol_daemon::placeholder();
}
