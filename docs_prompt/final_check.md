# Final Check

모든 과정 완료 후:

(1) cargo test
    cargo clippy --locked --all-targets -- -D warnings -W clippy::perf -W clippy::complexity
    오류/경고 확인 후 전체 수정해줘

(2) 도커 모두 내리고
    cargo fmt --all 하고
    git에 pr 없이 commit, push 해줘
