//! Windows 平台集成：进程优先级、单实例守卫与开机自启动。
//!
//! 窗口定位、显示与深浅色检测都交给 GPUI；这里**不要**直接调用
//! `ShowWindow` 之类的 Win32 窗口函数。GPUI 在持有 `App` 可变借用时
//! 调用它们，会令窗口过程回调再次借用 `App` 而 panic。

use anyhow::{Context as _, Result};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, HIGH_PRIORITY_CLASS, SetPriorityClass,
};

const AUTOSTART_APP_NAME: &str = "SmartWeNetwork";

/// 提升进程优先级，让开机时能抢在其它程序之前完成认证。
pub fn set_high_priority() {
    // SAFETY: `GetCurrentProcess` 返回永不失效的伪句柄，无需关闭。
    let succeeded = unsafe { SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS) } != 0;
    if !succeeded {
        eprintln!("无法提升进程优先级");
    }
}

/// 单实例守卫：持有命名互斥体直到被丢弃。
pub struct SingleInstance(HANDLE);

impl SingleInstance {
    /// 取得守卫；已有其它实例运行时返回 `None`。
    pub fn acquire() -> Option<Self> {
        let name: Vec<u16> = "SmartWeNetworkSingleton\0".encode_utf16().collect();
        // SAFETY: `name` 以 NUL 结尾且在调用期间有效；安全属性传空指针表示默认。
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        // SAFETY: 紧接着读取同一线程上 `CreateMutexW` 留下的错误码。
        let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already_running {
            if !handle.is_null() {
                // SAFETY: `handle` 是刚创建/打开的有效句柄，且不再使用。
                unsafe { CloseHandle(handle) };
            }
            return None;
        }
        // 创建互斥体失败时放行：宁可多一个实例，也不要让认证工具无法启动。
        Some(Self(handle))
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: 句柄由 `CreateMutexW` 创建，仅在此处关闭一次。
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn launcher() -> Result<auto_launch::AutoLaunch> {
    let exe = std::env::current_exe().context("无法确定程序路径")?;
    auto_launch::AutoLaunchBuilder::new()
        .set_app_name(AUTOSTART_APP_NAME)
        .set_app_path(&exe.to_string_lossy())
        .build()
        .context("无法构建自启动项")
}

/// 设置开机自启动（HKCU Run 键）。
pub fn set_autostart(enabled: bool) -> Result<()> {
    let launcher = launcher()?;
    if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    }
    .context("无法修改自启动项")
}

/// 以注册表中的真实状态为准，避免配置与系统漂移。
pub fn is_autostart_enabled() -> bool {
    launcher()
        .and_then(|launcher| launcher.is_enabled().context("无法读取自启动项"))
        .unwrap_or(false)
}
