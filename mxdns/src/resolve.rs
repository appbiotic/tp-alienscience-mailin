use crate::err::{Error, Result};
use dnssector::{
    constants::{Class, Type, DNS_MAX_COMPRESSED_SIZE},
    DNSIterable, DNSSector, ParsedPacket, RdataIterable, DNS_FLAG_TC,
};
use smol::{future::FutureExt, net::UdpSocket, Timer};
use std::{
    io::{self, ErrorKind::TimedOut},
    net::{IpAddr, SocketAddr},
    time::Duration,
};

const DNS_PORT: u16 = 53;
const SOURCE_ADDR: &str = "0.0.0.0:0";

pub struct Resolve {
    dns_server: SocketAddr,
    timeout: Duration,
}

impl Resolve {
    pub fn new<T: Into<IpAddr>>(dns_server_ip: T, timeout: Duration) -> Self {
        let addr = SocketAddr::new(dns_server_ip.into(), DNS_PORT);
        Self {
            dns_server: addr,
            timeout,
        }
    }

    async fn query(&self, packet: ParsedPacket, name: &[u8]) -> Result<ParsedPacket> {
        let raw_packet = packet.into_packet();
        let raw_response = self.query_raw(&raw_packet).await?;
        let response = DNSSector::new(raw_response)
            .map_err(|e| Error::ParseResponse(query_string(name), e))?
            .parse()
            .map_err(|e| Error::ParseResponse(query_string(name), e))?;
        if response.flags() & DNS_FLAG_TC == DNS_FLAG_TC {
            return Err(Error::TcpUnsupported(
                query_string(name),
                self.dns_server.to_string(),
            ));
        }
        Ok(response)
    }

    async fn query_raw(&self, packet: &[u8]) -> Result<Vec<u8>> {
        let socket = UdpSocket::bind(SOURCE_ADDR).await.map_err(Error::Bind)?;
        socket
            .connect(self.dns_server)
            .await
            .map_err(|e| Error::Connect(self.dns_server.to_string(), e))?;
        socket
            .send(&packet)
            .await
            .map_err(|e| Error::Send(self.dns_server.to_string(), e))?;
        let mut response = vec![0; DNS_MAX_COMPRESSED_SIZE];
        let len = socket
            .recv(&mut response)
            .or(self.timeout())
            .await
            .map_err(|e| Error::Recv(self.dns_server.to_string(), e))?;
        response.truncate(len);
        Ok(response)
    }

    async fn timeout(&self) -> io::Result<usize> {
        Timer::after(self.timeout).await;
        Err(TimedOut.into())
    }

    pub async fn query_a(&self, name: &[u8]) -> Result<Vec<IpAddr>> {
        let query = dnssector::gen::query(name, Type::A, Class::IN)
            .map_err(|e| Error::DnsQuery(query_string(name), e))?;
        let response = self.query(query, name).await?;
        extract_ips(response, name)
    }
}

fn extract_ips(mut packet: ParsedPacket, query_name: &[u8]) -> Result<Vec<IpAddr>> {
    use std::result::Result as StdResult;

    let mut ips = Vec::new();
    let mut response = packet.into_iter_answer();
    while let Some(i) = response {
        ips.push(i.rr_ip());
        response = i.next();
    }
    let (ips, errors): (Vec<_>, Vec<_>) = ips.into_iter().partition(StdResult::is_ok);
    if ips.is_empty() {
        if let Some(Err(e)) = errors.into_iter().nth(0) {
            let query = String::from_utf8_lossy(query_name).to_string();
            return Err(Error::ExtractIps(query, e));
        }
    }
    let ips: Vec<_> = ips.into_iter().map(StdResult::unwrap).collect();
    Ok(ips)
}

fn query_string(query: &[u8]) -> String {
    String::from_utf8_lossy(query).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use display_bytes::display_bytes;
    use std::{
        matches,
        net::{IpAddr, Ipv4Addr},
    };

    const SERVER: IpAddr = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
    const TIMEOUT: Duration = Duration::from_secs(2);
    const EXAMPLE_SERVER: &[u8] = b"example.com";
    const EXAMPLE_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));

    #[test]
    fn query_a() {
        let resolve = Resolve::new(SERVER, TIMEOUT);
        let addresses = smol::block_on(async { resolve.query_a(EXAMPLE_SERVER).await.unwrap() });
        let found = addresses.into_iter().any(|ip| ip == EXAMPLE_IP);
        assert!(
            found,
            "{} did not resolve to {}",
            display_bytes(EXAMPLE_SERVER),
            EXAMPLE_IP
        );
    }

    #[test]
    fn query_cname() {
        const EXAMPLE_CNAME: &[u8] = b"www.alienscience.org";
        const EXAMPLE_CNAME_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(116, 203, 10, 186));
        let resolve = Resolve::new(SERVER, TIMEOUT);
        let addresses = smol::block_on(async { resolve.query_a(EXAMPLE_CNAME).await.unwrap() });
        let found = addresses.into_iter().any(|ip| ip == EXAMPLE_CNAME_IP);
        assert!(
            found,
            "{} did not resolve to {}",
            display_bytes(EXAMPLE_CNAME),
            EXAMPLE_CNAME_IP
        );
    }

    #[test]
    fn query_timeout() {
        let resolve = Resolve::new(SERVER, Duration::from_micros(1));
        let res = smol::block_on(async { resolve.query_a(EXAMPLE_SERVER).await });
        assert!(
            matches!(&res, Err(Error::Recv(_, err)) if err.kind() == TimedOut),
            "Unexpected result {:?}",
            res
        );
    }
}
