// This file is @generated by prost-build.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SpawnRequest {
    #[prost(string, tag = "1")]
    pub name: ::prost::alloc::string::String,
    #[prost(string, repeated, tag = "2")]
    pub args: ::prost::alloc::vec::Vec<::prost::alloc::string::String>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SpawnResponse {
    #[prost(message, optional, tag = "1")]
    pub job: ::core::option::Option<Job>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct LogsRequest {
    #[prost(string, tag = "1")]
    pub job_id: ::prost::alloc::string::String,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct LogFrame {
    #[prost(enumeration = "LogFrameKind", tag = "1")]
    pub kind: i32,
    #[prost(bytes = "vec", tag = "2")]
    pub bs: ::prost::alloc::vec::Vec<u8>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PsRequest {
    #[prost(string, tag = "1")]
    pub job_id: ::prost::alloc::string::String,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PsResponse {
    #[prost(message, optional, tag = "1")]
    pub job: ::core::option::Option<Job>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct KillRequest {
    #[prost(string, tag = "1")]
    pub job_id: ::prost::alloc::string::String,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct KillResponse {
    #[prost(message, optional, tag = "1")]
    pub job: ::core::option::Option<Job>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Job {
    /// the job identifier. guaranteed to be unique.
    #[prost(string, tag = "1")]
    pub id: ::prost::alloc::string::String,
    /// the user that created the job.
    #[prost(message, optional, tag = "2")]
    pub owner: ::core::option::Option<User>,
    /// when the job was successfully started. if the job could not be started, the Spawn api will
    /// return an error
    #[prost(message, optional, tag = "3")]
    pub started_at: ::core::option::Option<::prost_types::Timestamp>,
    /// when the job ended, regardless of whether it was killed, terminated, crashed.
    #[prost(message, optional, tag = "4")]
    pub ended_at: ::core::option::Option<::prost_types::Timestamp>,
    /// if a user kills a job, the server will record the time of the request as
    /// well as the user. if the job is not running when the server attempts to
    /// kill the job, this will be None. if the kill is successful it is not
    /// guaranteed that killed_at will be the exact same as ended_at.
    #[prost(message, optional, tag = "5")]
    pub killed: ::core::option::Option<Killed>,
    /// the current state of the job.
    #[prost(oneof = "job::State", tags = "6, 7, 8")]
    pub state: ::core::option::Option<job::State>,
}
/// Nested message and enum types in `Job`.
pub mod job {
    /// the current state of the job.
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum State {
        #[prost(message, tag = "6")]
        Running(super::JobRunningState),
        #[prost(message, tag = "7")]
        Exited(super::JobExitedState),
        #[prost(message, tag = "8")]
        Crashed(super::JobCrashedState),
    }
}
/// The id of a user is the common name of the certificate it presents
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct User {
    #[prost(string, tag = "1")]
    pub id: ::prost::alloc::string::String,
}
/// Details about a kill request. Even though only the job owner can kill it, if
/// we add the concept of an admin role the user field will indicate the user
/// that killed it if not the owner.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Killed {
    #[prost(message, optional, tag = "1")]
    pub killed_at: ::core::option::Option<::prost_types::Timestamp>,
    #[prost(message, optional, tag = "2")]
    pub user: ::core::option::Option<User>,
}
/// the job was started and is currently running
#[derive(Clone, Copy, PartialEq, ::prost::Message)]
pub struct JobRunningState {
    #[prost(uint32, tag = "1")]
    pub pid: u32,
}
/// the job exited normally
#[derive(Clone, Copy, PartialEq, ::prost::Message)]
pub struct JobExitedState {
    #[prost(int32, tag = "1")]
    pub exit_code: i32,
}
/// the job crashed without exiting. on unix, it's possible to determine the
/// reason for crashing by querying for the signal that the process received.
/// it is unknown right now what this will look like for windows, so this might
/// change as the implementation proceeds.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct JobCrashedState {
    #[prost(int32, tag = "1")]
    pub signal: i32,
    #[prost(string, tag = "2")]
    pub msg: ::prost::alloc::string::String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum LogFrameKind {
    Stdout = 0,
    Stderr = 1,
}
impl LogFrameKind {
    /// String value of the enum field names used in the ProtoBuf definition.
    ///
    /// The values are not transformed in any way and thus are considered stable
    /// (if the ProtoBuf definition does not change) and safe for programmatic use.
    pub fn as_str_name(&self) -> &'static str {
        match self {
            Self::Stdout => "STDOUT",
            Self::Stderr => "STDERR",
        }
    }
    /// Creates an enum from field names used in the ProtoBuf definition.
    pub fn from_str_name(value: &str) -> ::core::option::Option<Self> {
        match value {
            "STDOUT" => Some(Self::Stdout),
            "STDERR" => Some(Self::Stderr),
            _ => None,
        }
    }
}
/// Generated client implementations.
pub mod store_client {
    #![allow(
        unused_variables,
        dead_code,
        missing_docs,
        clippy::wildcard_imports,
        clippy::let_unit_value,
    )]
    use tonic::codegen::*;
    use tonic::codegen::http::Uri;
    #[derive(Debug, Clone)]
    pub struct StoreClient<T> {
        inner: tonic::client::Grpc<T>,
    }
    impl StoreClient<tonic::transport::Channel> {
        /// Attempt to create a new client by connecting to a given endpoint.
        pub async fn connect<D>(dst: D) -> Result<Self, tonic::transport::Error>
        where
            D: TryInto<tonic::transport::Endpoint>,
            D::Error: Into<StdError>,
        {
            let conn = tonic::transport::Endpoint::new(dst)?.connect().await?;
            Ok(Self::new(conn))
        }
    }
    impl<T> StoreClient<T>
    where
        T: tonic::client::GrpcService<tonic::body::BoxBody>,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + std::marker::Send + 'static,
        <T::ResponseBody as Body>::Error: Into<StdError> + std::marker::Send,
    {
        pub fn new(inner: T) -> Self {
            let inner = tonic::client::Grpc::new(inner);
            Self { inner }
        }
        pub fn with_origin(inner: T, origin: Uri) -> Self {
            let inner = tonic::client::Grpc::with_origin(inner, origin);
            Self { inner }
        }
        pub fn with_interceptor<F>(
            inner: T,
            interceptor: F,
        ) -> StoreClient<InterceptedService<T, F>>
        where
            F: tonic::service::Interceptor,
            T::ResponseBody: Default,
            T: tonic::codegen::Service<
                http::Request<tonic::body::BoxBody>,
                Response = http::Response<
                    <T as tonic::client::GrpcService<tonic::body::BoxBody>>::ResponseBody,
                >,
            >,
            <T as tonic::codegen::Service<
                http::Request<tonic::body::BoxBody>,
            >>::Error: Into<StdError> + std::marker::Send + std::marker::Sync,
        {
            StoreClient::new(InterceptedService::new(inner, interceptor))
        }
        /// Compress requests with the given encoding.
        ///
        /// This requires the server to support it otherwise it might respond with an
        /// error.
        #[must_use]
        pub fn send_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.send_compressed(encoding);
            self
        }
        /// Enable decompressing responses.
        #[must_use]
        pub fn accept_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.accept_compressed(encoding);
            self
        }
        /// Limits the maximum size of a decoded message.
        ///
        /// Default: `4MB`
        #[must_use]
        pub fn max_decoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_decoding_message_size(limit);
            self
        }
        /// Limits the maximum size of an encoded message.
        ///
        /// Default: `usize::MAX`
        #[must_use]
        pub fn max_encoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_encoding_message_size(limit);
            self
        }
        /// spawns a new job
        pub async fn spawn(
            &mut self,
            request: impl tonic::IntoRequest<super::SpawnRequest>,
        ) -> std::result::Result<tonic::Response<super::SpawnResponse>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static("/jobs.Store/Spawn");
            let mut req = request.into_request();
            req.extensions_mut().insert(GrpcMethod::new("jobs.Store", "Spawn"));
            self.inner.unary(req, path, codec).await
        }
        /// fetches the logs for a particular job
        pub async fn logs(
            &mut self,
            request: impl tonic::IntoRequest<super::LogsRequest>,
        ) -> std::result::Result<
            tonic::Response<tonic::codec::Streaming<super::LogFrame>>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static("/jobs.Store/Logs");
            let mut req = request.into_request();
            req.extensions_mut().insert(GrpcMethod::new("jobs.Store", "Logs"));
            self.inner.server_streaming(req, path, codec).await
        }
        /// queries job state for one of the user's jobs.
        pub async fn ps(
            &mut self,
            request: impl tonic::IntoRequest<super::PsRequest>,
        ) -> std::result::Result<tonic::Response<super::PsResponse>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static("/jobs.Store/Ps");
            let mut req = request.into_request();
            req.extensions_mut().insert(GrpcMethod::new("jobs.Store", "Ps"));
            self.inner.unary(req, path, codec).await
        }
        /// terminates a specific job. only the owner of the job is allowed to kill it.
        pub async fn kill(
            &mut self,
            request: impl tonic::IntoRequest<super::KillRequest>,
        ) -> std::result::Result<tonic::Response<super::KillResponse>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic::codec::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static("/jobs.Store/Kill");
            let mut req = request.into_request();
            req.extensions_mut().insert(GrpcMethod::new("jobs.Store", "Kill"));
            self.inner.unary(req, path, codec).await
        }
    }
}
/// Generated server implementations.
pub mod store_server {
    #![allow(
        unused_variables,
        dead_code,
        missing_docs,
        clippy::wildcard_imports,
        clippy::let_unit_value,
    )]
    use tonic::codegen::*;
    /// Generated trait containing gRPC methods that should be implemented for use with StoreServer.
    #[async_trait]
    pub trait Store: std::marker::Send + std::marker::Sync + 'static {
        /// spawns a new job
        async fn spawn(
            &self,
            request: tonic::Request<super::SpawnRequest>,
        ) -> std::result::Result<tonic::Response<super::SpawnResponse>, tonic::Status>;
        /// Server streaming response type for the Logs method.
        type LogsStream: tonic::codegen::tokio_stream::Stream<
                Item = std::result::Result<super::LogFrame, tonic::Status>,
            >
            + std::marker::Send
            + 'static;
        /// fetches the logs for a particular job
        async fn logs(
            &self,
            request: tonic::Request<super::LogsRequest>,
        ) -> std::result::Result<tonic::Response<Self::LogsStream>, tonic::Status>;
        /// queries job state for one of the user's jobs.
        async fn ps(
            &self,
            request: tonic::Request<super::PsRequest>,
        ) -> std::result::Result<tonic::Response<super::PsResponse>, tonic::Status>;
        /// terminates a specific job. only the owner of the job is allowed to kill it.
        async fn kill(
            &self,
            request: tonic::Request<super::KillRequest>,
        ) -> std::result::Result<tonic::Response<super::KillResponse>, tonic::Status>;
    }
    #[derive(Debug)]
    pub struct StoreServer<T> {
        inner: Arc<T>,
        accept_compression_encodings: EnabledCompressionEncodings,
        send_compression_encodings: EnabledCompressionEncodings,
        max_decoding_message_size: Option<usize>,
        max_encoding_message_size: Option<usize>,
    }
    impl<T> StoreServer<T> {
        pub fn new(inner: T) -> Self {
            Self::from_arc(Arc::new(inner))
        }
        pub fn from_arc(inner: Arc<T>) -> Self {
            Self {
                inner,
                accept_compression_encodings: Default::default(),
                send_compression_encodings: Default::default(),
                max_decoding_message_size: None,
                max_encoding_message_size: None,
            }
        }
        pub fn with_interceptor<F>(
            inner: T,
            interceptor: F,
        ) -> InterceptedService<Self, F>
        where
            F: tonic::service::Interceptor,
        {
            InterceptedService::new(Self::new(inner), interceptor)
        }
        /// Enable decompressing requests with the given encoding.
        #[must_use]
        pub fn accept_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.accept_compression_encodings.enable(encoding);
            self
        }
        /// Compress responses with the given encoding, if the client supports it.
        #[must_use]
        pub fn send_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.send_compression_encodings.enable(encoding);
            self
        }
        /// Limits the maximum size of a decoded message.
        ///
        /// Default: `4MB`
        #[must_use]
        pub fn max_decoding_message_size(mut self, limit: usize) -> Self {
            self.max_decoding_message_size = Some(limit);
            self
        }
        /// Limits the maximum size of an encoded message.
        ///
        /// Default: `usize::MAX`
        #[must_use]
        pub fn max_encoding_message_size(mut self, limit: usize) -> Self {
            self.max_encoding_message_size = Some(limit);
            self
        }
    }
    impl<T, B> tonic::codegen::Service<http::Request<B>> for StoreServer<T>
    where
        T: Store,
        B: Body + std::marker::Send + 'static,
        B::Error: Into<StdError> + std::marker::Send + 'static,
    {
        type Response = http::Response<tonic::body::BoxBody>;
        type Error = std::convert::Infallible;
        type Future = BoxFuture<Self::Response, Self::Error>;
        fn poll_ready(
            &mut self,
            _cx: &mut Context<'_>,
        ) -> Poll<std::result::Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, req: http::Request<B>) -> Self::Future {
            match req.uri().path() {
                "/jobs.Store/Spawn" => {
                    #[allow(non_camel_case_types)]
                    struct SpawnSvc<T: Store>(pub Arc<T>);
                    impl<T: Store> tonic::server::UnaryService<super::SpawnRequest>
                    for SpawnSvc<T> {
                        type Response = super::SpawnResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::SpawnRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as Store>::spawn(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = SpawnSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/jobs.Store/Logs" => {
                    #[allow(non_camel_case_types)]
                    struct LogsSvc<T: Store>(pub Arc<T>);
                    impl<
                        T: Store,
                    > tonic::server::ServerStreamingService<super::LogsRequest>
                    for LogsSvc<T> {
                        type Response = super::LogFrame;
                        type ResponseStream = T::LogsStream;
                        type Future = BoxFuture<
                            tonic::Response<Self::ResponseStream>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::LogsRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as Store>::logs(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = LogsSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.server_streaming(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/jobs.Store/Ps" => {
                    #[allow(non_camel_case_types)]
                    struct PsSvc<T: Store>(pub Arc<T>);
                    impl<T: Store> tonic::server::UnaryService<super::PsRequest>
                    for PsSvc<T> {
                        type Response = super::PsResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::PsRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as Store>::ps(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = PsSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/jobs.Store/Kill" => {
                    #[allow(non_camel_case_types)]
                    struct KillSvc<T: Store>(pub Arc<T>);
                    impl<T: Store> tonic::server::UnaryService<super::KillRequest>
                    for KillSvc<T> {
                        type Response = super::KillResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::KillRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as Store>::kill(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = KillSvc(inner);
                        let codec = tonic::codec::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                _ => {
                    Box::pin(async move {
                        let mut response = http::Response::new(empty_body());
                        let headers = response.headers_mut();
                        headers
                            .insert(
                                tonic::Status::GRPC_STATUS,
                                (tonic::Code::Unimplemented as i32).into(),
                            );
                        headers
                            .insert(
                                http::header::CONTENT_TYPE,
                                tonic::metadata::GRPC_CONTENT_TYPE,
                            );
                        Ok(response)
                    })
                }
            }
        }
    }
    impl<T> Clone for StoreServer<T> {
        fn clone(&self) -> Self {
            let inner = self.inner.clone();
            Self {
                inner,
                accept_compression_encodings: self.accept_compression_encodings,
                send_compression_encodings: self.send_compression_encodings,
                max_decoding_message_size: self.max_decoding_message_size,
                max_encoding_message_size: self.max_encoding_message_size,
            }
        }
    }
    /// Generated gRPC service name
    pub const SERVICE_NAME: &str = "jobs.Store";
    impl<T> tonic::server::NamedService for StoreServer<T> {
        const NAME: &'static str = SERVICE_NAME;
    }
}
