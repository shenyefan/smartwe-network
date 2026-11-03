//! 图标资源：默认组件图标，外加面板额外用到的几枚。

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

icon_assets!(ExtraIcons, [Lock, LogOut, WifiOff]);

/// 默认图标集与 [`ExtraIcons`] 的合并；先查额外图标。
pub struct AppAssets;

/// 软件内使用的无底图标。
pub const LOGO: &str = "logo.svg";

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == LOGO {
            return Ok(Some(Cow::Borrowed(include_bytes!("../assets/logo.svg"))));
        }
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.push(LOGO.into());
        paths.extend(ExtraIcons.list(path)?);
        Ok(paths)
    }
}
