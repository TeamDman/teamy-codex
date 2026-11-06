//! Send one invented two-record OTLP request to an explicitly supplied loopback port.
#[path = "../tests/support/synthetic_otel.rs"]
mod synthetic_otel;

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

#[cfg(not(test))]
fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let port = args
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: send_synthetic_otel <loopback-port>",
            )
        })?;
    if args.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected one port",
        ));
    }
    send(port)?;
    // HTTP success can still indicate partial rejection; inspect viewer counts.
    writeln!(
        io::stdout(),
        "Synthetic request returned HTTP 200; check viewer acceptance counts."
    )
}

/// Send the fixture to IPv4 loopback using a bounded connection and response.
///
/// # Errors
/// Returns an error for an invalid port, transport failure or non-200 response.
pub fn send(port: u16) -> io::Result<()> {
    if port == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "port must be nonzero",
        ));
    }
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
    socket.set_read_timeout(Some(Duration::from_secs(3)))?;
    socket.set_write_timeout(Some(Duration::from_secs(3)))?;
    let body = synthetic_otel::payload();
    write!(
        socket,
        "POST /v1/logs HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    socket.write_all(&body)?;
    let mut response = Vec::new();
    socket.take(4096).read_to_end(&mut response)?;
    if !response.starts_with(b"HTTP/1.1 200") {
        return Err(io::Error::other("collector did not return HTTP 200"));
    }
    Ok(())
}
