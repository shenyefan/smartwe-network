//! SmartWe 网络认证：出现在屏幕右下角的上网认证小窗口。
//!
//! 启动即检测网络并展示面板；网络在线后默认 5 秒自动退出。

// 发布版不要附带控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod config;
mod network;
mod panel;
mod platform;
mod portal;
mod portal_crypto;
mod session;
mod shell;

use gpui_kit::QuitMode;
use gpui_kit::component::Theme;

use crate::assets::AppAssets;
use crate::shell::Shell;

fn main() {
    // 高优先级：开机自启动时抢在其它程序之前完成认证。
    platform::set_high_priority();

    // 已有实例在运行，避免重复检测与重复登录。
    let Some(_instance) = platform::SingleInstance::acquire() else {
        return;
    };

    gpui_kit::application()
        .with_assets(AppAssets)
        // 退出由外壳显式决定。
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::sync_system_appearance(None, cx);

            let shell = Shell::install(cx);
            shell.update(cx, |shell, cx| shell.open_panel(cx));
        });
}
