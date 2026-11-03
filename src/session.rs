//! 认证会话模型：设置、认证阶段与网络信息的唯一持有者。
//!
//! `Session` 不认识窗口和托盘。它只通过 [`SessionEvent`] 说明“需要用户
//! 处理”“流程结束”这类语义，由外壳（`Shell`）决定怎么呈现；面板则
//! 观察它的状态来渲染。

use std::time::{Duration, Instant};

use anyhow::Result;
use gpui_kit::{AppContext as _, Context, EventEmitter, SharedString, Task};

use crate::config::{self, Credentials, Settings};
use crate::network::{self, NetworkInfo, ProbeResult};
use crate::platform;
use crate::portal::{LoginOutcome, OnlineState, PortalClient};

/// 网络在线后自动退出前的等待时间。
const AUTO_CLOSE_DELAY: Duration = Duration::from_secs(5);

/// 认证流程的当前阶段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// 正在检测网络。
    Checking,
    /// 不在公司网络，无需认证。
    OffCompanyNetwork,
    /// 在公司网络且已通过认证。
    Online,
    /// 在公司网络，但认证门户无法访问。
    PortalUnreachable,
    /// 门户等待凭据。
    NeedsLogin,
    /// 正在提交登录请求。
    SigningIn,
    /// 正在请求退出登录。
    SigningOut,
    /// 认证成功。
    SignedIn,
    /// 上一次登录失败，附带原因。
    Failed(SharedString),
}

impl Phase {
    /// 有请求在进行，不接受新的检测或登录。
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Checking | Self::SigningIn | Self::SigningOut)
    }

    /// 是否应当展示登录表单。
    pub fn shows_login_form(&self) -> bool {
        matches!(self, Self::NeedsLogin | Self::SigningIn | Self::Failed(_))
    }

    /// 是否可以退出登录。
    pub fn can_sign_out(&self) -> bool {
        matches!(self, Self::Online | Self::SignedIn)
    }

    /// 网络已在线（认证成功、已认证，或不在公司网络而无需认证）。
    pub fn is_online(&self) -> bool {
        matches!(
            self,
            Self::OffCompanyNetwork | Self::Online | Self::SignedIn
        )
    }
}

/// `Session` 向外壳报告的语义事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// 流程已结束，应用可在延时后退出。
    Finished { after: Duration },
    /// 用户又在操作了，取消待执行的自动退出。
    Resumed,
}

impl EventEmitter<SessionEvent> for Session {}

pub struct Session {
    settings: Settings,
    autostart: bool,
    phase: Phase,
    network: NetworkInfo,
    /// 需要告知用户的非致命问题，例如设置保存失败；由面板就地显示。
    notice: Option<SharedString>,
    /// 用户主动退出登录后，本次运行不再自动登录，否则会立刻又登录回去。
    auto_sign_in_allowed: bool,
    /// 正在进行的检测或登录；替换或丢弃即取消。
    request: Option<Task<()>>,
    /// 自动关闭的时刻；有值表示正在倒计时。
    auto_close_deadline: Option<Instant>,
}

impl Session {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut session = Self {
            settings: Settings::load(),
            autostart: platform::is_autostart_enabled(),
            phase: Phase::Checking,
            network: NetworkInfo::default(),
            notice: None,
            auto_sign_in_allowed: true,
            request: None,
            auto_close_deadline: None,
        };
        session.begin_check(cx);
        session
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn network(&self) -> &NetworkInfo {
        &self.network
    }

    /// 最近一次非致命问题的说明。
    pub fn notice(&self) -> Option<&SharedString> {
        self.notice.as_ref()
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn is_autostart(&self) -> bool {
        self.autostart
    }

    /// 重新检测网络与认证状态；已有请求在进行时忽略。
    pub fn recheck(&mut self, cx: &mut Context<Self>) {
        if !self.phase.is_busy() {
            self.notice = None;
            self.cancel_auto_close(cx);
            self.begin_check(cx);
        }
    }

    /// 提交登录；已有请求在进行时忽略。
    pub fn sign_in(&mut self, credentials: Credentials, cx: &mut Context<Self>) {
        self.begin_sign_in(credentials, cx);
    }

    /// 退出登录；成功后重新检测，回到待认证状态。
    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        if !self.phase.can_sign_out() {
            return;
        }
        self.phase = Phase::SigningOut;
        self.auto_sign_in_allowed = false;
        // 用户主动退出，别让“认证成功后自动退出”的计时把应用关掉。
        self.notice = None;
        self.cancel_auto_close(cx);
        let client = PortalClient::new(&self.settings);
        self.request = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { client.logout() }).await;
            this.update(cx, |session, cx| match result {
                Ok(()) => session.begin_check(cx),
                Err(err) => {
                    session.notify_problem("退出登录失败", &err, cx);
                    session.begin_check(cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// 开关“网络在线后自动关闭”。已经在线时开启立即开始计时，关闭则取消计时。
    pub fn set_auto_close(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.auto_close = enabled;
        self.persist_settings(cx);
        if enabled {
            self.schedule_auto_close(cx);
        } else {
            self.cancel_auto_close(cx);
        }
        cx.notify();
    }

    /// 用户在操作（例如打开设置），取消待执行的自动关闭。
    pub fn pause_auto_close(&mut self, cx: &mut Context<Self>) {
        self.cancel_auto_close(cx);
    }

    /// 倒计时还剩多少，`1.0` 表示刚开始，`0.0` 表示到点；没有倒计时为 `None`。
    pub fn auto_close_remaining(&self) -> Option<f32> {
        let left = self
            .auto_close_deadline?
            .saturating_duration_since(Instant::now());
        Some((left.as_secs_f32() / AUTO_CLOSE_DELAY.as_secs_f32()).min(1.0))
    }

    fn cancel_auto_close(&mut self, cx: &mut Context<Self>) {
        self.auto_close_deadline = None;
        cx.emit(SessionEvent::Resumed);
        cx.notify();
    }

    /// 网络在线且开启了自动关闭时，请求外壳在延时后退出。
    fn schedule_auto_close(&mut self, cx: &mut Context<Self>) {
        if self.settings.auto_close && self.phase.is_online() {
            self.auto_close_deadline = Some(Instant::now() + AUTO_CLOSE_DELAY);
            cx.emit(SessionEvent::Finished {
                after: AUTO_CLOSE_DELAY,
            });
        }
    }

    /// 勾选“记住密码”。取消时一并取消依赖它的“自动登录”，并删除已存密码。
    pub fn set_remember_password(&mut self, remember: bool, cx: &mut Context<Self>) {
        self.settings.remember_password = remember;
        if !remember {
            self.settings.auto_login = false;
            if let Some(username) = &self.settings.username {
                config::clear_password(username);
            }
        }
        self.persist_settings(cx);
        cx.notify();
    }

    /// 勾选“自动登录”。它需要保存的密码，因此隐含“记住密码”。
    pub fn set_auto_login(&mut self, auto_login: bool, cx: &mut Context<Self>) {
        self.settings.auto_login = auto_login;
        if auto_login {
            self.settings.remember_password = true;
        }
        self.persist_settings(cx);
        cx.notify();
    }

    pub fn set_autostart(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if let Err(err) = platform::set_autostart(enabled) {
            self.notify_problem("修改开机自启动失败", &err, cx);
        }
        // 以系统里的真实状态为准，修改失败时开关会弹回去。
        self.autostart = platform::is_autostart_enabled();
        cx.notify();
    }

    /// 修改认证地址并重新检测。地址必须以 `http://` 或 `https://` 开头。
    pub fn set_portal_url(&mut self, url: &str, cx: &mut Context<Self>) {
        let Some(url) = normalized_url(url) else {
            self.reject_url("认证地址", cx);
            return;
        };
        if url == self.settings.portal_url {
            return;
        }
        self.settings.portal_url = url;
        self.recheck(cx);
        self.persist_settings(cx);
        cx.notify();
    }

    /// 修改跳转地址并重新检测。
    pub fn set_redirect_url(&mut self, url: &str, cx: &mut Context<Self>) {
        let Some(url) = normalized_url(url) else {
            self.reject_url("跳转地址", cx);
            return;
        };
        if url == self.settings.redirect_url {
            return;
        }
        self.settings.redirect_url = url;
        self.recheck(cx);
        self.persist_settings(cx);
        cx.notify();
    }

    fn reject_url(&mut self, what: &str, cx: &mut Context<Self>) {
        self.notice = Some(format!("{what}需要以 http:// 或 https:// 开头").into());
        cx.notify();
    }

    fn begin_check(&mut self, cx: &mut Context<Self>) {
        self.phase = Phase::Checking;
        let client = PortalClient::new(&self.settings);
        self.request = Some(cx.spawn(async move |this, cx| {
            let probe = cx
                .background_spawn(async move { network::probe(&client) })
                .await;
            this.update(cx, |session, cx| session.finish_check(probe, cx))
                .ok();
        }));
        cx.notify();
    }

    fn finish_check(&mut self, probe: ProbeResult, cx: &mut Context<Self>) {
        self.network = probe.network;
        self.phase = match (probe.on_company_network, probe.portal) {
            (false, _) => Phase::OffCompanyNetwork,
            (true, OnlineState::Authenticated) => Phase::Online,
            (true, OnlineState::NeedsLogin) => Phase::NeedsLogin,
            (true, OnlineState::Unreachable) => Phase::PortalUnreachable,
        };
        match self.phase {
            Phase::Online | Phase::OffCompanyNetwork => self.schedule_auto_close(cx),
            Phase::NeedsLogin => {
                self.try_auto_sign_in(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// 凭据齐全且已开启自动登录时直接认证。返回是否发起了认证。
    fn try_auto_sign_in(&mut self, cx: &mut Context<Self>) -> bool {
        if !(self.auto_sign_in_allowed
            && self.settings.auto_login
            && self.settings.remember_password)
        {
            return false;
        }
        let Some(username) = self.settings.username.clone() else {
            return false;
        };
        let Some(password) = config::load_password(&username) else {
            return false;
        };
        self.begin_sign_in(Credentials { username, password }, cx);
        true
    }

    fn begin_sign_in(&mut self, credentials: Credentials, cx: &mut Context<Self>) {
        if self.phase.is_busy() {
            return;
        }
        self.phase = Phase::SigningIn;
        self.notice = None;
        let client = PortalClient::new(&self.settings);
        self.request = Some(cx.spawn(async move |this, cx| {
            let attempt = credentials.clone();
            let result = cx
                .background_spawn(async move { client.login(&attempt) })
                .await;
            this.update(cx, |session, cx| {
                session.finish_sign_in(credentials, result, cx);
            })
            .ok();
        }));
        cx.notify();
    }

    fn finish_sign_in(
        &mut self,
        credentials: Credentials,
        result: Result<LoginOutcome>,
        cx: &mut Context<Self>,
    ) {
        let failure = match result {
            Ok(LoginOutcome::Accepted) => {
                self.phase = Phase::SignedIn;
                self.remember(&credentials, cx);
                self.schedule_auto_close(cx);
                cx.notify();
                return;
            }
            Ok(LoginOutcome::Rejected(reason)) => reason,
            Err(err) => format!("{err:#}"),
        };
        self.phase = Phase::Failed(failure.into());
        cx.notify();
    }

    /// 认证成功后保存用户名，并按选项保存或清除密码。
    fn remember(&mut self, credentials: &Credentials, cx: &mut Context<Self>) {
        let previous = self.settings.username.replace(credentials.username.clone());
        if let Some(previous) = previous.filter(|name| *name != credentials.username) {
            config::clear_password(&previous);
        }
        if self.settings.remember_password {
            if let Err(err) = config::save_password(credentials) {
                self.notify_problem("保存密码失败", &err, cx);
            }
        } else {
            config::clear_password(&credentials.username);
        }
        self.persist_settings(cx);
    }

    fn persist_settings(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = self.settings.save() {
            self.notify_problem("保存设置失败", &err, cx);
        }
    }

    fn notify_problem(&mut self, what: &str, err: &anyhow::Error, cx: &mut Context<Self>) {
        eprintln!("{what}: {err:#}");
        self.notice = Some(format!("{what}：{err}").into());
        cx.notify();
    }
}

/// 去掉首尾空白与末尾的 `/`；不是 http(s) 地址时返回 `None`。
fn normalized_url(input: &str) -> Option<String> {
    let url = input.trim().trim_end_matches('/');
    let host = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    (!host.is_empty()).then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_phases_are_the_ones_that_auto_close() {
        assert!(Phase::Online.is_online());
        assert!(Phase::SignedIn.is_online());
        assert!(Phase::OffCompanyNetwork.is_online());
        assert!(!Phase::NeedsLogin.is_online());
        assert!(!Phase::PortalUnreachable.is_online());
        assert!(!Phase::Checking.is_online());
    }

    #[test]
    fn normalizes_http_urls_only() {
        assert_eq!(
            normalized_url(" http://172.16.0.5:90/ ").as_deref(),
            Some("http://172.16.0.5:90")
        );
        assert_eq!(normalized_url("172.16.0.5"), None);
        assert_eq!(normalized_url("http://"), None);
    }
}
