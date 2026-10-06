/// Kerberos-related functionality for authentication.
use std::io::Cursor;

use hmac::digest::Update;
use hmac::Hmac;
use hmac::Mac;
use md5::Digest;
use md5::Md5;
use sodiumoxide::crypto::secretbox;

use crate::prudp::packet::Rc4;
use crate::rmc::basic::FromStream;
use crate::rmc::basic::FromStreamError;
use crate::rmc::basic::ReadStream;
use crate::rmc::basic::ToStream;

/// The size of the session key in bytes.
pub const SESSION_KEY_SIZE: usize = 16;

/// The internal representation of a Kerberos ticket.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug, ToStream, FromStream)]
pub struct KerberosTicketInternal {
    /// The principal ID.
    pub principle_id: u32,
    /// The expiration time of the ticket.
    pub valid_until: u64,
    /// The session key.
    pub session_key: [u8; SESSION_KEY_SIZE],
    /// The address that asked for the ticket (IPv4 as IPv4-mapped IPv6): only a connection
    /// from there may use it.
    pub issued_to: [u8; 16],
}

impl KerberosTicketInternal {
    /// Records `ip` as the address the ticket is for.
    #[must_use]
    pub fn for_address(mut self, ip: std::net::IpAddr) -> Self {
        self.issued_to = crate::prudp::address_bytes(ip);
        self
    }

    /// Whether a connection from `ip` may use the ticket.
    #[must_use]
    pub fn is_for(&self, ip: std::net::IpAddr) -> bool {
        self.issued_to == crate::prudp::address_bytes(ip)
    }

    /// Whether `ip` is next to the address the ticket was issued to: the same /24 (IPv4) or
    /// /64 (IPv6). A VPN or a carrier's NAT sends a PC's connections out of neighbouring
    /// addresses (Cloudflare WARP: Mailz on eu1, 2026-10-05, asked for the ticket from
    /// 104.28.198.246 and used it from .247), so their game couldn't sign in. Someone replaying
    /// a ticket they saw would have to send from inside the player's own range too.
    #[must_use]
    pub fn is_near(&self, ip: std::net::IpAddr) -> bool {
        let other = crate::prudp::address_bytes(ip);
        let v4_mapped = |a: &[u8; 16]| a[..10] == [0; 10] && a[10..12] == [0xff, 0xff];
        match (v4_mapped(&self.issued_to), v4_mapped(&other)) {
            (true, true) => self.issued_to[..15] == other[..15],
            (false, false) => self.issued_to[..8] == other[..8],
            _ => false,
        }
    }

    /// Seals the ticket using a secret key.
    fn seal(&self, key: &secretbox::Key) -> Vec<u8> {
        let n = secretbox::gen_nonce();
        let mut c = secretbox::seal(&self.to_bytes(), &n, key);

        let mut res = Vec::with_capacity(c.len() + secretbox::NONCEBYTES);
        res.extend_from_slice(n.as_ref());
        res.append(&mut c);
        res
    }

    /// Opens a sealed ticket using a secret key.
    pub fn open(data: &[u8], key: &secretbox::Key) -> Result<Self, FromStreamError> {
        if data.len() < secretbox::NONCEBYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("got {} bytes, required at least {}", data.len(), secretbox::NONCEBYTES),
            )
            .into());
        }
        let n = secretbox::Nonce::from_slice(&data[..secretbox::NONCEBYTES]).unwrap();
        let data = &data[secretbox::NONCEBYTES..];
        let data = secretbox::open(data, &n, key).map_err(|()| std::io::Error::new(std::io::ErrorKind::InvalidData, "open failed"))?;
        let mut stream = ReadStream::from_bytes(data);
        stream.read()
    }
}

/// A Kerberos ticket.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug)]
pub struct KerberosTicket {
    /// The session key.
    pub session_key: [u8; SESSION_KEY_SIZE],
    /// The principal ID.
    pub pid: u32,
    /// The internal ticket.
    pub internal: KerberosTicketInternal,
}

impl KerberosTicket {
    /// Derives a key from a peer PID and a password.
    fn derive_key(peer_pid: u32, password: Option<&str>) -> Vec<u8> {
        // derive key
        let count = 65000 + (peer_pid % 1024);
        let mut key = password.unwrap_or("UbiDummyPwd").as_bytes().to_vec();

        for _ in 0..count {
            let h = Md5::new().chain(key).finalize();
            key = h.to_vec();
        }
        key
    }

    /// Creates a `KerberosTicket` from a byte buffer.
    pub fn from_bytes(buf: &[u8], peer_pid: u32, password: Option<&str>, key: &secretbox::Key) -> Result<Self, FromStreamError> {
        let off = buf.len() - Md5::output_size();
        let (buf, _mac) = buf.split_at(off);

        let obf_key = Self::derive_key(peer_pid, password);
        let buf: Vec<u8> = Rc4::new(&obf_key).zip(buf).map(|(a, b)| a ^ b).collect();
        let mut rdr = ReadStream::from_reader(Cursor::new(buf));

        let session_key = FromStream::from_stream(&mut rdr)?;
        let pid = FromStream::from_stream(&mut rdr)?;
        let internal: Vec<u8> = rdr.read_all()?;
        let internal = KerberosTicketInternal::open(&internal, key)?;

        Ok(Self { session_key, pid, internal })
    }

    /// Converts the ticket to a byte vector.
    #[must_use]
    pub fn as_bytes(&self, peer_pid: u32, password: Option<&str>, key: &secretbox::Key) -> Vec<u8> {
        let mut buf = self.session_key.to_vec();
        buf.append(&mut self.pid.to_bytes());
        buf.append(&mut self.internal.seal(key).to_bytes());

        let key = Self::derive_key(peer_pid, password);

        let mut buf: Vec<u8> = Rc4::new(&key).zip(&buf).map(|(a, b)| a ^ b).collect();

        let mut mac: Hmac<Md5> = Hmac::new_from_slice(&key).unwrap();
        Mac::update(&mut mac, &buf);
        buf.extend(mac.finalize().into_bytes());

        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_is_for_the_address_that_asked() {
        let key = secretbox::gen_key();
        let ticket = KerberosTicketInternal {
            principle_id: 1234,
            valid_until: 1,
            session_key: [7; SESSION_KEY_SIZE],
            issued_to: [0; 16],
        }
        .for_address("203.0.113.5".parse().unwrap());
        let opened = KerberosTicketInternal::open(&ticket.seal(&key), &key).unwrap();
        assert!(opened.is_for("203.0.113.5".parse().unwrap()));
        assert!(opened.is_for("::ffff:203.0.113.5".parse().unwrap()), "either way of writing it");
        assert!(!opened.is_for("203.0.113.6".parse().unwrap()));
    }

    #[test]
    fn a_neighbouring_address_is_near() {
        let t = |ip: &str| {
            KerberosTicketInternal {
                principle_id: 1,
                valid_until: 1,
                session_key: [0; SESSION_KEY_SIZE],
                issued_to: [0; 16],
            }
            .for_address(ip.parse().unwrap())
        };
        // Mailz behind Cloudflare WARP.
        let warp = t("104.28.198.246");
        assert!(warp.is_near("104.28.198.247".parse().unwrap()));
        assert!(warp.is_near("::ffff:104.28.198.1".parse().unwrap()));
        assert!(!warp.is_near("104.28.199.246".parse().unwrap()), "another /24");
        assert!(!warp.is_near("2001:db8::1".parse().unwrap()), "v4 and v6 aren't neighbours");
        let v6 = t("2001:db8:1:2::10");
        assert!(v6.is_near("2001:db8:1:2:ffff::1".parse().unwrap()));
        assert!(!v6.is_near("2001:db8:1:3::10".parse().unwrap()), "another /64");
    }
}
