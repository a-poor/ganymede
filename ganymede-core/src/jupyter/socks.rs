//! SOCKS5 CONNECT with remote DNS, for a loopback SSH dynamic forward.
use std::{io, net::SocketAddr};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

pub(super) async fn connect(proxy: SocketAddr, url: &reqwest::Url) -> io::Result<TcpStream> {
    let invalid = || io::Error::other("invalid SOCKS5 response");
    let mut stream = TcpStream::connect(proxy).await?;
    stream.write_all(&[5, 1, 0]).await?;
    let mut greeting = [0; 2];
    stream.read_exact(&mut greeting).await?;
    if greeting != [5, 0] {
        return Err(invalid());
    }
    let mut request = vec![5, 1, 0];
    match url.host().ok_or_else(invalid)? {
        url::Host::Ipv4(ip) => {
            request.push(1);
            request.extend(ip.octets());
        }
        url::Host::Ipv6(ip) => {
            request.push(4);
            request.extend(ip.octets());
        }
        url::Host::Domain(host) => {
            let len = u8::try_from(host.len()).map_err(|_| invalid())?;
            request.extend([3, len]);
            request.extend(host.as_bytes());
        }
    }
    request.extend(
        url.port_or_known_default()
            .ok_or_else(invalid)?
            .to_be_bytes(),
    );
    stream.write_all(&request).await?;
    let mut reply = [0; 4];
    stream.read_exact(&mut reply).await?;
    if reply[..3] != [5, 0, 0] {
        return Err(io::Error::other("SSH forward could not reach the server"));
    }
    let len = match reply[3] {
        1 => 4,
        4 => 16,
        3 => stream.read_u8().await? as usize,
        _ => return Err(invalid()),
    };
    let mut bound = vec![0; len + 2];
    stream.read_exact(&mut bound).await?;
    Ok(stream)
}
