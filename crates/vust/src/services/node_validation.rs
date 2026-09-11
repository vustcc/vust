//! 节点写入边界校验：统一约束名称、标签、SSH 目标与部署参数。

use crate::types::{ApiError, ApiResult};

const MAX_NAME_CHARS: usize = 64;
const MAX_DESCRIPTION_CHARS: usize = 2_000;
const MAX_TAGS: usize = 32;
const MAX_TAG_CHARS: usize = 64;
const RESERVED_TAG_PREFIXES: &[&str] = &["vust.", "com.docker.compose."];

/// SSH 与部署参数的借用视图。
pub struct NodeConnectionInput<'a> {
    pub host: Option<&'a str>,
    pub ssh_port: Option<&'a str>,
    pub user: Option<&'a str>,
    pub auth_mode: Option<&'a str>,
    pub password: Option<&'a str>,
    pub private_key: Option<&'a str>,
    pub service_port: Option<&'a str>,
    pub install_dir: Option<&'a str>,
    pub controller_url: Option<&'a str>,
}

/// 校验必填节点名称。
pub fn validate_required_name(name: Option<&str>) -> ApiResult<()> {
    let name = name.map(str::trim).unwrap_or("");
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return invalid("node name must be 1-64 characters without control characters");
    }
    Ok(())
}

/// 校验可选节点名称。
pub fn validate_optional_name(name: Option<&str>) -> ApiResult<()> {
    match name {
        Some(_) => validate_required_name(name),
        None => Ok(()),
    }
}

/// 校验面向用户的描述。
pub fn validate_description(description: Option<&str>) -> ApiResult<()> {
    if description
        .is_some_and(|value| value.chars().count() > MAX_DESCRIPTION_CHARS || value.contains('\0'))
    {
        return invalid("node description is invalid or too long");
    }
    Ok(())
}

/// 校验节点分组名称。
pub fn validate_group(group: Option<&str>) -> ApiResult<()> {
    if group.is_some_and(|value| {
        let value = value.trim();
        value.is_empty() || value.chars().count() > 64 || value.chars().any(char::is_control)
    }) {
        return invalid("node group is invalid or too long");
    }
    Ok(())
}

/// 校验标签数量、重复项和内部保留前缀。
pub fn validate_tags(tags: Option<&[String]>) -> ApiResult<()> {
    let Some(tags) = tags else {
        return Ok(());
    };
    if tags.len() > MAX_TAGS {
        return invalid("node tags exceed the maximum count");
    }
    let mut normalized = Vec::with_capacity(tags.len());
    for tag in tags {
        let tag = tag.trim().to_ascii_lowercase();
        if tag.is_empty()
            || tag.chars().count() > MAX_TAG_CHARS
            || tag.chars().any(char::is_control)
            || RESERVED_TAG_PREFIXES
                .iter()
                .any(|prefix| tag.starts_with(prefix))
        {
            return invalid("node tags contain an invalid or reserved value");
        }
        normalized.push(tag);
    }
    normalized.sort_unstable();
    if normalized.windows(2).any(|pair| pair[0] == pair[1]) {
        return invalid("node tags must not contain duplicates");
    }
    Ok(())
}

/// 校验 SSH 与部署连接；`credentials_required` 用于区分创建和局部更新。
pub fn validate_connection(
    input: &NodeConnectionInput<'_>,
    credentials_required: bool,
) -> ApiResult<()> {
    validate_partial_connection(input)?;
    let host = input.host.map(str::trim).unwrap_or("");
    let user = input.user.map(str::trim).unwrap_or("");
    if host.is_empty() {
        return invalid("node SSH host is invalid");
    }
    if user.is_empty() {
        return invalid("node SSH user is invalid");
    }

    let auth_mode = input.auth_mode.unwrap_or("password");
    match auth_mode {
        "password"
            if credentials_required && input.password.map(str::trim).is_none_or(str::is_empty) =>
        {
            return invalid("SSH password must not be empty");
        }
        "key"
            if credentials_required
                && input.private_key.map(str::trim).is_none_or(str::is_empty) =>
        {
            return invalid("SSH private key must not be empty");
        }
        "password" | "key" => {}
        _ => unreachable!("partial connection validation rejects unknown authentication modes"),
    }

    Ok(())
}

/// 校验局部连接配置更新中实际出现的字段。
pub fn validate_partial_connection(input: &NodeConnectionInput<'_>) -> ApiResult<()> {
    if let Some(host) = input.host.map(str::trim)
        && (host.is_empty()
            || host.len() > 253
            || host.chars().any(char::is_whitespace)
            || host.contains("//"))
    {
        return invalid("node SSH host is invalid");
    }
    if let Some(user) = input.user.map(str::trim)
        && (user.is_empty() || user.len() > 64 || user.chars().any(char::is_whitespace))
    {
        return invalid("node SSH user is invalid");
    }
    if input.ssh_port.is_some() {
        validate_port(input.ssh_port, crate::config::DEFAULT_SSH_PORT, "SSH port")?;
    }
    if input.service_port.is_some() {
        validate_port(
            input.service_port,
            crate::config::DEFAULT_AGENT_PORT,
            "agent port",
        )?;
    }
    if input
        .auth_mode
        .is_some_and(|mode| !matches!(mode, "password" | "key"))
    {
        return invalid("SSH authentication mode is invalid");
    }

    if let Some(path) = input.install_dir.map(str::trim)
        && (path.is_empty()
            || !path.starts_with('/')
            || path == "/"
            || path.contains('\0')
            || path.len() > 512)
    {
        return invalid("install directory must be a safe absolute path");
    }

    if let Some(url) = input
        .controller_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| ApiError::BadRequest("controller URL is invalid".to_string()))?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !matches!(parsed.path(), "" | "/")
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return invalid("controller URL must be an HTTPS origin");
        }
    }
    Ok(())
}

fn validate_port(value: Option<&str>, default: &str, field: &str) -> ApiResult<u16> {
    let value = value.unwrap_or(default).trim();
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| ApiError::BadRequest(format!("{field} must be between 1 and 65535")))
}

fn invalid(message: &str) -> ApiResult<()> {
    Err(ApiError::BadRequest(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_connection<'a>() -> NodeConnectionInput<'a> {
        NodeConnectionInput {
            host: Some("2001:db8::10"),
            ssh_port: Some("22"),
            user: Some("root"),
            auth_mode: Some("key"),
            password: None,
            private_key: Some("~/.ssh/id_ed25519"),
            service_port: Some("7311"),
            install_dir: Some("/opt"),
            controller_url: Some("https://controller.example.com:7310"),
        }
    }

    #[test]
    fn accepts_supported_connection_shapes() {
        validate_connection(&valid_connection(), true).unwrap();
    }

    #[test]
    fn rejects_invalid_ports_and_authentication() {
        let mut input = valid_connection();
        input.ssh_port = Some("invalid");
        assert!(validate_connection(&input, true).is_err());
        input.ssh_port = Some("22");
        input.private_key = None;
        assert!(validate_connection(&input, true).is_err());
    }

    #[test]
    fn accepts_valid_partial_update_without_host_or_user() {
        let input = NodeConnectionInput {
            host: None,
            ssh_port: None,
            user: None,
            auth_mode: None,
            password: None,
            private_key: None,
            service_port: Some("7312"),
            install_dir: Some("/opt/vust"),
            controller_url: None,
        };
        validate_partial_connection(&input).unwrap();
    }

    #[test]
    fn rejects_reserved_and_duplicate_tags() {
        assert!(validate_tags(Some(&["vust.owner=system".to_string()])).is_err());
        assert!(validate_tags(Some(&["GPU".to_string(), "gpu".to_string()])).is_err());
    }
}
