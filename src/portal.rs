//! NetentSec 网页认证网关（门户）客户端。
//!
//! 门户地址与认证后的跳转地址都可在设置里改，默认门户为 `http://172.16.0.5:90`。
//! “是否已认证”直接询问门户：已认证时 `GET /login` 返回一段跳转到跳转地址的脚本，
//! 因此不必 ping 外网。

use std::io::Read as _;
use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{Context as _, Result};

use crate::config::{Credentials, Settings};
use crate::portal_crypto;

/// 公司网络的标志：在线网卡的默认网关是它。
pub const COMPANY_GATEWAY: Ipv4Addr = Ipv4Addr::new(172, 16, 15, 254);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
/// 门户页面都很小，限制读取量以免异常响应占满内存。
const MAX_BODY_BYTES: u64 = 1024 * 1024;

const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// 门户对当前会话的回答。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnlineState {
    /// 门户返回“跳转地址”的脚本：本机已认证。
    Authenticated,
    /// 门户返回登录表单。
    NeedsLogin,
    /// 门户没有给出可用响应。
    Unreachable,
}

/// 一次登录提交的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    Accepted,
    /// 门户拒绝，附带门户给出的原因。
    Rejected(String),
}

/// 门户响应的关键部分。
struct PortalResponse {
    status: u16,
    location: Option<String>,
    body: String,
}

/// 门户 HTTP 客户端。
pub struct PortalClient {
    agent: ureq::Agent,
    login_url: String,
    /// 门户的主机部分，用来识别“跳回门户”的 302。
    portal_host: String,
    /// 跳转地址的域名（去掉 `www.`）：已认证页面里必然出现。
    site_marker: String,
}

impl PortalClient {
    pub fn new(settings: &Settings) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(REQUEST_TIMEOUT)
            // 登录成功的信号就是 302，必须自己判断而不是跟随。
            .redirects(0)
            .build();
        let base = settings.portal_url.trim().trim_end_matches('/');
        Self {
            agent,
            login_url: format!("{base}/login"),
            portal_host: url_host(base).to_string(),
            site_marker: url_host(settings.redirect_url.trim())
                .trim_start_matches("www.")
                .to_string(),
        }
    }

    /// 询问门户本机是否已认证。偶发的超时会重试一次，免得误报“门户无法访问”。
    pub fn online_state(&self) -> OnlineState {
        let mut state = self.query_online_state();
        if state == OnlineState::Unreachable {
            state = self.query_online_state();
        }
        state
    }

    fn query_online_state(&self) -> OnlineState {
        let request = self
            .agent
            .get(&self.login_url)
            .set("User-Agent", BROWSER_USER_AGENT);
        match read_response(request.call()) {
            Ok(response) => classify_login_page(&response.body, &self.site_marker),
            Err(err) => {
                eprintln!("查询门户失败: {err:#}");
                OnlineState::Unreachable
            }
        }
    }

    /// 请求退出登录（`POST /login`，`login_type=logout`），并向门户确认本机
    /// 已不再是已认证状态。
    pub fn logout(&self) -> Result<()> {
        let request = self
            .agent
            .post(&self.login_url)
            .set("User-Agent", BROWSER_USER_AGENT)
            .set("Content-Type", "application/x-www-form-urlencoded");
        read_response(request.send_string("login_type=logout"))?;
        if self.online_state() == OnlineState::Authenticated {
            anyhow::bail!("门户没有接受退出请求");
        }
        Ok(())
    }

    /// 提交登录表单。传输失败返回 `Err`，门户拒绝返回 `Rejected`。
    pub fn login(&self, credentials: &Credentials) -> Result<LoginOutcome> {
        let form = login_form(credentials);
        let request = self
            .agent
            .post(&self.login_url)
            .set("User-Agent", BROWSER_USER_AGENT)
            .set("Content-Type", "application/x-www-form-urlencoded");
        let response = read_response(request.send_string(&form))?;

        // 成功时门户 302 到跳转地址。
        if (300..400).contains(&response.status)
            && response
                .location
                .as_deref()
                .is_some_and(|url| !url.contains(&self.portal_host))
        {
            return Ok(LoginOutcome::Accepted);
        }
        // 失败页把原因写在 layer.open 的 content 里。
        if let Some(message) = extract_layer_message(&response.body) {
            return Ok(LoginOutcome::Rejected(message));
        }
        // 其余情况以页面内容为准，而不是猜测。
        Ok(
            match classify_login_page(&response.body, &self.site_marker) {
                OnlineState::Authenticated => LoginOutcome::Accepted,
                OnlineState::NeedsLogin | OnlineState::Unreachable => {
                    LoginOutcome::Rejected("门户未接受该用户名或密码".to_string())
                }
            },
        )
    }
}

/// 登录请求体：用户名、`md6()` 编码后的密码、`login_type`。
fn login_form(credentials: &Credentials) -> String {
    format!(
        "username={}&password={}&login_type=login",
        form_encode(credentials.username.as_bytes()),
        portal_crypto::encrypt_password(&credentials.password),
    )
}

/// 取出 URL 的主机（含端口）部分：`http://a.b:90/x` → `a.b:90`。
fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

/// 读取响应。4xx/5xx 仍是门户的有效回答；只有传输层失败才算错误。
/// 正文按有损 UTF-8 解码，门户若返回其它编码也不会让请求失败。
fn read_response(result: Result<ureq::Response, ureq::Error>) -> Result<PortalResponse> {
    let response = match result {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(err) => return Err(err).context("无法连接门户"),
    };
    let status = response.status();
    let location = response.header("Location").map(str::to_owned);
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_BODY_BYTES)
        .read_to_end(&mut bytes)
        .context("读取门户响应失败")?;
    Ok(PortalResponse {
        status,
        location,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// 判断 `/login` 响应属于哪种状态。门户有回应就算可达：登录表单、跳转到
/// 登录页的空响应或无法识别的页面，都按需要登录处理。
fn classify_login_page(body: &str, site_marker: &str) -> OnlineState {
    if !site_marker.is_empty() && body.contains(site_marker) && body.contains("document.location") {
        OnlineState::Authenticated
    } else {
        OnlineState::NeedsLogin
    }
}

/// 取出门户错误页 `content: "…"` 中的提示文案。
fn extract_layer_message(body: &str) -> Option<String> {
    let rest = body[body.find("content:")? + "content:".len()..].trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let inner = &rest[quote.len_utf8()..];
    let message = &inner[..inner.find(quote)?];
    (!message.is_empty()).then(|| message.to_string())
}

/// 与 JavaScript 的 `encodeURIComponent` 一致，按原始字节做百分号编码。
fn form_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(bytes.len() * 3);
    for &byte in bytes {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARKER: &str = "smartwe.com.cn";

    #[test]
    fn login_form_has_only_the_three_fields() {
        let credentials = Credentials {
            username: "shenyefan".into(),
            password: "1".into(),
        };
        // md6("1") 为 "%B9"，原样放进请求体，不再编码。
        assert_eq!(
            login_form(&credentials),
            "username=shenyefan&password=%B9&login_type=login"
        );
    }

    #[test]
    fn extracts_message_from_real_failure_page() {
        let page = r#"<meta http-equiv="Content-Type" content="text/html; charset=utf-8" />
        <script>layer.open({ title: '提示', closeBtn: 0, btn: '确定',
            skin: 'layui-layer-rim', content: "用户密码错误",
            yes: function (index, layero) {} });</script>"#;
        assert_eq!(extract_layer_message(page).as_deref(), Some("用户密码错误"));
    }

    #[test]
    fn form_encode_escapes_non_ascii_username() {
        assert_eq!(form_encode("张三".as_bytes()), "%E5%BC%A0%E4%B8%89");
    }

    #[test]
    fn classifies_authenticated_page() {
        let body = "<script>document.location.href='http://www.smartwe.com.cn/';</script>";
        assert_eq!(
            classify_login_page(body, MARKER),
            OnlineState::Authenticated
        );
    }

    #[test]
    fn classifies_login_form_and_empty_redirect_page() {
        let form = r#"<form><input name="password"><input name="login_type"></form>"#;
        assert_eq!(classify_login_page(form, MARKER), OnlineState::NeedsLogin);
        // 未认证时门户可能只回一个空的 302：仍是“可达，需要登录”。
        assert_eq!(classify_login_page("  \n", MARKER), OnlineState::NeedsLogin);
    }

    #[test]
    fn empty_marker_never_matches() {
        let body = "<script>document.location.href='http://x/';</script>";
        assert_eq!(classify_login_page(body, ""), OnlineState::NeedsLogin);
    }

    #[test]
    fn extracts_url_host() {
        assert_eq!(url_host("http://172.16.0.5:90"), "172.16.0.5:90");
        assert_eq!(
            url_host("http://www.smartwe.com.cn/a?b"),
            "www.smartwe.com.cn"
        );
        assert_eq!(url_host("example.com/x"), "example.com");
    }

    #[test]
    fn client_derives_endpoints_from_settings() {
        let settings = Settings {
            portal_url: " http://10.0.0.1:8080/ ".into(),
            redirect_url: "https://www.example.com/".into(),
            ..Settings::default()
        };
        let client = PortalClient::new(&settings);
        assert_eq!(client.login_url, "http://10.0.0.1:8080/login");
        assert_eq!(client.portal_host, "10.0.0.1:8080");
        assert_eq!(client.site_marker, "example.com");
    }

    #[test]
    fn extracts_layer_message() {
        let body = r#"layer.open({ content: "用户不存在或被禁用", btn: 1 })"#;
        assert_eq!(
            extract_layer_message(body).as_deref(),
            Some("用户不存在或被禁用")
        );
    }

    #[test]
    fn extract_layer_message_rejects_unquoted_or_empty_content() {
        assert_eq!(extract_layer_message("content: 42"), None);
        assert_eq!(extract_layer_message(r#"content: """#), None);
        assert_eq!(extract_layer_message("no message"), None);
    }
}
