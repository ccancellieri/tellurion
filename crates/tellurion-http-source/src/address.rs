use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Returns whether an address is eligible for public outbound HTTPS.
pub fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [first, second, third, _] = address.octets();
    !matches!(
        (first, second, third),
        (0, _, _)
            | (10, _, _)
            | (100, 64..=127, _)
            | (127, _, _)
            | (169, 254, _)
            | (172, 16..=31, _)
            | (192, 0, 0)
            | (192, 0, 2)
            | (192, 168, _)
            | (192, 88, 99)
            | (198, 18..=19, _)
            | (198, 51, 100)
            | (203, 0, 113)
            | (224..=u8::MAX, _, _)
    )
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }

    let segments = address.segments();
    // The broker deliberately allows only ordinary global-unicast space. IANA's
    // special-purpose entries are fail-closed even when they are technically
    // forwardable: they are not suitable public egress destinations.
    if (segments[0] & 0xe000) != 0x2000 {
        return false;
    }
    // IETF protocol assignments, including Teredo, benchmarking, ORCHID,
    // ORCHIDv2, documentation, and related special-purpose allocations.
    if segments[0] == 0x2001 && (segments[1] < 0x0200 || segments[1] == 0x0db8) {
        return false;
    }
    // 6to4.
    if segments[0] == 0x2002 {
        return false;
    }
    // Documentation (RFC 9637).
    if segments[0] == 0x3fff && segments[1] < 0x1000 {
        return false;
    }
    // Direct Delegation AS112 service.
    segments[..3] != [0x2620, 0x004f, 0x8000]
}
