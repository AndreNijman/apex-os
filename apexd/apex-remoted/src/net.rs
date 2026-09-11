//! Reading this machine's own addresses, and the length-prefixed transport.
//!
//! ## Why `getifaddrs` and not a crate
//!
//! One `libc` call the workspace already links, against three crates and a
//! transitive tree for a list of IP addresses that goes in a QR code. The
//! addresses are a *hint*: a device tries them and falls back, and getting
//! one wrong costs a failed connection attempt rather than a wrong
//! connection — the Noise handshake is what decides whether the far end is
//! this machine.
//!
//! ## The transport framing
//!
//! Each Noise message is written as `[u32 big-endian length][ciphertext]`.
//! The length is the transport's, not the frame's: the receiver has to know
//! how many bytes to decrypt before it can see anything inside, and a length
//! written twice is two lengths that can disagree.

use std::io::{Read, Write};
use std::net::IpAddr;

/// The largest message the transport will read.
///
/// A Noise transport message cannot exceed 65535 bytes, so anything claiming
/// more is either a client that has lost sync or one trying to make this
/// process allocate. Refused before a byte is reserved.
pub const MAX_MESSAGE: usize = apex_remote_core::noise::MAX_MESSAGE;

/// Write one length-prefixed message.
pub fn write_message(w: &mut impl Write, message: &[u8]) -> std::io::Result<()> {
    if message.len() > MAX_MESSAGE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("a message of {} bytes exceeds {MAX_MESSAGE}", message.len()),
        ));
    }
    w.write_all(&(message.len() as u32).to_be_bytes())?;
    w.write_all(message)?;
    w.flush()
}

/// Read one length-prefixed message.
pub fn read_message(r: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_MESSAGE {
        // Refused before allocating. A four-byte header claiming four
        // gigabytes is the cheapest denial of service there is.
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("a peer announced a {len}-byte message; the limit is {MAX_MESSAGE}"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// Every non-loopback address this machine has.
///
/// Loopback is excluded because a phone can never reach it, and a QR code
/// that offers `127.0.0.1` costs the device one timeout before it moves on.
/// Link-local IPv6 is excluded for a subtler reason: `fe80::…` is only usable
/// with a scope id, which is the *receiver's* interface index and therefore
/// meaningless written down on this side.
pub fn local_addresses() -> Vec<IpAddr> {
    let mut out = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // Safe: getifaddrs allocates a list and returns 0 on success; the list is
    // freed below on every path out.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return out;
    }
    let mut cur = head;
    while !cur.is_null() {
        // Safe: `cur` is non-null and points at a node the kernel wrote.
        let node = unsafe { &*cur };
        if let Some(ip) = address_of(node) {
            let usable = !ip.is_loopback()
                && match ip {
                    IpAddr::V4(v4) => !v4.is_unspecified(),
                    IpAddr::V6(v6) => {
                        !v6.is_unspecified() && !(v6.segments()[0] & 0xffc0 == 0xfe80)
                    }
                };
            if usable && !out.contains(&ip) {
                out.push(ip);
            }
        }
        cur = node.ifa_next;
    }
    // Safe: `head` came from a successful getifaddrs and is freed once.
    unsafe { libc::freeifaddrs(head) };
    out
}

fn address_of(node: &libc::ifaddrs) -> Option<IpAddr> {
    if node.ifa_addr.is_null() {
        return None;
    }
    // Safe: checked non-null; the family field is the first member of every
    // sockaddr variant and is what says which variant it is.
    let family = unsafe { (*node.ifa_addr).sa_family } as i32;
    match family {
        libc::AF_INET => {
            let sa = node.ifa_addr as *const libc::sockaddr_in;
            // Safe: the family says this is a sockaddr_in.
            let raw = unsafe { (*sa).sin_addr.s_addr };
            Some(IpAddr::V4(std::net::Ipv4Addr::from(u32::from_be(raw))))
        }
        libc::AF_INET6 => {
            let sa = node.ifa_addr as *const libc::sockaddr_in6;
            // Safe: the family says this is a sockaddr_in6.
            let raw = unsafe { (*sa).sin6_addr.s6_addr };
            Some(IpAddr::V6(std::net::Ipv6Addr::from(raw)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_round_trips_through_the_length_prefix() {
        for payload in [vec![], vec![0u8], vec![7u8; 1000], vec![0xffu8; MAX_MESSAGE]] {
            let mut buf = Vec::new();
            write_message(&mut buf, &payload).expect("write");
            assert_eq!(buf.len(), payload.len() + 4);
            let mut cursor = std::io::Cursor::new(buf);
            assert_eq!(read_message(&mut cursor).expect("read"), payload);
        }
    }

    #[test]
    fn an_announced_length_beyond_the_limit_is_refused_before_allocating() {
        // The cheapest denial of service there is: four bytes claiming four
        // gigabytes. This must fail on the header, not on the read that
        // follows it — measured by giving it a stream with nothing after the
        // header at all.
        let mut header = (u32::MAX).to_be_bytes().to_vec();
        header.truncate(4);
        let mut cursor = std::io::Cursor::new(header);
        let e = read_message(&mut cursor).expect_err("an oversized length was accepted");
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        assert!(e.to_string().contains("the limit is"), "{e}");
    }

    #[test]
    fn writing_an_oversized_message_is_refused_rather_than_truncated() {
        let mut buf = Vec::new();
        let e = write_message(&mut buf, &vec![0u8; MAX_MESSAGE + 1])
            .expect_err("an oversized message was written");
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
        assert!(buf.is_empty(), "a refused write still emitted bytes");
    }

    #[test]
    fn a_truncated_stream_is_an_error_and_not_a_short_message() {
        // Two ways a connection dies mid-message, and neither may look like a
        // valid short message to the frame decoder above.
        let mut only_header = 10u32.to_be_bytes().to_vec();
        only_header.extend_from_slice(b"abc");
        let mut cursor = std::io::Cursor::new(only_header);
        assert!(read_message(&mut cursor).is_err());
        let mut cursor = std::io::Cursor::new(vec![0u8, 0]);
        assert!(read_message(&mut cursor).is_err());
    }

    #[test]
    fn local_addresses_are_reachable_ones_only() {
        // Runs against this machine's real interfaces. The assertion is the
        // rule rather than a fixed answer, so it holds on a laptop, in a
        // container with one veth, and on a host with none.
        for ip in local_addresses() {
            assert!(!ip.is_loopback(), "{ip} is loopback");
            if let IpAddr::V6(v6) = ip {
                assert_ne!(
                    v6.segments()[0] & 0xffc0,
                    0xfe80,
                    "{v6} is link-local and needs a scope id nobody can write down"
                );
            }
        }
    }
}
