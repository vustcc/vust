//! 安全 HTTP 客户端构建工具：提供基于项目 mTLS 证书配置的客户端构建。

use crate::certs::{AGENT_CA_CERT_PEM, issue_client_cert};
use reqwest::{Certificate, Client, ClientBuilder, Identity};
use std::time::Duration;

/// 默认连接超时时间（秒）
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// 默认请求超时时间（秒）
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// 创建带有 mTLS 客户端证书与 CA 根证书配置的 `ClientBuilder`。
///
/// 默认配置：
/// - 使用 rustls-tls
/// - 禁用系统代理 (no_proxy)
/// - 允许无效的证书主机名 (danger_accept_invalid_hostnames(true))
///
/// # 参数
/// * `common_name` - 证书的主题名称 (Common Name)
///
/// # 错误
/// 如果证书签发失败或 PEM 证书解析失败，将返回错误。
pub fn mtls_client_builder(common_name: &str) -> anyhow::Result<ClientBuilder> {
    let issued = issue_client_cert(common_name)?;
    let mut identity_pem = Vec::new();
    identity_pem.extend_from_slice(&issued.cert_pem);
    identity_pem.extend_from_slice(&issued.key_pem);
    let identity = Identity::from_pem(&identity_pem)?;
    let ca = Certificate::from_pem(AGENT_CA_CERT_PEM)?;

    Ok(Client::builder()
        .identity(identity)
        .add_root_certificate(ca)
        .use_rustls_tls()
        .no_proxy()
        .danger_accept_invalid_hostnames(true))
}

/// 创建一个带有 mTLS 配置和默认超时的 `reqwest::Client`。
///
/// 默认超时：
/// - 连接超时：3 秒
/// - 请求超时：20 秒
///
/// # 参数
/// * `common_name` - 证书的主题名称 (Common Name)
///
/// # 错误
/// 如果客户端构建失败或证书处理失败，将返回错误。
pub fn build_mtls_client(common_name: &str) -> anyhow::Result<Client> {
    build_mtls_client_with_timeouts(
        common_name,
        DEFAULT_CONNECT_TIMEOUT,
        DEFAULT_REQUEST_TIMEOUT,
    )
}

/// 创建一个带有 mTLS 配置和指定超时的 `reqwest::Client`。
///
/// # 参数
/// * `common_name` - 证书的主题名称 (Common Name)
/// * `connect_timeout` - 连接超时时间
/// * `request_timeout` - 请求超时时间
///
/// # 错误
/// 如果客户端构建失败或证书处理失败，将返回错误。
pub fn build_mtls_client_with_timeouts(
    common_name: &str,
    connect_timeout: Duration,
    request_timeout: Duration,
) -> anyhow::Result<Client> {
    Ok(mtls_client_builder(common_name)?
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()?)
}

/// 创建带有 CA 根证书配置的单向 TLS `ClientBuilder`。
///
/// 默认配置：
/// - 使用 rustls-tls
/// - 禁用系统代理 (no_proxy)
/// - 允许无效的证书主机名 (danger_accept_invalid_hostnames(true))
///
/// # 错误
/// 如果 CA 根证书解析失败，将返回错误。
pub fn tls_client_builder() -> anyhow::Result<ClientBuilder> {
    let ca = Certificate::from_pem(AGENT_CA_CERT_PEM)?;

    Ok(Client::builder()
        .add_root_certificate(ca)
        .use_rustls_tls()
        .no_proxy()
        .danger_accept_invalid_hostnames(true))
}

/// 创建带有 CA 根证书配置的单向 TLS `reqwest::Client`。
///
/// # 错误
/// 如果客户端构建失败或证书处理失败，将返回错误。
pub fn build_tls_client() -> anyhow::Result<Client> {
    Ok(tls_client_builder()?.build()?)
}
