use std::io;
use std::time::Duration;

use nagoya::net::UdpSocket;
use nagoya::reactor::{Addr, Handle, Reactor, block_on_with};

const WAIT: Duration = Duration::from_millis(30);

struct Report {
    server_port: u16,
    client_port: u16,
    received: Vec<u32>,
    echoes: Vec<u32>,
    lost: Vec<u32>,
    timeouts: usize,
    empty: bool,
    oversized: bool,
    cancel_recovered: bool,
    shared_port: bool,
}

impl Report {
    fn json(&self) -> String {
        format!(
            concat!(
                "{{\"serverPort\":{},\"clientPort\":{},",
                "\"received\":{:?},\"echoes\":{:?},\"lost\":{:?},",
                "\"timeouts\":{},\"empty\":{},\"oversized\":{},",
                "\"cancelRecovered\":{},\"sharedPort\":{}}}"
            ),
            self.server_port,
            self.client_port,
            self.received,
            self.echoes,
            self.lost,
            self.timeouts,
            self.empty,
            self.oversized,
            self.cancel_recovered,
            self.shared_port,
        )
    }
}

pub fn run() -> io::Result<String> {
    let reactor = Reactor::local()?;
    let handle = reactor.handle();
    let result = block_on_with(
        &reactor,
        nagoya::timeout(Duration::from_secs(8), async {
            let ipv4 = exercise(Addr::localhost(0), &handle).await?;
            let mut localhost = [0; 16];
            localhost[15] = 1;
            let ipv6 = exercise(Addr::V6(localhost, 0), &handle).await?;
            Ok::<_, io::Error>(format!(
                "{{\"ipv4\":{},\"ipv6\":{}}}",
                ipv4.json(),
                ipv6.json(),
            ))
        }),
    );
    reactor.shutdown();
    result
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "UDP fixture exceeded its deadline"))?
}

fn check(value: bool, message: &str) -> io::Result<()> {
    if value {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidData, message))
    }
}

fn packet(sequence: u32) -> [u8; 16] {
    let mut bytes = [0x5a; 16];
    bytes[..4].copy_from_slice(&sequence.to_be_bytes());
    bytes
}

async fn exercise(bind_addr: Addr, handle: &Handle) -> io::Result<Report> {
    let (server, reuse_available) = match UdpSocket::bind_reuse_addr(bind_addr, handle) {
        Ok(server) => (server, true),
        Err(_) => (UdpSocket::bind(bind_addr, handle)?, false),
    };
    let client = UdpSocket::bind(bind_addr, handle)?;
    let server_addr = server.local_addr()?;
    let client_addr = client.local_addr()?;
    check(
        server_addr.port() != 0 && client_addr.port() != 0 && server_addr != client_addr,
        "ephemeral UDP addresses were not distinct",
    )?;

    let mut buffer = [0; 64];
    let mut timeouts = 0;
    check(
        nagoya::timeout(WAIT, server.recv_from(&mut buffer))
            .await
            .is_err(),
        "empty socket did not time out",
    )?;
    timeouts += 1;

    for sequence in 0..8 {
        let payload = packet(sequence);
        check(
            client.send_to(&payload, server_addr).await? == payload.len(),
            "sequenced datagram send was incomplete",
        )?;
    }
    let mut received = Vec::new();
    for sequence in 0..8 {
        let (length, source) = server.recv_from(&mut buffer).await?;
        let expected = packet(sequence);
        check(
            source == client_addr && length == expected.len() && buffer[..length] == expected[..],
            "sequenced datagram payload, source or boundary changed",
        )?;
        received.push(sequence);
    }

    for sequence in (0..8).rev().filter(|sequence| *sequence != 3) {
        let payload = packet(sequence);
        check(
            server.send_to(&payload, client_addr).await? == payload.len(),
            "echo send was incomplete",
        )?;
    }
    let mut echoes = Vec::new();
    for sequence in (0..8).rev().filter(|sequence| *sequence != 3) {
        let (length, source) = client.recv_from(&mut buffer).await?;
        let expected = packet(sequence);
        check(
            source == server_addr && length == expected.len() && buffer[..length] == expected[..],
            "echo order, payload, source or boundary changed",
        )?;
        echoes.push(sequence);
    }
    check(
        nagoya::timeout(WAIT, client.recv_from(&mut buffer))
            .await
            .is_err(),
        "deliberately lost echo unexpectedly arrived",
    )?;
    timeouts += 1;
    let lost: Vec<u32> = (0..8)
        .filter(|sequence| !echoes.contains(sequence))
        .collect();
    check(lost == [3], "loss accounting did not identify sequence 3")?;

    let empty: [u8; 0] = [];
    check(
        client.send_to(&empty, server_addr).await? == 0,
        "empty datagram send returned a nonzero length",
    )?;
    let (length, source) = server.recv_from(&mut buffer).await?;
    check(
        length == 0 && source == client_addr,
        "empty datagram was treated as EOF or came from the wrong source",
    )?;

    let oversized = vec![0x71; 4096];
    check(
        client.send_to(&oversized, server_addr).await? == oversized.len(),
        "oversized datagram send was incomplete",
    )?;
    let mut small = [0; 7];
    let (length, source) = server.recv_from(&mut small).await?;
    check(
        length == small.len() && source == client_addr && small == [0x71; 7],
        "oversized datagram did not fill the receive buffer exactly",
    )?;

    // The timeout drops a pending recv_from future. A fresh receive must still
    // deliver the next datagram intact on the same socket.
    check(
        nagoya::timeout(WAIT, server.recv_from(&mut buffer))
            .await
            .is_err(),
        "pending receive did not time out before cancellation",
    )?;
    timeouts += 1;
    let recovery = packet(82);
    check(
        client.send_to(&recovery, server_addr).await? == recovery.len(),
        "post-cancellation datagram send was incomplete",
    )?;
    let (length, source) = server.recv_from(&mut buffer).await?;
    check(
        source == client_addr && length == recovery.len() && buffer[..length] == recovery[..],
        "receive after cancellation changed the datagram",
    )?;

    let shared_port = if reuse_available {
        match UdpSocket::bind_reuse_addr(server_addr, handle) {
            Ok(shared) => {
                check(
                    shared.local_addr()? == server_addr,
                    "reuse-bound socket did not share the server address",
                )?;
                true
            }
            Err(_) => false,
        }
    } else {
        false
    };

    Ok(Report {
        server_port: server_addr.port(),
        client_port: client_addr.port(),
        received,
        echoes,
        lost,
        timeouts,
        empty: true,
        oversized: true,
        cancel_recovered: true,
        shared_port,
    })
}
