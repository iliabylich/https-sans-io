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
    @just run io-uring

clippy-all:
    @just clippy blocking
    @just clippy poll
    @just clippy io-uring

run-all:
    @just run blocking
    @just run poll
    @just run io-uring

build-release:
    @just build blocking --release
    @just build poll --release
    @just build io-uring --release
    ls -l target/release/ | grep -E "blocking|poll|io-uring" | grep -vF ".d"
