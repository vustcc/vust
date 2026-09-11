//! 运行时监听配置：使用明文 JSON 保存 vust 监听地址，并通过文件系统权限进行安全隔离。

use anyhow::{Result, anyhow, bail};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_PRODUCTION_HOME: &str = "/opt/vust";
const DEFAULT_DEV_DATA_DIR: &str = ".vust";
const DEFAULT_HOST: &str = "::";
const DEFAULT_PORT: u16 = crate::config::DEFAULT_CONTROLLER_PORT;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenConfig {
    pub host: String,
    pub port: u16,
    pub public_host: Option<String>,
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            public_host: None,
        }
    }
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

fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        if ancestor.join("crates").is_dir() && ancestor.join("frontend").is_dir() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
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

fn config_path() -> PathBuf {
    if let Some(value) = env::var_os("VUST_RUNTIME_CONFIG") {
        return PathBuf::from(value);
    }
    if let Some(value) = env::var_os("VUST_CONFIG_DIR") {
        return PathBuf::from(value).join("runtime-listen.json");
    }
    if cfg!(debug_assertions) {
        dev_base_dir().join("runtime-listen.json")
    } else {
        production_home().join("config/runtime-listen.json")
    }
}

pub fn load_or_default() -> ListenConfig {
    match load() {
        Ok(cfg) => cfg,
        Err(err) => {
            let is_not_found = if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
                io_err.kind() == std::io::ErrorKind::NotFound
            } else {
                false
            };

            if is_not_found {
                let default_cfg = ListenConfig::default();
                if let Err(save_err) = save(&default_cfg) {
                    tracing::info!(
                        "Runtime listen config not found, failed to auto-save default config: {save_err}"
                    );
                } else {
                    tracing::info!(
                        "Runtime listen config not found, auto-saved default config successfully"
                    );
                }
                default_cfg
            } else {
                tracing::warn!(
                    "Failed to load runtime listen config, config might be corrupted or encrypted, resetting to defaults: {err}"
                );
                let default_cfg = ListenConfig::default();
                if let Err(save_err) = save(&default_cfg) {
                    tracing::error!("Failed to reset corrupted config: {save_err}");
                }
                default_cfg
            }
        }
    }
}

pub fn load() -> Result<ListenConfig> {
    let raw = fs::read_to_string(config_path())?;
    let config = serde_json::from_str::<ListenConfig>(&raw)?;
    Ok(config)
}

pub fn save(config: &ListenConfig) -> Result<()> {
    if config.host.trim().is_empty() {
        bail!("host cannot be empty");
    }
    if config.port == 0 {
        bail!("port must be in range 1-65535");
    }

    let content = serde_json::to_vec_pretty(config)?;
    let path = config_path();
    atomic_write(path.clone(), content)?;

    // 限制仅拥有者可读写 (0600)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.exists() {
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(0o600);
            let _ = fs::set_permissions(&path, perms);
        }
    }

    Ok(())
}

fn atomic_write(path: PathBuf, content: Vec<u8>) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("invalid target path: {}", path.display()))?;
    fs::create_dir_all(parent)?;

    let rand = SystemRandom::new();
    let mut suffix = [0_u8; 4];
    rand.fill(&mut suffix)
        .map_err(|_| anyhow!("failed to generate temp file suffix"))?;

    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("runtime"),
        hex::encode(suffix)
    ));

    fs::write(&tmp, content)?;
    fs::rename(tmp, path)?;
    Ok(())
}

use std::sync::{OnceLock, RwLock};

static ACTIVE_LISTEN_CONFIG: OnceLock<RwLock<ListenConfig>> = OnceLock::new();

/// 设置当前进程中实际活跃的监听配置（由 main 启动参数动态覆盖后决定）
pub fn set_active_config(config: ListenConfig) {
    if let Some(active) = ACTIVE_LISTEN_CONFIG.get() {
        if let Ok(mut guard) = active.write() {
            *guard = config;
        }
        return;
    }
    let _ = ACTIVE_LISTEN_CONFIG.set(RwLock::new(config));
}

/// 获取当前进程中实际活跃的监听配置，若未设置则 fallback 到从配置文件加载
pub fn get_active_config() -> ListenConfig {
    ACTIVE_LISTEN_CONFIG
        .get()
        .and_then(|config| config.read().ok().map(|guard| guard.clone()))
        .unwrap_or_else(load_or_default)
}
