build type *args:
    cargo build --bin {{type}} --features {{type}} {{args}}
run type *args:
    cargo run --bin {{type}} --features {{type}} {{args}}
clippy type:
    cargo clippy --features {{type}}

blocking:
    @just run blocking
poll:
    @just run poll
io-uring:
    @just run io-uring --features io-uring-with-dep

clippy-all:
    @just clippy blocking
    @just clippy poll
    @just clippy io-uring-with-dep

run-all:
    @just blocking
    @just poll
    @just io-uring

build-release:
    @just build blocking --release
    @just build poll --release
    @just build io-uring --features io-uring-with-dep --release
    ls -l target/release/ | grep -E "blocking|poll|io-uring" | grep -vF ".d"
