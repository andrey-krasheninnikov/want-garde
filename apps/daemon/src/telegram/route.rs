use super::TransportError;
use garde_config::{TelegramRoute, read_private};
use percent_encoding::percent_decode_str;
use reqwest::{Client, ClientBuilder, Proxy, Url, redirect::Policy, retry};
use std::{error::Error, time::Duration};

#[cfg(target_os = "linux")]
mod dns;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Direct,
    Vless,
    Wireguard,
    Tailscale,
}

pub(super) struct Route {
    pub(super) client: Client,
    pub(super) kind: Kind,
}

impl Route {
    pub(super) fn new(
        route: &TelegramRoute,
        configure: impl FnOnce(ClientBuilder) -> ClientBuilder,
    ) -> Result<Self, TransportError> {
        let builder = builder(true);
        let (kind, builder) = match route {
            TelegramRoute::Direct {} => (Kind::Direct, builder),
            TelegramRoute::Vless {
                proxy_credentials_file,
            } => (
                Kind::Vless,
                builder.proxy(private_proxy(proxy_credentials_file)?),
            ),
            TelegramRoute::Wireguard {
                interface,
                dns_server,
            } => (Kind::Wireguard, tunnel(builder, interface, *dns_server)?),
            TelegramRoute::Tailscale {
                interface,
                dns_server,
            } => (Kind::Tailscale, tunnel(builder, interface, *dns_server)?),
        };
        Ok(Self {
            kind,
            client: configure(builder)
                .build()
                .map_err(|_| TransportError::fatal("telegram.client"))?,
        })
    }

    pub(super) fn connect_failure(&self, error: &reqwest::Error) -> &'static str {
        #[cfg(target_os = "linux")]
        if let Some(failure) =
            sources(error).find_map(|source| source.downcast_ref::<dns::Failure>())
        {
            return match failure {
                dns::Failure::Interface => "telegram.tunnel_interface",
                dns::Failure::Routing => "telegram.tunnel_connect",
                dns::Failure::Lookup => "telegram.dns",
            };
        }
        if error.is_dns() {
            return "telegram.dns";
        }
        if sources(error).any(|source| source.is::<rustls::Error>()) {
            return "telegram.tls";
        }
        match self.kind {
            Kind::Direct => "telegram.network",
            Kind::Vless => "telegram.proxy_connect",
            Kind::Wireguard | Kind::Tailscale => "telegram.tunnel_connect",
        }
    }

    pub(super) fn label(&self) -> &'static str {
        match self.kind {
            Kind::Direct => "direct",
            Kind::Vless => "vless",
            Kind::Wireguard => "wireguard",
            Kind::Tailscale => "tailscale",
        }
    }
}

fn sources(error: &reqwest::Error) -> impl Iterator<Item = &(dyn Error + 'static)> {
    std::iter::successors(error.source(), |&source| source.source())
}

pub(super) fn builder(https_only: bool) -> ClientBuilder {
    Client::builder()
        .https_only(https_only)
        .redirect(Policy::none())
        .no_proxy()
        .retry(retry::never())
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(35))
}

fn private_proxy(path: &std::path::Path) -> Result<Proxy, TransportError> {
    let bytes = read_private(path, 2048)
        .map_err(|_| TransportError::fatal("telegram.proxy_credentials"))?;
    let error = || TransportError::fatal("telegram.proxy_credentials");
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| error())?
        .trim_end_matches(['\r', '\n']);
    if text
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(error());
    }
    let url = Url::parse(text).map_err(|_| error())?;
    // Numeric loopback endpoints keep proxy setup independent of the system resolver.
    let local = url.host_str().is_some_and(|host| {
        host.trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if url.scheme() != "socks5h"
        || !local
        || url.port().is_none_or(|port| port == 0)
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(error());
    }
    // Leave 29 bytes for negotiation, authentication headers, and CONNECT to api.telegram.org
    // in hyper-util 0.1.21's 262-byte SOCKS5 send buffer.
    const MAX_SOCKS5_AUTH_BYTES: usize = 233;
    match (url.username(), url.password()) {
        ("", None) if !text.contains('@') => {}
        (user, Some(password))
            if !user.is_empty()
                && !password.is_empty()
                && percent_decode_str(user).decode_utf8_lossy().len()
                    + percent_decode_str(password).decode_utf8_lossy().len()
                    <= MAX_SOCKS5_AUTH_BYTES => {}
        _ => return Err(error()),
    }
    Proxy::all(url).map_err(|_| error())
}

#[cfg(target_os = "linux")]
fn tunnel(
    builder: ClientBuilder,
    interface: &str,
    dns_server: std::net::SocketAddr,
) -> Result<ClientBuilder, TransportError> {
    Ok(builder
        .interface(interface)
        .dns_resolver(std::sync::Arc::new(dns::Resolver::new(
            interface, dns_server,
        ))))
}

#[cfg(not(target_os = "linux"))]
fn tunnel(
    _: ClientBuilder,
    _: &str,
    _: std::net::SocketAddr,
) -> Result<ClientBuilder, TransportError> {
    Err(TransportError::fatal("telegram.tunnel_unsupported"))
}
