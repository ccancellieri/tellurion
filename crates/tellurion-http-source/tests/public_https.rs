use std::{net::IpAddr, time::Duration};

use tellurion_http_source::{
    is_public_address, validate_public_url, Budget, BudgetErrorKind, BudgetLimits,
};

#[test]
fn accepts_only_unambiguous_public_https_locators() {
    let locator = validate_public_url("https://bücher.example/data.tif").expect("valid locator");
    assert_eq!(locator.display_name(), "xn--bcher-kva.example");
    assert!(!locator.fingerprint().contains("data.tif"));
    for raw_url in [
        "http://example.com/data.tif",
        "https://example.com:444/data.tif",
        "https://user@example.com/data.tif",
        "https://example.com/data.tif?token=secret",
        "https://example.com/data.tif#fragment",
        "https://example.com/%2e%2e/data.tif",
    ] {
        assert!(validate_public_url(raw_url).is_err(), "{raw_url}");
    }
}

#[test]
fn rejects_non_public_addresses_including_special_ipv6() {
    for address in [
        "10.0.0.1",
        "100.64.0.1",
        "127.0.0.1",
        "169.254.1.1",
        "192.0.2.1",
        "224.0.0.1",
        "240.0.0.1",
        "fc00::1",
        "fe80::1",
        "ff02::1",
        "2001:db8::1",
        "2001:2::1",
        "2001:20::1",
        "2002::1",
        "3fff::1",
        "::ffff:10.0.0.1",
    ] {
        assert!(
            !is_public_address(address.parse::<IpAddr>().expect("address")),
            "{address}"
        );
    }
    assert!(is_public_address("8.8.8.8".parse().unwrap()));
    assert!(is_public_address("2606:4700:4700::1111".parse().unwrap()));
}

#[test]
fn budget_refuses_work_before_it_starts_and_recovers_after_oversized_charge() {
    let budget = Budget::new(BudgetLimits {
        requests: 2,
        bytes: 3,
        deadline: Duration::from_secs(1),
        concurrent: 1,
    });
    assert_eq!(
        budget.reserve(4).unwrap_err().kind(),
        BudgetErrorKind::ByteLimit
    );

    let budget = Budget::new(BudgetLimits {
        requests: 2,
        bytes: 3,
        deadline: Duration::from_secs(1),
        concurrent: 1,
    });
    assert_eq!(
        budget.reserve(2).unwrap().finish(4).unwrap_err().kind(),
        BudgetErrorKind::ByteLimit
    );
    assert_eq!(
        budget.reserve(1).unwrap_err().kind(),
        BudgetErrorKind::Invalidated
    );
}
