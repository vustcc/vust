//! 配置读取：解析配置文件与默认值，提供全局访问。

use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const DEFAULT_PRODUCTION_HOME: &str = "/opt/vust";
pub const DEFAULT_SSH_PORT: &str = "22";
pub const DEFAULT_AGENT_PORT: &str = "7311";
pub const DEFAULT_CONTROLLER_PORT: u16 = 7310;
pub const DEFAULT_CONTROLLER_PORT_STR: &str = "7310";
pub const DEFAULT_AGENT_LISTEN_IP: &str = "[::]";
const DEFAULT_DEV_DATA_DIR: &str = ".vust";
const DEFAULT_AGENT_BINARY: &str = "/usr/local/bin/vust-agent";
const DEFAULT_VUSTCTL_PATH: &str = "/usr/local/bin/vustctl";
const DEFAULT_RELEASE_REPOSITORY: &str = "owner/vust";
const DEFAULT_RELEASE_CHANNEL: &str = "stable";
const DEFAULT_RELEASE_ASSET_PATTERN: &str = "vust-{version}-{target}.tar.gz";
pub const DEFAULT_RUNTIME_PROTOCOL_VERSION: &str = "1";

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn workspace_root() -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"));
    find_workspace_root(&cwd).unwrap_or(cwd)
}

fn production_home() -> PathBuf {
    env::var_os("VUST_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PRODUCTION_HOME))
}

fn default_agent_binary() -> String {
    if cfg!(debug_assertions) {
        let root = workspace_root();
        let debug = root.join("target/debug/vust-agent");
        if debug.exists() {
            return debug.to_string_lossy().into_owned();
        }
        return root
            .join("target/release/vust-agent")
            .to_string_lossy()
            .into_owned();
    }
    DEFAULT_AGENT_BINARY.to_string()
}

fn default_vustctl_path() -> String {
    if cfg!(debug_assertions) {
        let root = workspace_root();
        let debug = root.join("target/debug/vustctl");
        if debug.exists() {
            return debug.to_string_lossy().into_owned();
        }
        return root
            .join("target/release/vustctl")
            .to_string_lossy()
            .into_owned();
    }
    DEFAULT_VUSTCTL_PATH.to_string()
}

fn default_release_repository() -> String {
    DEFAULT_RELEASE_REPOSITORY.to_string()
}

fn default_release_channel() -> String {
    DEFAULT_RELEASE_CHANNEL.to_string()
}

fn default_release_asset_pattern() -> String {
    DEFAULT_RELEASE_ASSET_PATTERN.to_string()
}

fn default_download_cache_dir() -> String {
    let base = if cfg!(debug_assertions) {
        dev_base_dir()
    } else {
        production_home()
    };
    base.join("cache/releases").to_string_lossy().into_owned()
}

/// VUST 服务的运行配置（从文件或默认值加载）。
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VustConfig {
    pub cookie_secure: bool,
    #[serde(default)]
    pub agent_version_compatibility: AgentVersionCompatibilityConfig,
    #[serde(default)]
    pub upgrade: UpgradeConfig,
    #[serde(default = "default_agent_binary")]
    pub agent_binary: String,
    #[serde(default = "default_vustctl_path")]
    pub vustctl_path: String,
}

impl Default for VustConfig {
    fn default() -> Self {
        Self {
            cookie_secure: false,
            agent_version_compatibility: AgentVersionCompatibilityConfig::default(),
            upgrade: UpgradeConfig::default(),
            agent_binary: default_agent_binary(),
            vustctl_path: default_vustctl_path(),
        }
    }
}

/// 在线升级配置。
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeConfig {
    #[serde(default = "default_release_repository")]
    pub release_repository: String,
    #[serde(default = "default_release_channel")]
    pub release_channel: String,
    #[serde(default = "default_release_asset_pattern")]
    pub asset_pattern: String,
    #[serde(default = "default_download_cache_dir")]
    pub download_cache_dir: String,
    pub github_token: Option<String>,
    pub checksum_asset_name: Option<String>,
    #[serde(default = "default_true")]
    pub controller_auto_restart: bool,
}

impl Default for UpgradeConfig {
    fn default() -> Self {
        Self {
            release_repository: default_release_repository(),
            release_channel: default_release_channel(),
            asset_pattern: default_release_asset_pattern(),
            download_cache_dir: default_download_cache_dir(),
            github_token: None,
            checksum_asset_name: None,
            controller_auto_restart: true,
        }
    }
}

/// Agent 与主控的 SemVer 兼容策略配置。
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentVersionCompatibilityConfig {
    #[serde(default = "default_runtime_protocol_version")]
    pub runtime_protocol_version: String,
    #[serde(default = "default_min_supported_agent_version")]
    pub min_supported_agent_version: String,
    #[serde(default = "default_true")]
    pub require_semver: bool,
    #[serde(default = "default_false")]
    pub zero_major_requires_exact: bool,
    #[serde(default = "default_false")]
    pub zero_major_requires_prerelease_match: bool,
    #[serde(default = "default_true")]
    pub stable_requires_same_major: bool,
    #[serde(default = "default_true")]
    pub stable_disallow_agent_newer_than_controller: bool,
}

impl Default for AgentVersionCompatibilityConfig {
    fn default() -> Self {
        Self {
            runtime_protocol_version: default_runtime_protocol_version(),
            min_supported_agent_version: default_min_supported_agent_version(),
            require_semver: true,
            // 项目初期放宽限制以支持精细化在线升级，默认不强制要求 0.x.x 版本完全一致，后续项目文档明确后再行调整。
            zero_major_requires_exact: false,
            zero_major_requires_prerelease_match: false,
            stable_requires_same_major: true,
            stable_disallow_agent_newer_than_controller: true,
        }
    }
}

fn default_runtime_protocol_version() -> String {
    DEFAULT_RUNTIME_PROTOCOL_VERSION.to_string()
}

fn default_min_supported_agent_version() -> String {
    // 项目初期放宽限制，默认的最小支持 Agent 版本为基准版本
    "0.1.0-alpha.1".to_string()
}

static CONFIG: OnceLock<VustConfig> = OnceLock::new();

/// 读取配置并初始化全局缓存。
pub fn init() {
    let config = load_config().unwrap_or_default();
    let _ = CONFIG.set(config);
}

/// 返回全局配置实例，必要时使用默认值。
pub fn get() -> &'static VustConfig {
    CONFIG.get_or_init(VustConfig::default)
}

fn load_config() -> Option<VustConfig> {
    let raw = fs::read_to_string(config_path()).ok()?;
    toml::from_str(&raw).ok()
}

fn dev_base_dir() -> PathBuf {
    if let Some(value) = env::var_os("VUST_DATA_DIR") {
        return PathBuf::from(value);
    }
    if let Some(value) = env::var_os("VUST_DEV_HOME") {
        return PathBuf::from(value);
    }
    workspace_root().join(DEFAULT_DEV_DATA_DIR)
}

fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        if ancestor.join("crates").is_dir() && ancestor.join("frontend").is_dir() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn config_path() -> PathBuf {
    if let Some(value) = env::var_os("VUST_CONFIG") {
        return PathBuf::from(value);
    }
    if let Some(value) = env::var_os("VUST_CONFIG_DIR") {
        return PathBuf::from(value).join("vust.toml");
    }
    if cfg!(debug_assertions) {
        dev_base_dir().join("vust.toml")
    } else {
        production_home().join("config/vust.toml")
    }
}

/// 返回配置目录路径。
pub fn config_dir() -> PathBuf {
    if let Some(value) = env::var_os("VUST_CONFIG_DIR") {
        return PathBuf::from(value);
    }
    if cfg!(debug_assertions) {
        dev_base_dir().join("config")
    } else {
        production_home().join("config")
    }
}

/// 返回数据库数据目录路径。
pub fn data_dir() -> PathBuf {
    if let Some(value) = env::var_os("VUST_DB_DIR") {
        return PathBuf::from(value);
    }
    if cfg!(debug_assertions) {
        dev_base_dir().join("database")
    } else {
        production_home().join("database")
    }
}

/// 返回 PCAP 抓包文件存放目录路径。
pub fn pcap_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        dev_base_dir().join("data").join("pcaps")
    } else {
        production_home().join("data").join("pcaps")
    }
}

/// 返回证书文件存放目录路径。
pub fn certs_dir() -> PathBuf {
    if let Some(value) = env::var_os("VUST_CERTS_DIR") {
        return PathBuf::from(value);
    }
    if cfg!(debug_assertions) {
        dev_base_dir().join("config").join("certs")
    } else {
        production_home().join("config").join("certs")
    }
}
