use super::*;
use crate::services::recipient_dns::{check_mail_domain, MailRoute};
use hickory_resolver::{
    config::{ConnectionConfig, NameServerConfig, ResolverConfig},
    net::runtime::TokioRuntimeProvider,
    proto::{
        op::{Message, OpCode},
        rr::{
            rdata::{A, AAAA, CNAME, MX, SOA},
            Name, Record,
        },
    },
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tokio::{net::UdpSocket, task::JoinHandle};

fn name(value: &str) -> Name {
    Name::from_ascii(value).unwrap()
}

fn soa() -> Record {
    Record::from_rdata(
        name("fixture.test."),
        60,
        RData::SOA(SOA::new(
            name("ns.fixture.test."),
            name("hostmaster.fixture.test."),
            1,
            60,
            60,
            3600,
            30,
        )),
    )
}

/// A real loopback UDP server with deterministic DNS wire responses encoded
/// and decoded by Hickory itself, never a public nameserver.
fn respond(request: Message) -> Option<Message> {
    let query = request.queries.first()?.clone();
    let domain = query.name.to_string();
    let mut response = Message::response(request.metadata.id, OpCode::Query);
    response.metadata.authoritative = true;
    response.metadata.recursion_desired = request.metadata.recursion_desired;
    response.metadata.recursion_available = true;
    response.add_query(query.clone());
    match (domain.as_str(), query.query_type) {
        ("timeout.fixture.test.", _) => return None,
        ("broken.fixture.test.", _) => {
            response.metadata.response_code = ResponseCode::ServFail;
        }
        ("refused.fixture.test.", _) => {
            response.metadata.response_code = ResponseCode::Refused;
        }
        ("missing.fixture.test.", _) => {
            response.metadata.response_code = ResponseCode::NXDomain;
            response.add_authority(soa());
        }
        ("unproven.fixture.test.", _) => {
            response.metadata.response_code = ResponseCode::NXDomain;
        }
        ("mx.fixture.test.", RecordType::MX) => {
            response.add_answer(Record::from_rdata(
                query.name,
                120,
                RData::MX(MX::new(10, name("mail.fixture.test."))),
            ));
        }
        ("alias.fixture.test.", RecordType::MX) => {
            response.add_answer(Record::from_rdata(
                query.name,
                60,
                RData::CNAME(CNAME(name("mx.fixture.test."))),
            ));
        }
        ("null.fixture.test.", RecordType::MX) => {
            response.add_answer(Record::from_rdata(
                query.name,
                90,
                RData::MX(MX::new(0, Name::root())),
            ));
        }
        ("v4.fixture.test.", RecordType::A) => {
            response.add_answer(Record::from_rdata(
                query.name,
                90,
                RData::A(A(Ipv4Addr::new(192, 0, 2, 1))),
            ));
        }
        ("v6.fixture.test.", RecordType::AAAA) => {
            response.add_answer(Record::from_rdata(
                query.name,
                90,
                RData::AAAA(AAAA(Ipv6Addr::LOCALHOST)),
            ));
        }
        _ => {
            response.add_authority(soa());
        }
    }
    Some(response)
}

async fn fixture() -> (SystemMailDns, JoinHandle<()>) {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let mut buffer = [0; 4096];
        loop {
            let (len, peer) = socket.recv_from(&mut buffer).await.unwrap();
            let request = Message::from_vec(&buffer[..len]).unwrap();
            if let Some(response) = respond(request) {
                socket
                    .send_to(&response.to_vec().unwrap(), peer)
                    .await
                    .unwrap();
            }
        }
    });
    let mut connection = ConnectionConfig::udp();
    connection.port = port;
    let config = ResolverConfig::from_name_servers(vec![NameServerConfig::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        true,
        vec![connection],
    )]);
    let mut builder = TokioResolver::builder_with_config(config, TokioRuntimeProvider::default());
    configure(builder.options_mut());
    builder.options_mut().edns0 = false;
    let resolver = builder.build().unwrap();
    (SystemMailDns { resolver }, server)
}

#[tokio::test]
async fn mail_01b2_wire_dns_accepts_mx_cname_null_and_implicit_routes() {
    let (resolver, server) = fixture().await;
    assert!(matches!(
        check_mail_domain(&resolver, "mx.fixture.test").await,
        MailRoute::ExplicitMx { ttl: 1..=120 }
    ));
    assert!(matches!(
        check_mail_domain(&resolver, "alias.fixture.test").await,
        MailRoute::ExplicitMx { ttl: 1..=60 }
    ));
    assert!(matches!(
        check_mail_domain(&resolver, "null.fixture.test").await,
        MailRoute::NullMx { ttl: 1..=90 }
    ));
    for domain in ["v4.fixture.test", "v6.fixture.test"] {
        assert!(matches!(
            check_mail_domain(&resolver, domain).await,
            MailRoute::ImplicitMx { ttl: 1..=30 }
        ));
    }
    server.abort();
}

#[tokio::test]
async fn mail_01b2_wire_dns_distinguishes_authoritative_negatives_and_failures() {
    let (resolver, server) = fixture().await;
    assert!(matches!(
        check_mail_domain(&resolver, "missing.fixture.test").await,
        MailRoute::DomainNotFound { ttl: 1..=30 }
    ));
    assert!(matches!(
        check_mail_domain(&resolver, "noroute.fixture.test").await,
        MailRoute::NoRoute { ttl: 1..=30 }
    ));
    for domain in [
        "broken.fixture.test",
        "refused.fixture.test",
        "unproven.fixture.test",
        "timeout.fixture.test",
    ] {
        assert_eq!(check_mail_domain(&resolver, domain).await, MailRoute::Retry);
    }
    server.abort();
}
