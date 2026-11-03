//! 设置与凭据的持久化。
//!
//! 选项与用户名保存在用户配置目录下的 JSON 文件中；密码从不落盘明文，
//! 而是交给 Windows 凭据管理器（DPAPI 加密）。

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "smartwe-network";
const CONFIG_DIR: &str = "smartwe-network";
const CONFIG_FILE: &str = "config.json";

pub const DEFAULT_PORTAL_URL: &str = "http://172.16.0.5:90";
pub const DEFAULT_REDIRECT_URL: &str = "http://www.smartwe.com.cn/";

/// 用户可调的选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub username: Option<String>,
    pub remember_password: bool,
    pub auto_login: bool,
    /// 网络在线（含认证成功、非公司网络）后自动关闭程序。
    pub auto_close: bool,
    /// 认证门户地址，如 `http://172.16.0.5:90`。
    pub portal_url: String,
    /// 跳转地址：认证通过后门户会把浏览器带到这里，据此判断是否已认证。
    pub redirect_url: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            username: None,
            remember_password: false,
            auto_login: false,
            auto_close: true,
            portal_url: DEFAULT_PORTAL_URL.to_string(),
            redirect_url: DEFAULT_REDIRECT_URL.to_string(),
        }
    }
}

impl Settings {
    /// 读取设置；文件缺失或损坏时回退为默认值。
    pub fn load() -> Self {
        for path in config_paths() {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            return serde_json::from_str(&text).unwrap_or_else(|err| {
                eprintln!("设置文件 {} 已损坏，使用默认设置: {err}", path.display());
                Self::default()
            });
        }
        Self::default()
    }

    /// 保存设置。用户配置目录写不进去（被安全软件或权限拦截）时，
    /// 退而写到程序所在目录。
    pub fn save(&self) -> Result<()> {
        let text = serde_json::to_string_pretty(self).context("无法序列化设置")?;
        let mut last_error = None;
        for path in config_paths() {
            match write_file(&path, &text) {
                Ok(()) => return Ok(()),
                Err(err) => last_error = Some(err),
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("没有可用的配置位置")))
    }
}

fn write_file(path: &std::path::Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建配置目录 {}", parent.display()))?;
    }
    std::fs::write(path, text).with_context(|| format!("无法写入 {}", path.display()))
}

/// 配置文件的候选位置，按优先级排列。
fn config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = dirs::config_dir() {
        paths.push(dir.join(CONFIG_DIR).join(CONFIG_FILE));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
    {
        paths.push(dir.join(CONFIG_FILE));
    }
    paths
}

/// 门户登录凭据。不实现 `Debug`，避免密码误入日志。
#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// 把密码存入 Windows 凭据管理器。
pub fn save_password(credentials: &Credentials) -> Result<()> {
    keyring::Entry::new(KEYRING_SERVICE, &credentials.username)?
        .set_password(&credentials.password)
        .context("无法写入凭据管理器")
}

/// 读取某个用户名已保存的密码。
pub fn load_password(username: &str) -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, username)
        .and_then(|entry| entry.get_password())
        .ok()
}

/// 删除某个用户名已保存的密码；本就不存在时视为成功。
pub fn clear_password(username: &str) {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, username) {
        // 不存在条目只会返回 NoEntry，无需处理。
        let _ = entry.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_config_without_urls_gets_defaults() {
        let settings: Settings =
            serde_json::from_str(r#"{"username":"a","auto_login":true}"#).unwrap();
        assert_eq!(settings.username.as_deref(), Some("a"));
        assert!(settings.auto_login);
        assert_eq!(settings.portal_url, DEFAULT_PORTAL_URL);
        assert_eq!(settings.redirect_url, DEFAULT_REDIRECT_URL);
    }
}
