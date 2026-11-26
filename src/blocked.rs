//! What a client receives when its query is blocked.
//!
//! The answer is built as plain data from the configured mode, so every case
//! can be checked at the message level without a server. The handler turns it
//! into a response that echoes the query's ID, question and recursion-desired
//! flag.

use std::net::{Ipv4Addr, Ipv6Addr};

use hickory_server::proto::{
    op::ResponseCode,
    rr::{
        Name, RData, Record, RecordType,
        rdata::{A, AAAA, SOA},
    },
};

use crate::config::{Response, ResponseMode};

/// The parts of a blocked response that depend on the mode.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockedAnswer {
    pub response_code: ResponseCode,
    /// The authoritative-answer flag.
    pub authoritative: bool,
    pub answers: Vec<Record>,
    /// The SOA record for the authority section, when the mode has one.
    pub soa: Vec<Record>,
}

/// Build the answer for a blocked query of `qtype` for `qname`, per `config`.
pub fn blocked_answer(config: &Response, qname: &Name, qtype: RecordType) -> BlockedAnswer {
    match config.mode {
        ResponseMode::Null => address_answer(
            qname,
            qtype,
            Some(Ipv4Addr::UNSPECIFIED),
            Some(Ipv6Addr::UNSPECIFIED),
            config.ttl,
        ),
        ResponseMode::Address => address_answer(qname, qtype, config.ipv4, config.ipv6, config.ttl),
        ResponseMode::Nodata => no_data(),
        ResponseMode::Nxdomain => nxdomain(qname, config.ttl),
    }
}

/// An address for A or AAAA queries when one is configured for that family;
/// NOERROR with no records for everything else.
fn address_answer(
    qname: &Name,
    qtype: RecordType,
    ipv4: Option<Ipv4Addr>,
    ipv6: Option<Ipv6Addr>,
    ttl: u32,
) -> BlockedAnswer {
    let record = match qtype {
        RecordType::A => ipv4.map(|ip| Record::from_rdata(qname.clone(), ttl, RData::A(A(ip)))),
        RecordType::AAAA => {
            ipv6.map(|ip| Record::from_rdata(qname.clone(), ttl, RData::AAAA(AAAA(ip))))
        }
        _ => None,
    };
    BlockedAnswer {
        response_code: ResponseCode::NoError,
        authoritative: false,
        answers: record.into_iter().collect(),
        soa: Vec::new(),
    }
}

fn no_data() -> BlockedAnswer {
    BlockedAnswer {
        response_code: ResponseCode::NoError,
        authoritative: false,
        answers: Vec::new(),
        soa: Vec::new(),
    }
}

/// NXDOMAIN, authoritative, with an SOA for the queried name so resolvers can
/// cache the negative answer for `ttl` seconds.
fn nxdomain(qname: &Name, ttl: u32) -> BlockedAnswer {
    let hostmaster = Name::from_ascii("hostmaster.invalid.").unwrap_or_else(|_| Name::root());
    let soa = SOA::new(qname.clone(), hostmaster, 1, 3600, 600, 86400, ttl);
    BlockedAnswer {
        response_code: ResponseCode::NXDomain,
        authoritative: true,
        answers: Vec::new(),
        soa: vec![Record::from_rdata(qname.clone(), ttl, RData::SOA(soa))],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> Name {
        Name::from_ascii(text).expect("name")
    }

    fn config(mode: ResponseMode) -> Response {
        Response {
            mode,
            ..Response::default()
        }
    }

    fn address_config(ipv4: Option<&str>, ipv6: Option<&str>) -> Response {
        Response {
            mode: ResponseMode::Address,
            ipv4: ipv4.map(|ip| ip.parse().expect("ipv4")),
            ipv6: ipv6.map(|ip| ip.parse().expect("ipv6")),
            ..Response::default()
        }
    }

    fn answer(config: &Response, qtype: RecordType) -> BlockedAnswer {
        blocked_answer(config, &name("ads.example."), qtype)
    }

    fn only_rdata(answer: &BlockedAnswer) -> &RData {
        assert_eq!(answer.answers.len(), 1, "{answer:?}");
        &answer.answers[0].data
    }

    #[test]
    fn the_default_mode_is_null_and_answers_a_with_the_unspecified_address() {
        let a = answer(&Response::default(), RecordType::A);
        assert_eq!(a.response_code, ResponseCode::NoError);
        assert_eq!(only_rdata(&a), &RData::A(A(Ipv4Addr::UNSPECIFIED)));
    }

    #[test]
    fn null_mode_answers_aaaa_with_the_unspecified_address() {
        let a = answer(&config(ResponseMode::Null), RecordType::AAAA);
        assert_eq!(a.response_code, ResponseCode::NoError);
        assert_eq!(only_rdata(&a), &RData::AAAA(AAAA(Ipv6Addr::UNSPECIFIED)));
    }

    #[test]
    fn null_mode_answers_other_types_with_noerror_and_no_records() {
        for qtype in [
            RecordType::MX,
            RecordType::TXT,
            RecordType::CNAME,
            RecordType::NS,
            RecordType::HTTPS,
        ] {
            let a = answer(&config(ResponseMode::Null), qtype);
            assert_eq!(a.response_code, ResponseCode::NoError, "{qtype}");
            assert!(a.answers.is_empty(), "{qtype}: {a:?}");
            assert!(a.soa.is_empty(), "{qtype}");
        }
    }

    #[test]
    fn address_mode_answers_each_family_with_its_configured_address() {
        let config = address_config(Some("192.0.2.80"), Some("2001:db8::80"));
        let a4 = answer(&config, RecordType::A);
        let a6 = answer(&config, RecordType::AAAA);
        assert_eq!(only_rdata(&a4), &RData::A(A("192.0.2.80".parse().unwrap())));
        assert_eq!(
            only_rdata(&a6),
            &RData::AAAA(AAAA("2001:db8::80".parse().unwrap()))
        );
        assert!(answer(&config, RecordType::MX).answers.is_empty());
    }

    #[test]
    fn address_mode_with_a_missing_family_answers_noerror_with_no_records() {
        let only_v4 = address_config(Some("192.0.2.80"), None);
        let a = answer(&only_v4, RecordType::AAAA);
        assert_eq!(a.response_code, ResponseCode::NoError);
        assert!(a.answers.is_empty());
        let only_v6 = address_config(None, Some("2001:db8::80"));
        let a = answer(&only_v6, RecordType::A);
        assert_eq!(a.response_code, ResponseCode::NoError);
        assert!(a.answers.is_empty());
    }

    #[test]
    fn nodata_mode_answers_every_type_with_noerror_and_no_records() {
        for qtype in [RecordType::A, RecordType::AAAA, RecordType::MX] {
            let a = answer(&config(ResponseMode::Nodata), qtype);
            assert_eq!(a.response_code, ResponseCode::NoError, "{qtype}");
            assert!(a.answers.is_empty(), "{qtype}");
            assert!(!a.authoritative, "{qtype}");
        }
    }

    #[test]
    fn nxdomain_mode_has_the_aa_flag_and_an_soa_for_the_queried_name() {
        for qtype in [RecordType::A, RecordType::AAAA, RecordType::MX] {
            let a = answer(&config(ResponseMode::Nxdomain), qtype);
            assert_eq!(a.response_code, ResponseCode::NXDomain, "{qtype}");
            assert!(a.authoritative, "{qtype}");
            assert!(a.answers.is_empty(), "{qtype}");
            assert_eq!(a.soa.len(), 1, "{qtype}");
            assert_eq!(a.soa[0].record_type(), RecordType::SOA);
            assert_eq!(a.soa[0].name, name("ads.example."));
        }
    }

    #[test]
    fn only_nxdomain_mode_sets_aa_or_carries_an_soa() {
        for mode in [ResponseMode::Null, ResponseMode::Nodata] {
            let a = answer(&config(mode), RecordType::A);
            assert!(!a.authoritative && a.soa.is_empty(), "{mode:?}");
        }
    }

    #[test]
    fn the_ttl_defaults_to_3600_and_applies_to_every_record() {
        let a = answer(&Response::default(), RecordType::A);
        assert_eq!(a.answers[0].ttl, 3600);
        let n = answer(&config(ResponseMode::Nxdomain), RecordType::A);
        assert_eq!(n.soa[0].ttl, 3600);
    }

    #[test]
    fn a_custom_ttl_applies_to_the_answer_and_to_the_soa_negative_caching_value() {
        let null = Response {
            ttl: 60,
            ..Response::default()
        };
        assert_eq!(answer(&null, RecordType::A).answers[0].ttl, 60);

        let nx = Response {
            ttl: 60,
            ..config(ResponseMode::Nxdomain)
        };
        let a = answer(&nx, RecordType::A);
        assert_eq!(a.soa[0].ttl, 60);
        match &a.soa[0].data {
            RData::SOA(soa) => assert_eq!(soa.minimum, 60),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn records_are_owned_by_the_queried_name() {
        let a = blocked_answer(&Response::default(), &name("Ads.Example."), RecordType::A);
        assert_eq!(a.answers[0].name, name("Ads.Example."));
    }
}
