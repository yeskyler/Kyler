//! Display one primary network connection using host routes, without external IP discovery.
use std::{collections::BTreeSet, net::Ipv4Addr};

use kyler_config::AppConfig;

#[cfg(unix)]
fn interfaces() -> std::io::Result<BTreeSet<(String, Ipv4Addr)>> {
    use std::{ffi::CStr, ptr};
    struct Addresses(*mut libc::ifaddrs);
    impl Drop for Addresses {
        fn drop(&mut self) {
            unsafe { libc::freeifaddrs(self.0) };
        }
    }
    let mut head = ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let list = Addresses(head);
    let mut current = list.0;
    let mut result = BTreeSet::new();
    while !current.is_null() {
        // getifaddrs owns this list until the guard releases it.
        let interface = unsafe { &*current };
        if !interface.ifa_addr.is_null()
            && !interface.ifa_name.is_null()
            && interface.ifa_flags & libc::IFF_UP as u32 != 0
            && interface.ifa_flags & libc::IFF_RUNNING as u32 != 0
            && interface.ifa_flags & (libc::IFF_LOOPBACK | libc::IFF_POINTOPOINT) as u32 == 0
            && unsafe { (*interface.ifa_addr).sa_family as i32 } == libc::AF_INET
        {
            let address = unsafe { &*(interface.ifa_addr as *const libc::sockaddr_in) };
            let address = Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes());
            if usable_address(address) {
                let name = unsafe { CStr::from_ptr(interface.ifa_name) }
                    .to_string_lossy()
                    .into_owned();
                result.insert((name, address));
            }
        }
        current = interface.ifa_next;
    }
    Ok(result)
}
#[cfg(not(unix))]
fn interfaces() -> std::io::Result<BTreeSet<(String, Ipv4Addr)>> {
    Ok(BTreeSet::new())
}

#[derive(Debug, PartialEq, Eq)]
struct DefaultRoute {
    interface: String,
    priority: u32,
}

fn usable_address(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !matches!(a, 0 | 127 | 224..=255)
        && !(a == 169 && b == 254)
        && !(a == 198 && matches!(b, 18 | 19))
        && !matches!((a, b, c), (192, 0, 0 | 2) | (198, 51, 100) | (203, 0, 113))
}

fn usable_interface(name: &str) -> bool {
    // Exclude known tunnel/container interfaces, while allowing host bridges, bonds and VLANs.
    ![
        "lo",
        "utun",
        "tun",
        "tap",
        "wg",
        "ppp",
        "ipsec",
        "tailscale",
        "zt",
        "docker",
        "veth",
        "virbr",
        "br-",
        "cni",
        "flannel",
        "kube",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

#[cfg(any(target_os = "linux", test))]
fn linux_routes(table: &str) -> Vec<DefaultRoute> {
    table
        .lines()
        .filter_map(|line| {
            let columns: Vec<_> = line.split_whitespace().collect();
            if columns.len() < 8 || columns[1] != "00000000" || columns[7] != "00000000" {
                return None;
            }
            let flags = u32::from_str_radix(columns[3], 16).ok()?;
            if flags & 1 == 0 || flags & 0x200 != 0 {
                return None;
            }
            Some(DefaultRoute {
                interface: columns[0].into(),
                priority: columns[6].parse().ok()?,
            })
        })
        .collect()
}

#[cfg(any(target_os = "macos", test))]
fn macos_routes(table: &str) -> Vec<DefaultRoute> {
    table
        .lines()
        .filter_map(|line| {
            let columns: Vec<_> = line.split_whitespace().collect();
            if columns.len() < 4 || columns[0] != "default" {
                return None;
            }
            let flags = columns[2];
            if !flags.contains('U') || flags.contains(['R', 'B']) {
                return None;
            }
            // Unscoped defaults take precedence; scoped physical defaults remain usable beneath a VPN.
            Some(DefaultRoute {
                interface: columns[3].into(),
                priority: u32::from(flags.contains('I')),
            })
        })
        .collect()
}

fn default_routes() -> std::io::Result<Vec<DefaultRoute>> {
    #[cfg(target_os = "linux")]
    {
        Ok(linux_routes(&std::fs::read_to_string("/proc/net/route")?))
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/sbin/netstat")
            .args(["-rn", "-f", "inet"])
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other("Cannot read IPv4 routing table"));
        }
        Ok(macos_routes(&String::from_utf8_lossy(&output.stdout)))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(Vec::new())
    }
}

fn select_primary(
    interfaces: &BTreeSet<(String, Ipv4Addr)>,
    routes: &[DefaultRoute],
) -> Option<(String, Ipv4Addr)> {
    let candidates: Vec<_> = routes
        .iter()
        .filter(|route| {
            usable_interface(&route.interface)
                && interfaces
                    .iter()
                    .any(|(name, address)| name == &route.interface && usable_address(*address))
        })
        .collect();
    let priority = candidates.iter().map(|route| route.priority).min()?;
    let names: BTreeSet<_> = candidates
        .iter()
        .filter(|route| route.priority == priority)
        .map(|route| &route.interface)
        .collect();
    // Neither equal-priority links nor multiple addresses should be resolved alphabetically.
    if names.len() != 1 {
        return None;
    }
    let name = *names.first()?;
    let addresses: Vec<_> = interfaces
        .iter()
        .filter(|(interface, address)| interface == name && usable_address(*address))
        .collect();
    if addresses.len() != 1 {
        return None;
    }
    Some(addresses[0].clone())
}

pub(super) fn print(config: &AppConfig) {
    eprintln!(
        "\nLocal access:\n  Backend: {}",
        kyler_config::web::connect_url(config.server.bind)
    );
    if config.web.enabled {
        eprintln!(
            "  Web:     {}",
            kyler_config::web::connect_url(config.web.bind)
        );
    }
    let backend = config.server.bind.ip().is_ipv4() && config.server.bind.ip().is_unspecified();
    let web = config.web.enabled
        && config.web.bind.ip().is_ipv4()
        && config.web.bind.ip().is_unspecified();
    if backend || web {
        eprintln!("\nNetwork access:");
        let primary = interfaces()
            .ok()
            .zip(default_routes().ok())
            .and_then(|(interfaces, routes)| select_primary(&interfaces, &routes));
        if let Some((name, address)) = primary {
            eprintln!("  Interface: {name}");
            if backend {
                eprintln!("  Backend: http://{address}:{}", config.server.bind.port());
            }
            if web {
                eprintln!("  Web:     http://{address}:{}", config.web.bind.port());
            }
        } else {
            eprintln!(
                "  Cannot reliably identify the primary IPv4 network connection. Check the host routing configuration."
            );
        }
    }
    if config.web.enabled {
        eprintln!(
            "\nOpen the Web address and sign in with your Web token.\nView token: kyler service token"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(values: &[(&str, &str)]) -> BTreeSet<(String, Ipv4Addr)> {
        values
            .iter()
            .map(|(name, ip)| ((*name).into(), ip.parse().unwrap()))
            .collect()
    }

    #[test]
    fn macos_uses_underlying_connection_when_vpn_owns_default() {
        let interfaces = links(&[("en0", "192.168.1.8"), ("utun4", "240.0.0.1")]);
        let routes = macos_routes(
            "Destination Gateway Flags Netif Expire\ndefault link#25 UCSg utun4\ndefault 192.168.1.1 UGScIg en0\n192.168.1 link#11 UCS en0\n",
        );
        assert_eq!(
            select_primary(&interfaces, &routes),
            Some(("en0".into(), "192.168.1.8".parse().unwrap()))
        );
    }

    #[test]
    fn macos_prefers_unscoped_physical_default() {
        let interfaces = links(&[("en0", "192.168.1.8"), ("en1", "10.0.0.2")]);
        let routes = macos_routes(
            "default 192.168.1.1 UGScIg en0\ndefault 10.0.0.1 UGScg en1\ndefault 10.1.1.1 UGR en2\n",
        );
        assert_eq!(select_primary(&interfaces, &routes).unwrap().0, "en1");
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn linux_uses_metric_and_ignores_vpn_container_and_nondefault_routes() {
        let interfaces = links(&[
            ("eth0", "10.0.0.2"),
            ("wlan0", "192.168.1.8"),
            ("wg0", "10.20.0.2"),
            ("docker0", "172.17.0.1"),
        ]);
        let routes = linux_routes(
            "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\neth0 00000000 0100000A 0003 0 0 100 00000000 0 0 0\nwlan0 00000000 0101A8C0 0003 0 0 600 00000000 0 0 0\nwg0 00000000 00000000 0001 0 0 0 00000000 0 0 0\ndocker0 00000000 00000000 0001 0 0 0 00000000 0 0 0\nwlan0 0001A8C0 00000000 0001 0 0 0 00FFFFFF 0 0 0\n",
        );
        assert_eq!(select_primary(&interfaces, &routes).unwrap().0, "eth0");
    }

    #[test]
    fn unavailable_or_ambiguous_routes_do_not_pick_an_arbitrary_link() {
        let interfaces = links(&[("en0", "192.168.1.8"), ("en1", "10.0.0.2")]);
        assert!(select_primary(&interfaces, &[]).is_none());
        assert!(select_primary(&interfaces, &macos_routes("default link#1 U en2")).is_none());
        assert!(
            select_primary(
                &interfaces,
                &macos_routes("default link#1 U en0\ndefault link#2 U en1")
            )
            .is_none()
        );
        assert!(
            select_primary(
                &links(&[("en0", "192.168.1.8"), ("en0", "192.168.1.9")]),
                &macos_routes("default link#1 U en0")
            )
            .is_none()
        );
        assert_eq!(
            select_primary(
                &interfaces,
                &macos_routes("default link#1 U en0\ndefault link#1 U en0")
            )
            .unwrap()
            .0,
            "en0"
        );
    }

    #[test]
    fn linux_rejects_invalid_down_or_rejected_defaults() {
        assert!(linux_routes("eth0 00000000 00000000 0201 0 0 0 00000000\neth0 00000000 00000000 0000 0 0 0 00000000\neth0 00000000 00000000 0001 0 0 invalid 00000000\ninvalid\n").is_empty());
    }

    #[test]
    fn addresses_allow_private_public_and_cloud_ranges_but_not_reserved_ranges() {
        for address in [
            "192.168.1.8",
            "10.0.0.2",
            "172.16.1.1",
            "100.64.1.1",
            "8.8.8.8",
        ] {
            assert!(usable_address(address.parse().unwrap()), "{address}");
        }
        for address in [
            "0.0.0.0",
            "0.1.2.3",
            "127.0.0.1",
            "169.254.1.2",
            "198.18.0.1",
            "192.0.2.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(!usable_address(address.parse().unwrap()), "{address}");
        }
    }

    #[test]
    fn physical_bridge_bond_and_vlan_names_are_not_confused_with_tunnels() {
        for name in ["en0", "ens3", "eth0", "wlan0", "bond0", "br0", "eth0.10"] {
            assert!(usable_interface(name), "{name}");
        }
        for name in [
            "utun4",
            "tun0",
            "tap0",
            "wg0",
            "tailscale0",
            "docker0",
            "veth123",
            "br-abc",
            "cni0",
            "virbr0",
        ] {
            assert!(!usable_interface(name), "{name}");
        }
    }
}
