fmt:
    cargo +nightly fmt

server:
    cargo run --bin server

client-echo:
    cargo run --bin client run echo hi | pbcopy

client-pw:
    cargo run --bin client run cat /etc/passwd | pbcopy

client-logs:
    cargo run --bin client -- logs "$(pbpaste)"
