//! Implements the `OverlordCoreProtocol` for handling core server functionalities
//! and configuration requests.

use quazal::prudp::ClientRegistry;
use quazal::rmc::basic::ToStream;
use quazal::rmc::types::Variant;
use quazal::rmc::Protocol;
use quazal::Context;

use crate::login_or_service_required as login_required;

#[allow(clippy::module_name_repetitions)]
/// Implements the `Protocol` trait for the Overlord Core protocol.
pub struct OverlordCoreProtocol;

impl<T> Protocol<T> for OverlordCoreProtocol {
    /// Returns the unique ID of this protocol.
    fn id(&self) -> u16 {
        5003
    }

    /// Returns the name of this protocol.
    fn name(&self) -> String {
        "OverlordCoreProtocol".into()
    }

    /// Returns the number of methods implemented by this protocol.
    fn num_methods(&self) -> u32 {
        1
    }

    /// Handles incoming Remote Method Call (RMC) requests for the Overlord Core protocol.
    ///
    /// This function dispatches requests based on their method ID.
    fn handle(
        &self,
        _logger: &slog::Logger,
        _ctx: &Context,
        ci: &mut quazal::ClientInfo<T>,
        request: &quazal::rmc::Request,
        _client_registry: &ClientRegistry<T>,
        _socket: &std::net::UdpSocket,
    ) -> std::result::Result<Vec<u8>, quazal::rmc::Error> {
        #[allow(clippy::enum_glob_use)]
        use Variant::*;

        // Ensure the client is logged in before processing the request.
        login_required(&*ci)?;
        // Only method ID 1 (fetch_config) is supported by this protocol.
        if request.method_id != 1 {
            return Err(quazal::rmc::Error::UnknownMethod);
        }

        // Hardcoded configuration values returned by the server.
        let cfg: Vec<(std::string::String, Variant)> = vec![
            ("VER_SERVER_STAGE".to_owned(), I64(10)),
            ("2593515025".to_owned(), I64(1)),
            ("2626349757".to_owned(), I64(1)),
            ("NC_CONNECTION_TICKET_TIMEOUT".to_owned(), F64(10.0)),
            ("NC_CONNECTION_HANDSHAKING_TIMEOUT".to_owned(), F64(5.0)),
            ("314486871".to_owned(), I64(1)),
            ("3759634546".to_owned(), I64(1)),
            ("NC_MAIN_PORT_RANGE".to_owned(), I64(32)),
            ("2449304206".to_owned(), I64(1)),
            ("OVERLORD_VERSION".to_owned(), String("0.8.0.0".into())),
            ("2685611256".to_owned(), I64(1)),
            ("3376331533".to_owned(), I64(1)),
            ("VER_SERVER_CODE".to_owned(), I64(3007)),
            ("FILESERVICE_UNLOCKS_UPLOAD_ENABLED".to_owned(), I64(1)),
            ("2739184075".to_owned(), I64(1)),
            ("2942435614".to_owned(), I64(1)),
            ("SN_FRIENDCHALLENGES_MAX_READ_INTERVAL".to_owned(), F64(900.0)),
            ("NC_CONNECTION_JOIN_TIMEOUT".to_owned(), F64(15.0)),
            ("1027449109".to_owned(), I64(1)),
            ("4175756708".to_owned(), I64(1)),
            ("NC_CONNECTION_INACTIVITY_THRESHOLD".to_owned(), F64(8.0)),
            ("FILESERVICE_ADMIN_RDVID".to_owned(), I64(1119)),
            ("SN_WEEKLYCHALLENGES_ENABLE".to_owned(), I64(1)),
            ("1597953054".to_owned(), I64(1)),
            ("2156388390".to_owned(), I64(1)),
            ("3785106560".to_owned(), I64(1)),
            ("STATS_WRITE_INTERVAL".to_owned(), F64(1.0)),
            ("3835530207".to_owned(), I64(1)),
            ("1492891464".to_owned(), I64(1)),
            ("SN_DAILYCHALLENGES_ENABLE".to_owned(), I64(1)),
            ("2505766166".to_owned(), I64(1)),
            ("11866509".to_owned(), I64(1)),
            ("SN_FRIENDCHALLENGES_ENABLE".to_owned(), I64(1)),
            ("FILESERVICE_UNLOCKS_UPLOAD_INTERVAL".to_owned(), F64(3600.0)),
            ("2524360986".to_owned(), I64(1)),
            ("721797971".to_owned(), I64(1)),
            ("1525666223".to_owned(), I64(1)),
            ("COMMUNITYEVENT_DOUBLECASH".to_owned(), I64(0)),
            ("UPLAY_MAX_RANK_LIMITED_MODE".to_owned(), I64(5)),
            ("SN_GONEDARKCHALLENGES_ENABLE".to_owned(), I64(1)),
            ("_OSDK_VERSION".to_owned(), String("1.4.16.32918".into())),
            ("NC_MAIN_PORT".to_owned(), I64(13000)),
            ("COMMUNITYEVENT_DOUBLEXP".to_owned(), I64(0)),
            ("3804368594".to_owned(), I64(1)),
            ("NC_CONNECTION_CLOSING_TIMEOUT".to_owned(), F64(2.0)),
            ("NC_CONNECTION_ESTABLISHED_TIMEOUT".to_owned(), F64(10.0)),
        ];

        /*
        // Alternative implementation using a HashMap for configuration values.
        // This could be used for more dynamic configuration loading.
         let cfg: std::collections::HashMap<std::string::String, Variant> = [
            ...
        ].iter()
        .cloned()
        .collect();
        */
        Ok(cfg.to_bytes())
    }

    /// Returns the name of the method corresponding to the given `method_id`.
    ///
    /// Returns `None` if the `method_id` is not recognized.
    fn method_name(&self, method_id: u32) -> Option<String> {
        if method_id == 1 {
            Some("fetch_config".into())
        } else {
            None
        }
    }
}

/// Creates a new boxed `OverlordCoreProtocol` instance.
///
/// This function is typically used to register the core protocol
/// with the server's protocol dispatcher.
pub fn new_protocol<T: 'static>() -> Box<dyn Protocol<T>> {
    Box::new(OverlordCoreProtocol)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value as the reply carries it.
    enum V {
        I(i64),
        F(f64),
        S(&'static str),
    }
    use V::F;
    use V::I;
    use V::S;

    /// The configuration the game is sent, in the order it's sent: a copy of the table in
    /// `handle`, kept here so a changed value fails the test too.
    const CONFIG: &[(&str, V)] = &[
        ("VER_SERVER_STAGE", I(10)),
        ("2593515025", I(1)),
        ("2626349757", I(1)),
        ("NC_CONNECTION_TICKET_TIMEOUT", F(10.0)),
        ("NC_CONNECTION_HANDSHAKING_TIMEOUT", F(5.0)),
        ("314486871", I(1)),
        ("3759634546", I(1)),
        ("NC_MAIN_PORT_RANGE", I(32)),
        ("2449304206", I(1)),
        ("OVERLORD_VERSION", S("0.8.0.0")),
        ("2685611256", I(1)),
        ("3376331533", I(1)),
        ("VER_SERVER_CODE", I(3007)),
        ("FILESERVICE_UNLOCKS_UPLOAD_ENABLED", I(1)),
        ("2739184075", I(1)),
        ("2942435614", I(1)),
        ("SN_FRIENDCHALLENGES_MAX_READ_INTERVAL", F(900.0)),
        ("NC_CONNECTION_JOIN_TIMEOUT", F(15.0)),
        ("1027449109", I(1)),
        ("4175756708", I(1)),
        ("NC_CONNECTION_INACTIVITY_THRESHOLD", F(8.0)),
        ("FILESERVICE_ADMIN_RDVID", I(1119)),
        ("SN_WEEKLYCHALLENGES_ENABLE", I(1)),
        ("1597953054", I(1)),
        ("2156388390", I(1)),
        ("3785106560", I(1)),
        ("STATS_WRITE_INTERVAL", F(1.0)),
        ("3835530207", I(1)),
        ("1492891464", I(1)),
        ("SN_DAILYCHALLENGES_ENABLE", I(1)),
        ("2505766166", I(1)),
        ("11866509", I(1)),
        ("SN_FRIENDCHALLENGES_ENABLE", I(1)),
        ("FILESERVICE_UNLOCKS_UPLOAD_INTERVAL", F(3600.0)),
        ("2524360986", I(1)),
        ("721797971", I(1)),
        ("1525666223", I(1)),
        ("COMMUNITYEVENT_DOUBLECASH", I(0)),
        ("UPLAY_MAX_RANK_LIMITED_MODE", I(5)),
        ("SN_GONEDARKCHALLENGES_ENABLE", I(1)),
        ("_OSDK_VERSION", S("1.4.16.32918")),
        ("NC_MAIN_PORT", I(13000)),
        ("COMMUNITYEVENT_DOUBLEXP", I(0)),
        ("3804368594", I(1)),
        ("NC_CONNECTION_CLOSING_TIMEOUT", F(2.0)),
        ("NC_CONNECTION_ESTABLISHED_TIMEOUT", F(10.0)),
    ];

    /// A Quazal string: its length with the closing NUL (u16, little-endian), its bytes, NUL.
    fn string(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&u16::try_from(s.len() + 1).unwrap().to_le_bytes());
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    }

    /// The reply, written out by hand from the layout the game reads (what a server of
    /// Ubisoft's sent): the number of entries (u32, little-endian), then each entry's name
    /// (a string) and value (a variant: a type byte, 1 for an i64, 2 for an f64, 4 for a
    /// string, then the value, little-endian).
    fn expected_reply() -> Vec<u8> {
        let mut out = u32::try_from(CONFIG.len()).unwrap().to_le_bytes().to_vec();
        for (name, value) in CONFIG {
            string(&mut out, name);
            match value {
                I(v) => {
                    out.push(1);
                    out.extend_from_slice(&v.to_le_bytes());
                }
                F(v) => {
                    out.push(2);
                    out.extend_from_slice(&v.to_le_bytes());
                }
                S(v) => {
                    out.push(4);
                    string(&mut out, v);
                }
            }
        }
        out
    }

    #[test]
    /// Tests the `handle` method for method ID 1 (fetch_config).
    ///
    /// Verifies that the returned configuration matches the expected binary data.
    fn test_method1() {
        let logger = slog::Logger::root(slog::Discard, slog::o!());
        let ctx = quazal::Context::default();
        let mut ci = quazal::ClientInfo::<()>::new("127.0.0.1:2".parse().unwrap());
        ci.user_id = Some(1);
        let prot = OverlordCoreProtocol;
        let request = quazal::rmc::Request {
            protocol_id: Protocol::<()>::id(&prot),
            call_id: 1,
            method_id: 1,
            parameters: vec![],
        };
        let resp = prot
            .handle(
                &logger,
                &ctx,
                &mut ci,
                &request,
                &ClientRegistry::default(),
                &std::net::UdpSocket::bind("127.0.0.1:12345").unwrap(),
            )
            .unwrap();

        let expected = expected_reply();
        // The start, checked by hand too: 46 entries, then the first name (17 bytes with its
        // NUL) and its value (an i64, 10).
        assert_eq!(expected[..6], [46, 0, 0, 0, 17, 0]);
        assert_eq!(expected[6..23], *b"VER_SERVER_STAGE\0");
        assert_eq!(expected[23..32], [1, 10, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(expected.len(), resp.len());
        assert_eq!(
            expected.as_slice(),
            resp.as_slice(),
            "{}",
            diff::slice(expected.as_ref(), resp.as_slice())
                .into_iter()
                .map(|diff| match diff {
                    diff::Result::Left(l) => format!("-{l:02x}"),
                    diff::Result::Both(l, _) => format!(" {l:02x}"),
                    diff::Result::Right(r) => format!("+{r:02x}"),
                })
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
