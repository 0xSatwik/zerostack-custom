//! TCP relay: localhost ports -> Instabox box over the Tailscale SOCKS5 proxy.
//! Rust port of /root/pi-bridge/tailnet-relay.py (B4).
//!
//! Altare runs Tailscale in userspace-networking mode (no /dev/net/tun), so
//! ordinary processes cannot reach Tailscale IPs directly. tailscaled exposes
//! a SOCKS5 server on 127.0.0.1:1055. This relay listens on localhost and
//! forwards each connection through that SOCKS5 proxy to the Instabox box
//! (100.105.110.103).
//!
//! Forwarded ports:
//!   8931 -> Instabox Playwright MCP server (legacy; kept for parity)
//!   6081 -> Instabox browser control page (viewer + Stop/Start)
//!   6080 -> Instabox noVNC (websockify, the VNC viewer)
//! Run in tmux: tmux new -s tailnet-relay -d '/root/pi-bridge/tailnet-relay'

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::time::Duration;

const SOCKS_HOST: &str = "127.0.0.1";
const SOCKS_PORT: u16 = 1055;
const TARGET_HOST: Ipv4Addr = Ipv4Addr::new(100, 105, 110, 103);

const FORWARDS: &[(u16, u16)] = &[
    (8931, 8931), // Playwright MCP (legacy)
    (6081, 6081), // browser control page
    (6080, 6080), // noVNC
];

/// Open a TCP connection to target via the SOCKS5 proxy. Returns the socket.
fn socks5_connect(target_port: u16) -> std::io::Result<TcpStream> {
    let mut s = TcpStream::connect((SOCKS_HOST, SOCKS_PORT))?;
    s.set_read_timeout(Some(Duration::from_secs(15)))?;
    s.set_write_timeout(Some(Duration::from_secs(15)))?;
    // greeting: VER=5, NMETHODS=1, METHODS=[0x00 no auth]
    s.write_all(&[0x05, 0x01, 0x00])?;
    let mut resp = [0u8; 2];
    s.read_exact(&mut resp)?;
    if resp[0] != 0x05 || resp[1] != 0x00 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("SOCKS5 greeting rejected: {:?}", resp),
        ));
    }
    // request: VER=5, CMD=1 connect, RSV=0, ATYP=1 IPv4, addr, port
    let mut req = vec![0x05, 0x01, 0x00, 0x01];
    req.extend_from_slice(&TARGET_HOST.octets());
    req.extend_from_slice(&target_port.to_be_bytes());
    s.write_all(&req)?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head)?;
    if head[0] != 0x05 || head[1] != 0x00 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("SOCKS5 connect failed: {:?}", head),
        ));
    }
    // Read the rest of the reply (BND.ADDR + BND.PORT).
    let atyp = head[3];
    let rest_len = match atyp {
        0x01 => 6, // IPv4 (4) + port (2)
        0x03 => {
            // domain: 1 length byte first
            let mut len_b = [0u8; 1];
            s.read_exact(&mut len_b)?;
            (len_b[0] as usize) + 2
        }
        0x04 => 18, // IPv6 (16) + port (2)
        _ => 0,
    };
    if rest_len > 0 {
        let mut rest = vec![0u8; rest_len];
        s.read_exact(&mut rest)?;
    }
    s.set_read_timeout(None)?;
    s.set_write_timeout(None)?;
    Ok(s)
}

fn relay(mut a: TcpStream, mut b: TcpStream) {
    let mut buf = [0u8; 65536];
    loop {
        match a.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if b.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = a.shutdown(std::net::Shutdown::Both);
    let _ = b.shutdown(std::net::Shutdown::Both);
}

fn handle(client: TcpStream, target_port: u16) {
    let remote = match socks5_connect(target_port) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("relay to {} failed: {}", target_port, e);
            return;
        }
    };
    let client2 = match client.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    let remote2 = match remote.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    let t1 = std::thread::spawn(move || relay(client, remote));
    let t2 = std::thread::spawn(move || relay(remote2, client2));
    let _ = t1.join();
    let _ = t2.join();
}

fn serve(local_port: u16, target_port: u16) {
    let srv = match TcpListener::bind(("127.0.0.1", local_port)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot bind 127.0.0.1:{}: {}", local_port, e);
            return;
        }
    };
    eprintln!("relaying 127.0.0.1:{} -> {}:{}", local_port, TARGET_HOST, target_port);
    for conn in srv.incoming() {
        match conn {
            Ok(client) => {
                std::thread::spawn(move || handle(client, target_port));
            }
            Err(_) => continue,
        }
    }
}

fn main() {
    let mut handles = Vec::new();
    for &(local_port, target_port) in FORWARDS {
        handles.push(std::thread::spawn(move || serve(local_port, target_port)));
    }
    for h in handles {
        let _ = h.join();
    }
}
