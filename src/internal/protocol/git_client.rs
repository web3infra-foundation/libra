//! Git protocol (git://) client that connects over TCP, advertises refs, and streams pack data.

use std::io::Error as IoError;

use bytes::{Bytes, BytesMut};
use futures_util::stream::{self, StreamExt};
use git_internal::errors::GitError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use url::Url;

use super::{
    DiscoveryResult, FetchStream, ProtocolClient, generate_upload_pack_content,
    parse_discovered_references,
};
use crate::git_protocol::{ServiceType, add_pkt_line_string};

const DEFAULT_GIT_PORT: u16 = 9418;

pub struct GitClient {
    host: String,
    port: u16,
    repo_path: String,
}

impl ProtocolClient for GitClient {
    fn from_url(url: &Url) -> Self {
        let host = url.host_str().unwrap_or_default().to_string();
        let port = url.port().unwrap_or(DEFAULT_GIT_PORT);
        let mut repo_path = url.path().to_string();
        if repo_path.ends_with('/') && repo_path.len() > 1 {
            repo_path.pop();
        }
        Self {
            host,
            port,
            repo_path,
        }
    }
}

impl GitClient {
    fn build_service_request(&self, service: ServiceType) -> Bytes {
        let mut buf = BytesMut::new();
        let request = format!("{service} {}\0host={}\0", self.repo_path, self.host);
        add_pkt_line_string(&mut buf, request);
        buf.freeze()
    }

    async fn open_stream(&self) -> Result<TcpStream, IoError> {
        TcpStream::connect((self.host.as_str(), self.port)).await
    }

    async fn read_advertisement(&self, stream: &mut TcpStream) -> Result<Bytes, IoError> {
        let mut buf = BytesMut::new();
        loop {
            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await?;
            let len_str = std::str::from_utf8(&len_buf)
                .map_err(|e| IoError::other(format!("Invalid pkt-line length: {e}")))?;
            let len = usize::from_str_radix(len_str, 16)
                .map_err(|e| IoError::other(format!("Invalid pkt-line length: {e}")))?;
            buf.extend_from_slice(&len_buf);
            if len == 0 {
                break;
            }
            let mut data = vec![0u8; len - 4];
            stream.read_exact(&mut data).await?;
            buf.extend_from_slice(&data);
        }
        Ok(buf.freeze())
    }

    pub async fn discovery_reference(
        &self,
        service: ServiceType,
    ) -> Result<DiscoveryResult, GitError> {
        let mut stream = self
            .open_stream()
            .await
            .map_err(|e| GitError::NetworkError(format!("Failed to connect: {e}")))?;
        let request = self.build_service_request(service);
        stream
            .write_all(&request)
            .await
            .map_err(|e| GitError::NetworkError(format!("Failed to send request: {e}")))?;
        let response = self
            .read_advertisement(&mut stream)
            .await
            .map_err(|e| GitError::NetworkError(format!("Failed to read response: {e}")))?;
        parse_discovered_references(response, service)
    }

    pub async fn fetch_objects(
        &self,
        have: &[String],
        want: &[String],
    ) -> Result<FetchStream, IoError> {
        let mut stream = self.open_stream().await?;
        let request = self.build_service_request(ServiceType::UploadPack);
        stream.write_all(&request).await?;
        self.read_advertisement(&mut stream).await?;

        let body = generate_upload_pack_content(have, want);
        stream.write_all(&body).await?;
        stream.flush().await?;

        let mut response = Vec::new();
        stream.read_to_end(&mut response).await?;
        Ok(stream::once(async move { Ok(Bytes::from(response)) }).boxed())
    }
}
