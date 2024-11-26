# Overview

This is a grpc-based rust job runner. The client uses mTLS to authenticate to the server. The
client in its default configuration uses the `data/tls/client1` keypair which has been signed with a
CA cert that the server in its default configuration accepts.

Job state, including output streams, are stored in memory and are not durable. When the server
quits, any running jobs will be terminated.

The client can perform four operations:

1. `run`: spawns a new job on the server given the supplied command and args. The server will return
   a job id that can be used with the other apis.

2. `logs`: retrives the stdout/stderr logs for a job. The job can be either running or finished. If
   the job is still running, logs will be streamed to the client.

3. `ps`: retrieves the job state for a job. The job does not have to be running.

4. `kill`: sends a kill signal to the specified job.


# Platform

Support unix/linux platforms as well as windows.

This was a learning project and the goal was to get experience with the Win32 APIs, so I coded
the process management functionality against the Win32 APIs using the `windows-rs` bindings.

The `coord::spawn::Child` trait defines the interface for managing child processes:

```rust
type WaitReturn<'a> = Pin<Box<dyn Future<Output = io::Result<ExitStatus>> + Send + 'a>>;
type KillReturn<'a> = Pin<Box<dyn Future<Output = io::Result<()>> + Send + 'a>>;
pub trait OutputStream: AsyncRead + Unpin + Send {}
impl<T: AsyncRead + Unpin + Send> OutputStream for T {}

/// represents a spawned child process.
pub trait Child: Send {
    fn pid(&self) -> u32;
    fn stdout(&mut self) -> Box<dyn OutputStream>;
    fn stderr(&mut self) -> Box<dyn OutputStream>;
    fn wait(&mut self) -> WaitReturn<'_>;
    fn kill(&mut self) -> KillReturn<'_>;
}
```

On non-windows platforms, the implementation delegates to the `tokio` process APIs. On windows, this
is implemented using calls into the Win32 APIs.

# Examples

This should run out of the box assuming you have a valid rust setup.

## Server

```sh
❯ cargo run --bin server
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s
     Running `target/debug/server`
2024-11-26T00:00:06.612817Z  INFO jobs::grpc: Initialized rustls/aws-lc-rs crypto provider.
2024-11-26T00:00:06.612920Z  INFO jobs::grpc: Enabled cipher suites in priority order:
2024-11-26T00:00:06.612980Z  INFO jobs::grpc: - TLS13_AES_256_GCM_SHA384
2024-11-26T00:00:06.613057Z  INFO jobs::grpc: - TLS13_CHACHA20_POLY1305_SHA256
2024-11-26T00:00:06.613068Z  INFO jobs::grpc: - TLS13_AES_128_GCM_SHA256
2024-11-26T00:00:06.613082Z  INFO jobs::grpc: Starting server...
2024-11-26T00:00:06.613137Z  INFO jobs::grpc: Listening on 0.0.0.0:50051
```

## Client

```sh
➜ cargo run --bin client -- --help
Usage: client [OPTIONS] <COMMAND>

Commands:
  run
  logs
  ps
  kill
  help  Print this message or the help of the given subcommand(s)

Options:
      --cert <CERT>            the client cert [default: data/tls/client1.pem]
      --key <KEY>              the client key [default: data/tls/client1.key]
      --server-ca <SERVER_CA>  the server CA cert [default: data/tls/server_ca.pem]
      --domain <DOMAIN>        the domain against which the server cert is verified. defaults to the domain of the endpoint
      --endpoint <ENDPOINT>    the endpoint of the server [default: https://localhost:50051]
  -h, --help                   Print help

➜ cargo run --bin client run -- tokei --sort code
019365c8-7571-79f2-8991-9be713d63d56

❯ cargo run --bin client logs 019365c8-7571-79f2-8991-9be713d63d56
===============================================================================
 Language            Files        Lines         Code     Comments       Blanks
===============================================================================
 TOML                    3          156           99           40           17
 Shell                   2          116           84            8           24
 Protocol Buffers        1          117           68           24           25
-------------------------------------------------------------------------------
 Rust                   13         3556         3130          144          282
 |- Markdown             9          238            0          219           19
 (Total)                           3794         3130          363          301
-------------------------------------------------------------------------------
 Markdown                1           61            0           40           21
 |- Rust                 1           13           11            1            1
 (Total)                             74           11           41           22
===============================================================================
 Total                  20         4006         3381          256          369
===============================================================================

❯ cargo run --bin client ps 019365c8-7571-79f2-8991-9be713d63d56
Exited code=0

```

