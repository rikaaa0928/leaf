#[cfg(feature = "outbound-rog-h3")]
use std::io::{self, ErrorKind};
#[cfg(feature = "outbound-rog-h3")]
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

#[cfg(feature = "outbound-rog-h3")]
use bytes::{Buf, BufMut, Bytes, BytesMut};

#[cfg(feature = "outbound-rog-h3")]
use crate::session::SocksAddr;

#[cfg(feature = "outbound-rog-h3")]
pub const H3_RAW_STREAM_PATH: &str = "/moe.rikaaa0928.rog.RogService/RawStream";
#[cfg(feature = "outbound-rog-h3")]
pub const H3_RAW_UDP_PATH: &str = "/moe.rikaaa0928.rog.RogService/RawUdp";

#[cfg(feature = "outbound-rog-h3")]
pub const MAX_LPM_FRAME_SIZE: usize = 16 * 1024 * 1024;

#[cfg(feature = "outbound-rog-h3")]
pub const ATYP_SAME: u8 = 0x00;
#[cfg(feature = "outbound-rog-h3")]
pub const ATYP_IPV4: u8 = 0x01;
#[cfg(feature = "outbound-rog-h3")]
pub const ATYP_DOMAIN: u8 = 0x03;
#[cfg(feature = "outbound-rog-h3")]
pub const ATYP_IPV6: u8 = 0x04;

/// Standard 5-byte gRPC Length-Prefixed-Message (LPM) decoder:
/// `[Compressed-Flag (1B = 0x00)][Message-Length (4B u32 BE)][Payload]`
#[cfg(feature = "outbound-rog-h3")]
#[derive(Default)]
pub struct LpmDecoder {
    buf: BytesMut,
}

#[cfg(feature = "outbound-rog-h3")]
impl LpmDecoder {
    pub fn new() -> Self {
        Self {
            buf: BytesMut::with_capacity(64 * 1024),
        }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    pub fn next_frame(&mut self) -> io::Result<Option<Bytes>> {
        if self.buf.len() < 5 {
            return Ok(None);
        }
        let len =
            u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if len > MAX_LPM_FRAME_SIZE {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("LPM frame length {} exceeds max {}", len, MAX_LPM_FRAME_SIZE),
            ));
        }
        if self.buf.len() < 5 + len {
            return Ok(None);
        }
        self.buf.advance(5);
        Ok(Some(self.buf.split_to(len).freeze()))
    }
}

/// Wrap raw payload bytes in a 5-byte gRPC LPM frame
#[cfg(feature = "outbound-rog-h3")]
pub fn encode_lpm_frame(payload: &[u8]) -> Bytes {
    let mut out = BytesMut::with_capacity(5 + payload.len());
    out.put_u8(0x00);
    out.put_u32(payload.len() as u32);
    out.extend_from_slice(payload);
    out.freeze()
}

#[cfg(feature = "outbound-rog-h3")]
pub fn encode_addr_port(buf: &mut BytesMut, host: &str, port: u16) -> io::Result<()> {
    let trimmed = host.trim_matches(['[', ']']);
    if let Ok(v4) = trimmed.parse::<Ipv4Addr>() {
        buf.put_u8(ATYP_IPV4);
        buf.extend_from_slice(&v4.octets());
    } else if let Ok(v6) = trimmed.parse::<Ipv6Addr>() {
        buf.put_u8(ATYP_IPV6);
        buf.extend_from_slice(&v6.octets());
    } else {
        let domain_bytes = trimmed.as_bytes();
        if domain_bytes.is_empty() || domain_bytes.len() > 255 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("invalid domain length: {}", domain_bytes.len()),
            ));
        }
        buf.put_u8(ATYP_DOMAIN);
        buf.put_u8(domain_bytes.len() as u8);
        buf.extend_from_slice(domain_bytes);
    }
    buf.put_u16(port);
    Ok(())
}

#[cfg(feature = "outbound-rog-h3")]
pub fn decode_addr_port(buf: &mut Bytes) -> io::Result<(String, u16)> {
    if buf.remaining() < 1 {
        return Err(io::Error::new(ErrorKind::UnexpectedEof, "missing ATYP"));
    }
    let atyp = buf.get_u8();
    let host = match atyp {
        ATYP_IPV4 => {
            if buf.remaining() < 4 + 2 {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "truncated IPv4 address",
                ));
            }
            let mut octets = [0u8; 4];
            buf.copy_to_slice(&mut octets);
            Ipv4Addr::from(octets).to_string()
        }
        ATYP_IPV6 => {
            if buf.remaining() < 16 + 2 {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "truncated IPv6 address",
                ));
            }
            let mut octets = [0u8; 16];
            buf.copy_to_slice(&mut octets);
            Ipv6Addr::from(octets).to_string()
        }
        ATYP_DOMAIN => {
            if buf.remaining() < 1 {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "missing domain length",
                ));
            }
            let dlen = buf.get_u8() as usize;
            if buf.remaining() < dlen + 2 {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "truncated domain address",
                ));
            }
            let domain_bytes = buf.copy_to_bytes(dlen);
            String::from_utf8(domain_bytes.to_vec())
                .map_err(|_| io::Error::new(ErrorKind::InvalidData, "invalid UTF-8 domain"))?
        }
        other => {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("unsupported ATYP: {:#x}", other),
            ));
        }
    };
    let port = buf.get_u16();
    Ok((host, port))
}

/// Encode TCP Frame 0: `[AuthLen(1B)][Auth][ATYP][DstAddr][DstPort(2B)][InitialPayload]`
/// wrapped in a 5-byte LPM frame.
#[cfg(feature = "outbound-rog-h3")]
pub fn encode_tcp_handshake_frame(
    auth: &str,
    dst_host: &str,
    dst_port: u16,
    initial_payload: &[u8],
) -> io::Result<Bytes> {
    let auth_bytes = auth.as_bytes();
    if auth_bytes.len() > 255 {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "auth password exceeds 255 bytes",
        ));
    }
    let mut inner = BytesMut::with_capacity(32 + initial_payload.len());
    inner.put_u8(auth_bytes.len() as u8);
    inner.extend_from_slice(auth_bytes);
    encode_addr_port(&mut inner, dst_host, dst_port)?;
    if !initial_payload.is_empty() {
        inner.extend_from_slice(initial_payload);
    }
    Ok(encode_lpm_frame(&inner))
}

/// Encode UDP Frame 0: `[AuthLen(1B)][Auth][SrcATYP][SrcAddr][SrcPort(2B)]`
#[cfg(feature = "outbound-rog-h3")]
pub fn encode_udp_handshake_frame(auth: &str, src_host: &str, src_port: u16) -> io::Result<Bytes> {
    let auth_bytes = auth.as_bytes();
    if auth_bytes.len() > 255 {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "auth password exceeds 255 bytes",
        ));
    }
    let mut inner = BytesMut::with_capacity(32);
    inner.put_u8(auth_bytes.len() as u8);
    inner.extend_from_slice(auth_bytes);
    encode_addr_port(&mut inner, src_host, src_port)?;
    Ok(encode_lpm_frame(&inner))
}

/// Encode UDP datagram frame with `ATYP_SAME (0x00)` compression when target matches previous packet.
#[cfg(feature = "outbound-rog-h3")]
pub fn encode_udp_packet_frame(
    dst_host: &str,
    dst_port: u16,
    last_dst: &mut Option<(String, u16)>,
    payload: &[u8],
) -> io::Result<Bytes> {
    let is_same = last_dst
        .as_ref()
        .is_some_and(|(h, p)| h == dst_host && *p == dst_port);

    let mut inner = BytesMut::with_capacity(24 + payload.len());
    if is_same {
        inner.put_u8(ATYP_SAME);
    } else {
        encode_addr_port(&mut inner, dst_host, dst_port)?;
        *last_dst = Some((dst_host.to_string(), dst_port));
    }
    inner.extend_from_slice(payload);
    Ok(encode_lpm_frame(&inner))
}

/// Decode UDP datagram frame into `(SocksAddr, Bytes)` without extra Vec allocation.
#[cfg(feature = "outbound-rog-h3")]
pub fn decode_udp_packet_frame(
    mut frame: Bytes,
    last_dst: &mut Option<(String, u16)>,
) -> io::Result<(SocksAddr, Bytes)> {
    if frame.remaining() < 1 {
        return Err(io::Error::new(
            ErrorKind::UnexpectedEof,
            "empty UDP packet frame",
        ));
    }
    let peek_atyp = frame[0];
    let (dst_addr, dst_port) = if peek_atyp == ATYP_SAME {
        frame.advance(1);
        last_dst.clone().ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidData,
                "received ATYP_SAME before any explicit destination address",
            )
        })?
    } else {
        let parsed = decode_addr_port(&mut frame)?;
        *last_dst = Some(parsed.clone());
        parsed
    };

    let socks_addr = if let Ok(ip) = dst_addr.parse::<IpAddr>() {
        SocksAddr::Ip(SocketAddr::new(ip, dst_port))
    } else {
        SocksAddr::Domain(dst_addr, dst_port)
    };

    Ok((socks_addr, frame))
}

#[cfg(all(test, feature = "outbound-rog-h3"))]
mod tests {
    use super::*;

    #[test]
    fn test_udp_same_addr_compression() {
        let mut enc_last = None;
        let mut dec_last = None;

        let p1 = encode_udp_packet_frame("8.8.8.8", 53, &mut enc_last, b"query1").unwrap();
        let p2 = encode_udp_packet_frame("8.8.8.8", 53, &mut enc_last, b"query2").unwrap();
        assert_eq!(p1.len() - p2.len(), 6);

        let mut dec = LpmDecoder::new();
        dec.push(&p1);
        dec.push(&p2);

        let (addr1, data1) =
            decode_udp_packet_frame(dec.next_frame().unwrap().unwrap(), &mut dec_last).unwrap();
        assert_eq!(addr1.to_string(), "8.8.8.8:53");
        assert_eq!(&data1[..], b"query1");

        let (addr2, data2) =
            decode_udp_packet_frame(dec.next_frame().unwrap().unwrap(), &mut dec_last).unwrap();
        assert_eq!(addr2.to_string(), "8.8.8.8:53");
        assert_eq!(&data2[..], b"query2");
    }
}
