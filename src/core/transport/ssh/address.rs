//! SSH 目标的浏览器地址解析；只在 Core 后台执行，前端不解析 SSH 配置。

use std::net::{IpAddr, ToSocketAddrs};
use std::path::Path;
use std::process::Command;

use crate::transport::{TransportError, TransportResult};

struct EffectiveSshAddress {
    host: String,
    family: String,
}

impl EffectiveSshAddress {
    fn parse(output: &[u8]) -> TransportResult<Self> {
        let output = std::str::from_utf8(output)
            .map_err(|_| TransportError::message("SSH configuration is not UTF-8"))?;
        let mut host = None;
        let mut family = "any";
        for line in output.lines() {
            let mut fields = line.split_whitespace();
            match fields.next() {
                Some("hostname") => {
                    host = fields.next();
                    if fields.next().is_some() {
                        return Err(TransportError::message("invalid SSH hostname"));
                    }
                }
                Some("addressfamily") => family = fields.next().unwrap_or("any"),
                _ => {}
            }
        }
        let host = host
            .filter(|host| !host.is_empty())
            .ok_or_else(|| TransportError::message("SSH configuration has no hostname"))?;
        let host = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host);
        let host = if let Ok(ip) = host.parse::<IpAddr>() {
            ip.to_string()
        } else if host
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b".-_".contains(&ch))
        {
            host.to_string()
        } else {
            return Err(TransportError::message("invalid SSH hostname"));
        };
        if !matches!(family, "any" | "inet" | "inet6") {
            return Err(TransportError::message("invalid SSH address family"));
        }
        Ok(Self {
            host,
            family: family.to_string(),
        })
    }

    fn browser_host(&self, resolved: &[IpAddr]) -> String {
        if let Ok(ip) = self.host.parse::<IpAddr>() {
            return ip.to_string();
        }
        resolved
            .iter()
            .find(|ip| match self.family.as_str() {
                "inet" => ip.is_ipv4(),
                "inet6" => ip.is_ipv6(),
                _ => true,
            })
            .map(ToString::to_string)
            .unwrap_or_else(|| self.host.clone())
    }
}

pub(crate) fn resolve_browser_host(alias: &str, config: Option<&Path>) -> TransportResult<String> {
    if alias.trim().is_empty() {
        return Err(TransportError::message("SSH target alias is empty"));
    }
    // 由系统 OpenSSH 处理 Include / Match / HostName；不要另写一套连接语义。
    let mut command = Command::new("ssh");
    command.arg("-G");
    if let Some(config) = config {
        command.arg("-F").arg(config);
    }
    let output = command.arg("--").arg(alias).output()?;
    if !output.status.success() {
        return Err(TransportError::message(format!(
            "could not resolve SSH target address: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        )));
    }
    let address = EffectiveSshAddress::parse(&output.stdout)?;
    // DNS 可能阻塞；本入口只在端口扫描后台线程调用。不能解析时保留真实 HostName。
    let resolved: Vec<_> = if address.host.parse::<IpAddr>().is_ok() {
        Vec::new()
    } else {
        (address.host.as_str(), 0)
            .to_socket_addrs()
            .map(|addresses| addresses.map(|address| address.ip()).collect())
            .unwrap_or_default()
    };
    Ok(address.browser_host(&resolved))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_browser_host_uses_effective_hostname_instead_of_alias_user_or_ssh_port() {
        let host = EffectiveSshAddress::parse(
            b"host ryzen\nhostname 192.0.2.42\nuser alice\nport 2222\naddressfamily any\n",
        )
        .unwrap();
        assert_eq!(host.browser_host(&[]), "192.0.2.42");
    }

    #[test]
    fn ssh_browser_host_preserves_ipv6_without_url_brackets() {
        for address in ["2001:db8::42", "[2001:db8::42]"] {
            let host = EffectiveSshAddress::parse(
                format!("hostname {address}\naddressfamily inet6\n").as_bytes(),
            )
            .unwrap();
            assert_eq!(host.browser_host(&[]), "2001:db8::42");
        }
    }

    #[test]
    fn ssh_browser_host_resolves_dns_using_configured_address_family() {
        let addresses = [
            "2001:db8::42".parse().unwrap(),
            "192.0.2.42".parse().unwrap(),
        ];
        for (family, expected) in [
            ("any", "2001:db8::42"),
            ("inet", "192.0.2.42"),
            ("inet6", "2001:db8::42"),
        ] {
            let host = EffectiveSshAddress::parse(
                format!("hostname machine.example\naddressfamily {family}\n").as_bytes(),
            )
            .unwrap();
            assert_eq!(host.browser_host(&addresses), expected);
        }
    }

    #[test]
    fn ssh_browser_host_keeps_effective_hostname_if_local_dns_is_unavailable() {
        let host = EffectiveSshAddress::parse(b"hostname jump-target.example\n").unwrap();
        assert_eq!(host.browser_host(&[]), "jump-target.example");
    }

    #[test]
    fn ssh_browser_host_rejects_missing_and_malformed_hosts() {
        for output in [
            "host alias\n",
            "hostname \n",
            "hostname user@machine\n",
            "hostname machine/path\n",
            "hostname machine:8080\n",
            "hostname machine name\n",
            "hostname https://machine\n",
        ] {
            assert!(
                EffectiveSshAddress::parse(output.as_bytes()).is_err(),
                "{output}"
            );
        }
    }

    #[test]
    fn ssh_browser_host_honors_system_ssh_include_and_match_configuration() {
        let directory = std::env::temp_dir().join(format!(
            "muxterm-test-ssh-browser-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir(&directory).unwrap();
        let config = directory.join("config");
        let include = directory.join("included");
        std::fs::write(
            &include,
            "Host port-test\n  HostName 192.0.2.47\n  Port 2222\n  User example\n",
        )
        .unwrap();
        std::fs::write(
            &config,
            format!(
                "Include {}\nMatch originalhost port-test\n  AddressFamily inet\n",
                include.display()
            ),
        )
        .unwrap();
        let result = resolve_browser_host("port-test", Some(&config));
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(result.unwrap(), "192.0.2.47");
    }
}
