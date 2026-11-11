//! Display connection URLs from local interfaces without external IP discovery.
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
            && interface.ifa_flags & libc::IFF_LOOPBACK as u32 == 0
            && unsafe { (*interface.ifa_addr).sa_family as i32 } == libc::AF_INET
        {
            let address = unsafe { &*(interface.ifa_addr as *const libc::sockaddr_in) };
            let address = Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes());
            if !address.is_loopback() && !address.is_unspecified() && !address.is_multicast() {
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
    if config.server.bind.ip().is_unspecified()
        || config.web.enabled && config.web.bind.ip().is_unspecified()
    {
        eprintln!("\nLocal network access:");
        let mut found = false;
        for (name, address) in interfaces().unwrap_or_default() {
            let backend =
                config.server.bind.ip().is_ipv4() && config.server.bind.ip().is_unspecified();
            let web = config.web.enabled
                && config.web.bind.ip().is_ipv4()
                && config.web.bind.ip().is_unspecified();
            if !backend && !web {
                continue;
            }
            found = true;
            eprintln!("  Interface {name}:");
            if backend {
                eprintln!(
                    "    Backend: http://{address}:{}",
                    config.server.bind.port()
                );
            }
            if web {
                eprintln!("    Web:     http://{address}:{}", config.web.bind.port());
            }
        }
        if !found {
            eprintln!("  No active IPv4 interface found. Check the local network connection.");
        }
    }
    if config.web.enabled {
        eprintln!(
            "\nOpen the Web address and sign in with your Web token.\nView token: kyler service token"
        );
    }
}
