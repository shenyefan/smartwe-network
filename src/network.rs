//! 网络信息与公司网络判定。
//!
//! 公司网络以“某块在线网卡的默认网关是 172.16.15.254”判定，直接读网卡
//! 配置，不依赖 ICMP：部分机器上的安全软件会拒绝非白名单程序发送
//! ICMP（`IcmpSendEcho` 返回“拒绝访问”），那样 ping 永远判为失败。

use std::net::{IpAddr, Ipv4Addr};

use anyhow::Result;

use crate::portal::{COMPANY_GATEWAY, OnlineState, PortalClient};

/// 状态面板展示的活动网卡信息。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkInfo {
    /// 网卡友好名，如“以太网”“WLAN”。
    pub adapter: String,
    pub ipv4: Option<Ipv4Addr>,
    pub gateway: Option<Ipv4Addr>,
}

impl NetworkInfo {
    /// 网关是否为公司网关。
    pub fn is_company_network(&self) -> bool {
        self.gateway == Some(COMPANY_GATEWAY)
    }
}

/// 一次完整网络检测的结果。
#[derive(Debug, Clone)]
pub struct ProbeResult {
    /// 是否处于公司网络。
    pub on_company_network: bool,
    /// 门户的回答；非公司网络时不会询问门户。
    pub portal: OnlineState,
    pub network: NetworkInfo,
}

/// 阻塞地完成一次检测，应在后台线程调用。
pub fn probe(portal: &PortalClient) -> ProbeResult {
    let network = collect().unwrap_or_else(|err| {
        eprintln!("读取网卡信息失败: {err:#}");
        NetworkInfo::default()
    });
    let on_company_network = network.is_company_network();
    let portal_state = if on_company_network {
        portal.online_state()
    } else {
        OnlineState::Unreachable
    };
    ProbeResult {
        on_company_network,
        portal: portal_state,
        network,
    }
}

/// 读取在线、带 IPv4 地址的网卡，并挑出要展示的那一块。
fn collect() -> Result<NetworkInfo> {
    let adapters = ipconfig::get_adapters()?
        .iter()
        .filter(|adapter| adapter.oper_status() == ipconfig::OperStatus::IfOperStatusUp)
        .map(|adapter| NetworkInfo {
            adapter: adapter.friendly_name().to_string(),
            ipv4: first_ipv4(adapter.ip_addresses()),
            gateway: first_ipv4(adapter.gateways()),
        })
        .filter(|info| info.ipv4.is_some())
        .collect();
    Ok(choose_adapter(adapters))
}

fn first_ipv4(ips: &[IpAddr]) -> Option<Ipv4Addr> {
    ips.iter().find_map(|ip| match ip {
        IpAddr::V4(v4) => Some(*v4),
        IpAddr::V6(_) => None,
    })
}

/// 优先公司网关所在的网卡，其次任何带网关（承载默认路由）的，最后第一块。
fn choose_adapter(adapters: Vec<NetworkInfo>) -> NetworkInfo {
    let rank = |info: &NetworkInfo| match info.gateway {
        Some(COMPANY_GATEWAY) => 0,
        Some(_) => 1,
        None => 2,
    };
    adapters.into_iter().min_by_key(rank).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(name: &str, gateway: Option<Ipv4Addr>) -> NetworkInfo {
        NetworkInfo {
            adapter: name.to_string(),
            ipv4: Some(Ipv4Addr::new(10, 0, 0, 2)),
            gateway,
        }
    }

    #[test]
    fn prefers_the_adapter_on_the_company_gateway() {
        let chosen = choose_adapter(vec![
            adapter("虚拟网卡", None),
            adapter("家庭路由", Some(Ipv4Addr::new(192, 168, 1, 1))),
            adapter("WLAN", Some(COMPANY_GATEWAY)),
        ]);
        assert_eq!(chosen.adapter, "WLAN");
        assert!(chosen.is_company_network());
    }

    #[test]
    fn falls_back_to_adapter_with_a_gateway_then_to_the_first() {
        let with_gateway = choose_adapter(vec![
            adapter("虚拟网卡", None),
            adapter("以太网", Some(Ipv4Addr::new(192, 168, 1, 1))),
        ]);
        assert_eq!(with_gateway.adapter, "以太网");
        assert!(!with_gateway.is_company_network());

        let first = choose_adapter(vec![adapter("甲", None), adapter("乙", None)]);
        assert_eq!(first.adapter, "甲");
    }

    #[test]
    fn no_adapters_yields_empty_info() {
        let chosen = choose_adapter(Vec::new());
        assert!(chosen.adapter.is_empty());
        assert!(!chosen.is_company_network());
    }
}
