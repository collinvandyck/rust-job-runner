//! random helpers. used by the bins.

use anyhow::{Context, Result};
use std::path::Path;
use tonic::transport::{Certificate, Identity};

/// a common method that both client and server can use
pub fn init_tracing() {
    let writer = std::io::stderr;
    tracing_subscriber::fmt().with_writer(writer).init();
}

pub async fn read_identity(cert: impl AsRef<Path>, key: impl AsRef<Path>) -> Result<Identity> {
    let cert = read_path(cert.as_ref())
        .await
        .context("could not read cert")?;
    let key = read_path(key.as_ref())
        .await
        .context("could not read key")?;
    Ok(Identity::from_pem(cert, key))
}

pub async fn read_ca_cert(cert: impl AsRef<Path>) -> Result<Certificate> {
    let cert = read_path(cert.as_ref())
        .await
        .context("could not read ca cert")?;
    Ok(Certificate::from_pem(cert))
}

async fn read_path(p: impl AsRef<Path>) -> Result<Vec<u8>> {
    tokio::fs::read(p.as_ref())
        .await
        .with_context(|| format!("failed to read {:?}", p.as_ref()))
}
