//! 应用外壳：把面板窗口与会话事件连在一起。
//!
//! 外壳决定何时打开面板、何时退出；认证逻辑全在 [`Session`]。面板窗口
//! 随时可以被关闭，外壳不保存它的句柄，需要时向 GPUI 询问当前的窗口。

use std::time::Duration;

use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, Pixels, Size, Subscription, Task,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, point, px, size,
};

use crate::panel::PanelView;
use crate::session::{Session, SessionEvent};

pub const WINDOW_SIZE: Size<Pixels> = size(px(340.), px(460.));
/// 面板与屏幕可用区域边缘的间距。
const WINDOW_MARGIN: Pixels = px(16.);

/// 让外壳随应用存活：实体没有强引用就会被释放。
struct ShellHandle(#[allow(dead_code)] Entity<Shell>);

impl Global for ShellHandle {}

pub struct Shell {
    session: Entity<Session>,
    /// 延时退出；替换即取消。
    quit_timer: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    /// 创建外壳并让它常驻到应用结束。
    pub fn install(cx: &mut App) -> Entity<Self> {
        let shell = cx.new(Self::new);
        cx.set_global(ShellHandle(shell.clone()));
        shell
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let session = cx.new(Session::new);
        let subscriptions = vec![
            cx.subscribe(&session, |shell, _, event, cx| {
                shell.handle_session_event(event, cx);
            }),
            // 没有托盘：面板就是应用本身，窗口关了就退出。
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            }),
        ];
        Self {
            session,
            quit_timer: None,
            _subscriptions: subscriptions,
        }
    }

    /// 打开面板；已经打开则把它带到前台。
    pub fn open_panel(&mut self, cx: &mut Context<Self>) {
        if let Some(window) = cx.windows().into_iter().next() {
            window
                .update(cx, |_, window, _| window.activate_window())
                .ok();
            return;
        }
        let session = self.session.clone();
        let options = panel_window_options(cx);
        if let Err(err) = gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| PanelView::new(session, window, cx))
        }) {
            eprintln!("无法打开面板窗口: {err:#}");
            cx.quit();
        }
    }

    fn handle_session_event(&mut self, event: &SessionEvent, cx: &mut Context<Self>) {
        match event {
            SessionEvent::Finished { after } => self.quit_after(*after, cx),
            SessionEvent::Resumed => self.quit_timer = None,
        }
    }

    fn quit_after(&mut self, after: Duration, cx: &mut Context<Self>) {
        self.quit_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(after).await;
            this.update(cx, |_, cx| cx.quit()).ok();
        }));
    }
}

/// 面板窗口：无边框，贴近屏幕可用区域的右下角。
fn panel_window_options(cx: &App) -> WindowOptions {
    let bounds = cx
        .primary_display()
        .map(|display| {
            let area = display.visible_bounds();
            let origin = point(
                area.right() - WINDOW_SIZE.width - WINDOW_MARGIN,
                area.bottom() - WINDOW_SIZE.height - WINDOW_MARGIN,
            );
            WindowBounds::Windowed(gpui_kit::Bounds::new(origin, WINDOW_SIZE))
        })
        .unwrap_or_else(|| WindowBounds::centered(WINDOW_SIZE, cx));

    WindowOptions {
        window_bounds: Some(bounds),
        // 没有系统标题栏；关闭、设置按钮与拖动区域由面板自己绘制。
        titlebar: None,
        // 弹窗：置顶，不会被其它窗口盖住。
        kind: WindowKind::PopUp,
        // Windows 11 亚克力模糊；Windows 10 自动回退为普通窗口。
        window_background: WindowBackgroundAppearance::Blurred,
        is_resizable: false,
        is_minimizable: false,
        ..WindowOptions::default()
    }
}
