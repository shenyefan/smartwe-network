//! 认证面板：右下角的无边框磨砂玻璃窗口。
//!
//! 主页只有一个状态、一行网络信息，需要时才出现登录表单；右上角是设置与
//! 关闭。设置页收纳开机自启动、自动登录、联网后自动关闭与两个地址。面板是 [`Session`] 的
//! 一个视图：设置与阶段以 `Session` 为唯一真源，本视图只持有页面选择与
//! 输入框状态。

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, Theme,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    spinner::Spinner,
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, WindowControlArea,
    div, prelude::FluentBuilder as _,
};

use crate::assets::LOGO;
use crate::config::{self, Credentials};
use crate::session::{Phase, Session};

/// 窗口底色的不透明度：透出亚克力模糊的桌面，深浅色主题各自成色。
const GLASS_ALPHA: f32 = 0.6;

/// 跟随系统深浅色，并把窗口底色调成半透明。
/// 每次同步都会重载主题颜色，因此必须成对调用。
fn sync_glass_theme(window: &mut Window, cx: &mut gpui_kit::App) {
    Theme::sync_system_appearance(Some(window), cx);
    Theme::update(cx, |theme| theme.background.a = GLASS_ALPHA);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Main,
    Settings,
}

pub struct PanelView {
    session: Entity<Session>,
    page: Page,
    username: Entity<InputState>,
    password: Entity<InputState>,
    portal_url: Entity<InputState>,
    redirect_url: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl PanelView {
    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = session.read(cx).settings().clone();
        let stored_password = settings
            .username
            .as_deref()
            .filter(|_| settings.remember_password)
            .and_then(config::load_password)
            .unwrap_or_default();
        let username = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("用户名")
                .default_value(settings.username.clone().unwrap_or_default())
        });
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("密码")
                .masked(true)
                .default_value(stored_password)
        });
        let portal_url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("http://172.16.0.5:90")
                .default_value(settings.portal_url.clone())
        });
        let redirect_url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("http://www.smartwe.com.cn")
                .default_value(settings.redirect_url.clone())
        });

        // 跟随系统深浅色：打开时同步一次，之后随系统切换。
        sync_glass_theme(window, cx);
        let mut subscriptions = vec![
            window.observe_window_appearance(sync_glass_theme),
            cx.observe(&session, |_, _, cx| cx.notify()),
        ];
        for input in [&username, &password] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event, _, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.submit(cx),
                    InputEvent::Focus | InputEvent::Blur => {}
                },
            ));
        }
        // 地址输入框在回车或失焦时才提交，免得每敲一个字就重新检测。
        for input in [&portal_url, &redirect_url] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event, _, cx| match event {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_urls(cx),
                    InputEvent::Change | InputEvent::Focus => {}
                },
            ));
        }

        Self {
            session,
            page: Page::Main,
            username,
            password,
            portal_url,
            redirect_url,
            _subscriptions: subscriptions,
        }
    }

    /// 输入框里当前的凭据；任一项为空时为 `None`。
    fn credentials(&self, cx: &Context<Self>) -> Option<Credentials> {
        let username = self.username.read(cx).value().trim().to_string();
        let password = self.password.read(cx).value().to_string();
        (!username.is_empty() && !password.is_empty()).then_some(Credentials { username, password })
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.session.read(cx).phase().is_busy() {
            return;
        }
        if let Some(credentials) = self.credentials(cx) {
            self.session
                .update(cx, |session, cx| session.sign_in(credentials, cx));
        }
    }

    /// 把地址输入框的内容交给会话；无变化时会话自己忽略。
    fn commit_urls(&mut self, cx: &mut Context<Self>) {
        let portal = self.portal_url.read(cx).value().to_string();
        let site = self.redirect_url.read(cx).value().to_string();
        self.session.update(cx, |session, cx| {
            session.set_portal_url(&portal, cx);
            session.set_redirect_url(&site, cx);
        });
    }

    fn show(&mut self, page: Page, cx: &mut Context<Self>) {
        if page == Page::Settings {
            self.session
                .update(cx, |session, cx| session.pause_auto_close(cx));
        }
        self.page = page;
        cx.notify();
    }
}

impl Render for PanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let countdown = self.session.read(cx).auto_close_remaining();
        if countdown.is_some() {
            // 倒计时期间逐帧重绘，进度条才会平滑回缩。
            window.request_animation_frame();
        }
        let phase = self.session.read(cx).phase().clone();
        let body = match self.page {
            Page::Main => self.render_main(&phase, cx).into_any_element(),
            Page::Settings => self.render_settings(cx).into_any_element(),
        };
        let theme = cx.theme();
        let (foreground, primary) = (theme.foreground, theme.primary);
        v_flex()
            .id("panel")
            .relative()
            .size_full()
            .px_5()
            .pb_5()
            .pt_2()
            .gap_3()
            .border_1()
            .border_color(foreground.opacity(0.12))
            .text_color(foreground)
            .child(self.render_header(cx))
            .child(body)
            .when_some(countdown, |this, remaining| {
                // 紧贴窗口底边的倒计时条，向左回缩到消失。
                this.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .h_1()
                        .w(gpui_kit::relative(remaining))
                        .bg(primary),
                )
            })
    }
}

impl PanelView {
    /// 顶栏：左侧是拖动区域（设置页上还有返回），右侧是设置与关闭。
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let in_settings = self.page == Page::Settings;
        let busy = self.session.read(cx).phase().is_busy();
        h_flex()
            .h_9()
            .gap_1()
            .when(in_settings, |this| {
                this.child(
                    Button::new("back")
                        .ghost()
                        .small()
                        .icon(IconName::ArrowLeft)
                        .on_click(cx.listener(|this, _, _, cx| this.show(Page::Main, cx))),
                )
            })
            .child(
                // 整块空白都能拖动窗口。
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .window_control_area(WindowControlArea::Drag)
                    .gap_2()
                    .text_sm()
                    .text_color(muted)
                    .when(!in_settings, |this| {
                        this.child(Icon::empty().path(LOGO).size_4())
                    })
                    .child(if in_settings {
                        "设置"
                    } else {
                        "网络认证"
                    }),
            )
            .when(!in_settings, |this| {
                this.child(
                    Button::new("settings")
                        .ghost()
                        .small()
                        .icon(IconName::Settings)
                        .on_click(cx.listener(|this, _, _, cx| this.show(Page::Settings, cx))),
                )
            })
            .when(!in_settings, |this| {
                this.child(
                    Button::new("refresh")
                        .ghost()
                        .small()
                        .icon(IconName::RefreshCw)
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.session.update(cx, |session, cx| session.recheck(cx));
                        })),
                )
            })
            .child(
                Button::new("close")
                    .ghost()
                    .small()
                    .icon(IconName::Close)
                    .on_click(|_, window, _| window.remove_window()),
            )
    }

    fn render_main(&self, phase: &Phase, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_1()
            .min_h_0()
            .gap_5()
            // 没有输入框时，状态在窗口中央；要登录时靠上，把下方留给表单。
            .child(self.render_status(phase, !phase.shows_login_form(), cx))
            .when(phase.shows_login_form(), |this| this.child(div().flex_1()))
            .when(phase.shows_login_form(), |this| {
                this.child(self.render_login_form(phase, cx))
            })
            .child(self.render_actions(phase, cx))
            .child(self.render_info_line(self.main_info(phase, cx), cx))
    }

    /// 底部那一行的内容：有错误显示错误，否则显示网络信息。
    fn main_info(&self, phase: &Phase, cx: &Context<Self>) -> Option<(SharedString, bool)> {
        match phase {
            Phase::Failed(reason) => Some((reason.clone(), true)),
            _ => self
                .session
                .read(cx)
                .notice()
                .map(|notice| (notice.clone(), true))
                .or_else(|| self.network_line(cx).map(|line| (line.into(), false))),
        }
    }

    /// 窗口最底部居中的一行小字：错误用警示色，网络信息用弱化色。
    fn render_info_line(
        &self,
        info: Option<(SharedString, bool)>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        div()
            .pt_2()
            .text_xs()
            .text_center()
            .when_some(info, |this, (text, is_error)| {
                this.text_color(if is_error { danger } else { muted.opacity(0.8) })
                    .child(text)
            })
    }

    /// 图标、标题与一行说明。
    fn render_status(
        &self,
        phase: &Phase,
        centered: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let auto_close = self.session.read(cx).settings().auto_close;
        let theme = cx.theme();
        let (icon, tone, title, detail): (IconName, Hsla, &str, SharedString) = match phase {
            Phase::Checking => (
                IconName::Network,
                theme.muted_foreground,
                "正在检测",
                "请稍候".into(),
            ),
            Phase::OffCompanyNetwork => (
                IconName::WifiOff,
                theme.muted_foreground,
                "非公司网络",
                "无需认证".into(),
            ),
            Phase::Online => (
                IconName::CircleCheck,
                theme.success,
                "已连接",
                "公司网络已认证".into(),
            ),
            Phase::PortalUnreachable => (
                IconName::TriangleAlert,
                theme.danger,
                "门户无法访问",
                "已连上公司网络，但认证门户没有响应".into(),
            ),
            Phase::NeedsLogin => (
                IconName::Lock,
                theme.warning,
                "需要认证",
                "登录后即可上网".into(),
            ),
            Phase::SigningIn => (IconName::Lock, theme.warning, "正在认证", "请稍候".into()),
            Phase::SigningOut => (
                IconName::LogOut,
                theme.muted_foreground,
                "正在退出登录",
                "请稍候".into(),
            ),
            Phase::SignedIn => (
                IconName::CircleCheck,
                theme.success,
                "认证成功",
                if auto_close {
                    "即将自动关闭".into()
                } else {
                    "已可以上网".into()
                },
            ),
            Phase::Failed(_) => (
                IconName::CircleX,
                theme.danger,
                "认证失败",
                "请检查后重试".into(),
            ),
        };
        let muted = theme.muted_foreground;

        v_flex()
            .gap_4()
            // 居中时占满剩余高度，内容上下左右都居中。
            .when(centered, |this| {
                this.flex_1().items_center().justify_center().text_center()
            })
            .child(
                h_flex()
                    .size_12()
                    .justify_center()
                    .rounded_full()
                    .bg(tone.opacity(0.16))
                    .child(if phase.is_busy() {
                        Spinner::new().large().into_any_element()
                    } else {
                        Icon::new(icon).size_6().text_color(tone).into_any_element()
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .when(centered, |this| this.items_center())
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(div().text_sm().text_color(muted).child(detail)),
            )
    }

    /// “适配器 · IPv4”；没有网络时为 `None`。
    fn network_line(&self, cx: &Context<Self>) -> Option<String> {
        let network = self.session.read(cx).network();
        let ip = network.ipv4.map(|ip| ip.to_string());
        match (network.adapter.as_str(), ip) {
            ("", None) => None,
            ("", Some(ip)) => Some(ip),
            (adapter, None) => Some(adapter.to_string()),
            (adapter, Some(ip)) => Some(format!("{adapter} · {ip}")),
        }
    }

    fn render_login_form(&self, phase: &Phase, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = phase.is_busy();
        let can_submit = !busy && self.credentials(cx).is_some();

        v_flex()
            .gap_3()
            .child(
                Input::new(&self.username)
                    .disabled(busy)
                    .prefix(Icon::new(IconName::User).small()),
            )
            .child(
                Input::new(&self.password)
                    .disabled(busy)
                    .prefix(Icon::new(IconName::Lock).small())
                    .mask_toggle(),
            )
            // 登录是这个表单的默认提交，也是 Enter 的动作，因此用 primary。
            .child(
                Button::new("sign-in")
                    .primary()
                    .label(if busy { "认证中" } else { "连接" })
                    .loading(busy)
                    .disabled(!can_submit)
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
            )
    }

    /// 与当前状态相关的动作：退出登录。
    fn render_actions(&self, phase: &Phase, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex().when(phase.can_sign_out(), |this| {
            this.child(
                Button::new("sign-out")
                    .outline()
                    .icon(IconName::LogOut)
                    .label("退出登录")
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.session.update(cx, |session, cx| session.sign_out(cx));
                    })),
            )
        })
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.read(cx);
        let settings = session.settings();
        let (remember, auto_login, auto_close) = (
            settings.remember_password,
            settings.auto_login,
            settings.auto_close,
        );
        let autostart = session.is_autostart();
        let phase = session.phase().clone();
        let muted = cx.theme().muted_foreground;

        let switch_row = |title: &'static str, hint: &'static str, switch: Switch| {
            h_flex()
                .justify_between()
                .gap_4()
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(div().text_sm().child(title))
                        .child(div().text_xs().text_color(muted).child(hint)),
                )
                .child(switch)
        };
        let field = |label: &'static str, input: Input| {
            v_flex()
                .gap_1p5()
                .child(div().text_xs().text_color(muted).child(label))
                .child(input)
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .gap_5()
            .child(switch_row(
                "开机自启动",
                "登录 Windows 后自动认证",
                Switch::new("autostart")
                    .checked(autostart)
                    .on_change(cx.listener(|this, enabled, _, cx| {
                        this.session
                            .update(cx, |session, cx| session.set_autostart(*enabled, cx));
                    })),
            ))
            .child(switch_row(
                "记住登录信息",
                "把用户名和密码保存在本机",
                Switch::new("remember")
                    .checked(remember)
                    .on_change(cx.listener(|this, enabled, _, cx| {
                        this.session.update(cx, |session, cx| {
                            session.set_remember_password(*enabled, cx);
                        });
                    })),
            ))
            .child(switch_row(
                "自动登录",
                "需要认证时用已保存的密码登录",
                Switch::new("auto-login")
                    .checked(auto_login)
                    .on_change(cx.listener(|this, enabled, _, cx| {
                        this.session
                            .update(cx, |session, cx| session.set_auto_login(*enabled, cx));
                    })),
            ))
            .child(switch_row(
                "联网后自动关闭",
                "网络在线 5 秒后退出程序",
                Switch::new("auto-close")
                    .checked(auto_close)
                    .on_change(cx.listener(|this, enabled, _, cx| {
                        this.session
                            .update(cx, |session, cx| session.set_auto_close(*enabled, cx));
                    })),
            ))
            .child(field("认证地址", Input::new(&self.portal_url)))
            .child(field("跳转地址", Input::new(&self.redirect_url)))
            .child(div().flex_1())
            .child(self.render_info_line(self.main_info(&phase, cx), cx))
    }
}
