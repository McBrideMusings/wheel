//! The pairing code a device scans or pastes to pair with a wheeld (ADR-0015), built from the
//! daemon's `remote` and `pairing` replies; `wheel remote pair` and `wheeld pair` both print it.

/// The address a code names: `wanted` when wheeld listens on it, else a Tailscale address
/// (100.64.0.0/10), else the first IP, else the first address at all.
pub fn choose_address(addresses: &[String], wanted: Option<&str>) -> Result<String, String> {
    if let Some(a) = wanted {
        return match addresses.iter().any(|x| x == a) {
            true => Ok(a.to_string()),
            false => Err(format!(
                "{a} is not an address wheeld listens on; they are: {}",
                addresses.join(", ")
            )),
        };
    }
    let octets = |a: &str| -> Option<Vec<u8>> {
        let parts: Vec<u8> = a.split('.').filter_map(|p| p.parse().ok()).collect();
        (parts.len() == 4 && a.split('.').count() == 4).then_some(parts)
    };
    let tailscale =
        |a: &&String| octets(a).is_some_and(|p| p[0] == 100 && (64..=127).contains(&p[1]));
    let is_ip = |a: &&String| a.contains(':') || octets(a).is_some();
    addresses
        .iter()
        .find(tailscale)
        .or_else(|| addresses.iter().find(is_ip))
        .or(addresses.first())
        .cloned()
        .ok_or_else(|| "the daemon reports no address".to_string())
}

/// The code itself: `{cert_sha256, token, url}` as one line of JSON.
pub fn code(fingerprint: &str, token: &str, address: &str, port: u16) -> String {
    let host = match address.contains(':') {
        true => format!("[{address}]"),
        false => address.to_string(),
    };
    serde_json::json!({
        "cert_sha256": fingerprint,
        "token": token,
        "url": format!("wss://{host}:{port}/ws"),
    })
    .to_string()
}

/// Modules of quiet zone around the symbol, as the QR spec asks.
const QUIET: usize = 4;

/// `text` as a QR code in Unicode half blocks, two modules to a character cell, drawn black on a
/// bright white background with ANSI colours so it scans whatever the terminal's own colours are.
pub fn qr(text: &str) -> Result<String, String> {
    let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::L)
        .map_err(|e| format!("the pairing code does not fit in a QR code: {e}"))?;
    let width = code.width();
    let dark = |x: usize, y: usize| {
        x >= QUIET
            && y >= QUIET
            && x - QUIET < width
            && y - QUIET < width
            && code[(x - QUIET, y - QUIET)] == qrcode::Color::Dark
    };
    let side = width + 2 * QUIET;
    let mut out = String::new();
    for y in (0..side).step_by(2) {
        out.push_str("\x1b[30;107m");
        for x in 0..side {
            out.push(match (dark(x, y), dark(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        out.push_str("\x1b[0m\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addrs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_address_prefers_tailscale_then_an_ip() {
        let list = addrs(&["mac.local", "192.168.1.4", "100.101.2.3"]);
        assert_eq!(choose_address(&list, None).unwrap(), "100.101.2.3");
        let list = addrs(&["mac.local", "fe80::1", "192.168.1.4"]);
        assert_eq!(choose_address(&list, None).unwrap(), "fe80::1");
        assert_eq!(
            choose_address(&addrs(&["mac.local"]), None).unwrap(),
            "mac.local"
        );
        assert!(choose_address(&[], None).is_err());
        assert!(choose_address(&list, Some("10.0.0.1")).is_err());
        assert_eq!(
            choose_address(&list, Some("192.168.1.4")).unwrap(),
            "192.168.1.4"
        );
    }

    #[test]
    fn an_ipv6_address_is_bracketed_in_the_url() {
        let c: serde_json::Value = serde_json::from_str(&code("ab", "t", "fe80::1", 7443)).unwrap();
        assert_eq!(c["url"], "wss://[fe80::1]:7443/ws");
    }

    /// The printed blocks, read back into modules, are the encoder's symbol inside its quiet zone.
    #[test]
    fn the_printed_qr_draws_every_module() {
        let text = code(&"f".repeat(64), &"a".repeat(64), "100.101.2.3", 7443);
        let symbol =
            qrcode::QrCode::with_error_correction_level(&text, qrcode::EcLevel::L).unwrap();
        let rows: Vec<Vec<char>> = qr(&text)
            .unwrap()
            .lines()
            .map(|l| {
                l.strip_prefix("\x1b[30;107m")
                    .unwrap()
                    .strip_suffix("\x1b[0m")
                    .unwrap()
                    .chars()
                    .collect()
            })
            .collect();
        let side = symbol.width() + 2 * QUIET;
        assert_eq!(rows.len(), side.div_ceil(2));
        for y in 0..side {
            for x in 0..side {
                let drawn = match rows[y / 2][x] {
                    '█' => true,
                    '▀' => y % 2 == 0,
                    '▄' => y % 2 == 1,
                    ' ' => false,
                    c => panic!("unexpected {c:?}"),
                };
                let inside = (QUIET..QUIET + symbol.width()).contains(&x)
                    && (QUIET..QUIET + symbol.width()).contains(&y);
                let want = inside && symbol[(x - QUIET, y - QUIET)] == qrcode::Color::Dark;
                assert_eq!(drawn, want, "module ({x}, {y})");
            }
        }
    }
}
