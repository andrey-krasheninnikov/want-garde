use hickory_proto::{
    op::{Message, MessageType, OpCode, Query, ResponseCode},
    rr::{DNSClass, Name as DnsName, RData, RecordType},
};
use reqwest::dns::{Name, Resolve, Resolving};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    fmt, io,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpSocket, UdpSocket},
    time::timeout,
};

const MAX_DNS_BYTES: usize = 4096;
const DNS_BUDGET: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub(super) struct Resolver {
    interface: String,
    server: SocketAddr,
}

#[derive(Debug)]
pub(super) enum Failure {
    Interface,
    Lookup,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Interface => "DNS interface binding failed",
            Self::Lookup => "DNS lookup through the configured interface failed",
        })
    }
}

impl std::error::Error for Failure {}

impl From<io::Error> for Failure {
    fn from(_: io::Error) -> Self {
        Self::Lookup
    }
}

impl Resolver {
    pub(super) fn new(interface: &str, server: SocketAddr) -> Self {
        Self {
            interface: interface.to_owned(),
            server,
        }
    }

    fn socket(&self, kind: Type, protocol: Protocol) -> Result<Socket, Failure> {
        let socket = Socket::new(Domain::for_address(self.server), kind, Some(protocol))?;
        socket
            .bind_device(Some(self.interface.as_bytes()))
            .map_err(|_| Failure::Interface)?;
        socket.set_nonblocking(true)?;
        Ok(socket)
    }

    async fn udp(&self, query: &[u8]) -> Result<Message, Failure> {
        let socket = self.socket(Type::DGRAM, Protocol::UDP)?;
        let unspecified = if self.server.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let any: SocketAddr = unspecified.parse().expect("static wildcard address");
        socket.bind(&any.into())?;
        let socket = UdpSocket::from_std(socket.into())?;
        socket.connect(self.server).await?;
        socket.send(query).await?;
        let mut bytes = [0; MAX_DNS_BYTES];
        let len = socket.recv(&mut bytes).await?;
        Message::from_vec(&bytes[..len]).map_err(|_| Failure::Lookup)
    }

    async fn tcp(&self, query: &[u8]) -> Result<Message, Failure> {
        let socket = self.socket(Type::STREAM, Protocol::TCP)?;
        let socket = TcpSocket::from_std_stream(socket.into());
        let mut stream = socket.connect(self.server).await?;
        let len = u16::try_from(query.len()).map_err(|_| Failure::Lookup)?;
        stream.write_u16(len).await?;
        stream.write_all(query).await?;
        let len = usize::from(stream.read_u16().await?);
        if len > MAX_DNS_BYTES {
            return Err(Failure::Lookup);
        }
        let mut bytes = vec![0; len];
        stream.read_exact(&mut bytes).await?;
        Message::from_vec(&bytes).map_err(|_| Failure::Lookup)
    }

    async fn exchange(
        &self,
        name: DnsName,
        record_type: RecordType,
    ) -> Result<Vec<IpAddr>, Failure> {
        let mut query = Message::query();
        query.metadata.recursion_desired = true;
        query.add_query(Query::query(name.clone(), record_type));
        let wire = query.to_vec().map_err(|_| Failure::Lookup)?;
        let matches_query = |response: &Message| {
            response.metadata.id == query.metadata.id
                && response.metadata.message_type == MessageType::Response
                && response.metadata.op_code == OpCode::Query
                && response.queries == query.queries
        };
        let mut response = self.udp(&wire).await?;
        if matches_query(&response) && response.metadata.truncation {
            response = self.tcp(&wire).await?;
        }
        if !matches_query(&response)
            || response.metadata.truncation
            || response.metadata.response_code != ResponseCode::NoError
        {
            return Err(Failure::Lookup);
        }

        let mut owner = &name;
        for _ in 0..8 {
            let mut alias = None;
            let addresses: Vec<_> = response
                .answers
                .iter()
                .filter_map(|answer| {
                    if &answer.name != owner || answer.dns_class != DNSClass::IN {
                        return None;
                    }
                    match &answer.data {
                        RData::A(address) if record_type == RecordType::A => {
                            Some(IpAddr::V4(address.0))
                        }
                        RData::AAAA(address) if record_type == RecordType::AAAA => {
                            Some(IpAddr::V6(address.0))
                        }
                        RData::CNAME(target) => {
                            alias = Some(&target.0);
                            None
                        }
                        _ => None,
                    }
                })
                .take(16)
                .collect();
            if !addresses.is_empty() {
                return Ok(addresses);
            }
            let Some(next) = alias else {
                return Ok(Vec::new());
            };
            owner = next;
        }
        Err(Failure::Lookup)
    }

    async fn lookup(&self, name: &str) -> Result<Vec<SocketAddr>, Failure> {
        if name != "api.telegram.org" {
            return Err(Failure::Lookup);
        }
        let host = DnsName::from_ascii("api.telegram.org.").map_err(|_| Failure::Lookup)?;
        let (ipv4, ipv6) = tokio::join!(
            timeout(DNS_BUDGET, self.exchange(host.clone(), RecordType::A)),
            timeout(DNS_BUDGET, self.exchange(host, RecordType::AAAA))
        );
        let mut addresses = Vec::new();
        let mut error = Failure::Lookup;
        for result in [ipv4, ipv6] {
            match result {
                Ok(Ok(ips)) => addresses.extend(ips.into_iter().map(|ip| SocketAddr::new(ip, 0))),
                Ok(Err(Failure::Interface)) => error = Failure::Interface,
                _ => {}
            }
        }
        if addresses.is_empty() {
            Err(error)
        } else {
            Ok(addresses)
        }
    }
}

impl Resolve for Resolver {
    fn resolve(&self, name: Name) -> Resolving {
        let resolver = self.clone();
        Box::pin(async move {
            let addresses = resolver.lookup(name.as_str()).await?;
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}
