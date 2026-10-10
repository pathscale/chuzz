//! Native, synchronous DTLS/SRTP fixture driven by a ps-qa page.
//!
//! Packets stay in Rust. Only counters and certificate fingerprints reach JS.
//! Virtual time advances to protocol deadlines; no sockets, sleeps, threads,
//! channels, codecs or browser peer-connection shims are involved.

use std::fmt::Display;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use blitz_script::ScriptDocument;
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};
use p256::ecdsa::{DerSignature, SigningKey, VerifyingKey, signature::Verifier};
use p256::pkcs8::{DecodePrivateKey, DecodePublicKey};
use str0m::config::Fingerprint;
use str0m::crypto::dtls::{DtlsCert, ProtocolVersion};
use str0m::format::Codec;
use str0m::media::MediaKind;
use str0m::net::{Receive, Transmit};
use str0m::rtp::{RawPacket, RtpWrite, Ssrc};
use str0m::{Candidate, Event, Input, Output, Rtc, RtcError};
use x509_cert::Certificate;
use x509_cert::der::{Decode, Encode};

const PACKETS_PER_PEER: usize = 3;
const FIRST_SEQUENCE: u64 = 65_534;
const VIRTUAL_LIMIT: Duration = Duration::from_secs(6);
const OUTPUT_LIMIT: usize = 512;
const CYCLE_LIMIT: usize = 4096;
const CAPTURE_LIMIT: usize = 64;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    certificates_verified: usize,
    fingerprints: [String; 2],
    dtls_connected: [bool; 2],
    dtls_version: &'static str,
    dropped_dtls_packets: usize,
    retransmission_verified: bool,
    protected_rtp: [usize; 2],
    authenticated_rtp: [usize; 2],
    authenticated_rtcp: [usize; 2],
    tampered_packets: usize,
    tamper_rejected: bool,
    replayed_packets: usize,
    replay_rejected: bool,
    rollover_verified: bool,
    wrong_fingerprints_rejected: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Other,
    Rtp,
    Rtcp,
}

fn kind(bytes: &[u8]) -> Kind {
    if bytes.len() < 2 || !(128..192).contains(&bytes[0]) {
        return Kind::Other;
    }
    if (64..96).contains(&(bytes[1] & 0x7f)) {
        Kind::Rtcp
    } else {
        Kind::Rtp
    }
}

struct Wire {
    from: usize,
    packet: Transmit,
}

struct Pair {
    peers: [Rtc; 2],
    start: Instant,
    now: Instant,
    wall_deadline: Instant,
    connected: [bool; 2],
    received: [Vec<(u64, Arc<[u8]>)>; 2],
    rtcp_received: [usize; 2],
    captured: Vec<Wire>,
    drop_first_dtls: bool,
    dropped_dtls: usize,
}

fn driver_failure(message: &str) -> RtcError {
    RtcError::Io(std::io::Error::other(message))
}

fn message(error: impl Display) -> String {
    error.to_string()
}

fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}

fn payload(side: usize, index: usize) -> [u8; 8] {
    // Opaque encoded-payload markers. W02 verifies transport, not Opus decoding.
    [0xf8, side as u8, index as u8, 0x57, 0x30, 0x32, 0x33, 0x6d]
}

impl Pair {
    fn new(
        certificates: &[DtlsCert; 2],
        wrong_pin: Option<usize>,
        drop_first_dtls: bool,
    ) -> Result<Self, RtcError> {
        let now = Instant::now();
        let make = |certificate: DtlsCert| {
            super::crypto::config()
                .set_dtls_cert(certificate)
                .set_rtp_mode(true)
                .enable_raw_packets(true)
                .set_rtcp_report_interval_audio(Duration::from_millis(100))
                .build(now)
        };
        let mut peers = [
            make(certificates[0].clone()),
            make(certificates[1].clone()),
        ];
        let addresses: [SocketAddr; 2] = [
            (Ipv4Addr::LOCALHOST, 40_002).into(),
            (Ipv4Addr::LOCALHOST, 40_003).into(),
        ];
        let candidates = [
            Candidate::host(addresses[0], "udp").expect("constant QA candidate"),
            Candidate::host(addresses[1], "udp").expect("constant QA candidate"),
        ];
        let fingerprints = [
            peers[0].direct_api().local_dtls_fingerprint().clone(),
            peers[1].direct_api().local_dtls_fingerprint().clone(),
        ];
        let credentials = [
            peers[0].direct_api().local_ice_credentials(),
            peers[1].direct_api().local_ice_credentials(),
        ];

        for side in 0..2 {
            let other = 1 - side;
            peers[side].add_local_candidate(candidates[side].clone());
            peers[side].add_remote_candidate(candidates[other].clone());
            let mut expected = fingerprints[other].clone();
            if wrong_pin == Some(side) {
                expected.bytes[0] ^= 1;
            }
            let mut direct = peers[side].direct_api();
            direct.set_remote_fingerprint(expected);
            direct.set_remote_ice_credentials(credentials[other].clone());
            direct.set_ice_controlling(side == 0);
            let mid = "w02".into();
            direct.declare_media(mid, MediaKind::Audio);
            direct.declare_stream_tx((42 + side as u32).into(), None, mid, None);
            direct
                .expect_stream_rx((42 + other as u32).into(), None, mid, None)
                .reset_roc(0);
            direct.start_dtls(side == 0)?;
        }
        for peer in &mut peers {
            peer.handle_input(Input::Timeout(now))?;
        }

        Ok(Self {
            peers,
            start: now,
            now,
            wall_deadline: now + Duration::from_secs(10),
            connected: [false; 2],
            received: [Vec::new(), Vec::new()],
            rtcp_received: [0; 2],
            captured: Vec::new(),
            drop_first_dtls,
            dropped_dtls: 0,
        })
    }

    fn receive(&mut self, wire: &Wire, bytes: &[u8]) -> Result<(), RtcError> {
        let received = Receive::new(
            wire.packet.proto,
            wire.packet.source,
            wire.packet.destination,
            bytes,
        )?;
        self.peers[1 - wire.from].handle_input(Input::Receive(self.now, received))
    }

    fn cycle(&mut self, capture: bool) -> Result<([Instant; 2], bool), RtcError> {
        if Instant::now() >= self.wall_deadline {
            return Err(driver_failure("crypto fixture exceeded its wall deadline"));
        }
        let mut deadlines = [self.now; 2];
        let mut progressed = false;
        for side in 0..2 {
            let mut reached_timeout = false;
            for _ in 0..OUTPUT_LIMIT {
                match self.peers[side].poll_output()? {
                    Output::Timeout(deadline) => {
                        deadlines[side] = deadline;
                        reached_timeout = true;
                        break;
                    }
                    Output::Transmit(packet) => {
                        progressed = true;
                        let is_dtls = packet
                            .contents
                            .first()
                            .is_some_and(|byte| (20..64).contains(byte));
                        if self.drop_first_dtls && is_dtls {
                            self.drop_first_dtls = false;
                            self.dropped_dtls += 1;
                            continue;
                        }
                        let wire = Wire { from: side, packet };
                        if capture && kind(&wire.packet.contents) != Kind::Other {
                            if self.captured.len() >= CAPTURE_LIMIT {
                                return Err(driver_failure("crypto capture limit reached"));
                            }
                            self.captured.push(wire);
                        } else {
                            self.receive(&wire, &wire.packet.contents)?;
                        }
                    }
                    Output::Event(event) => {
                        progressed = true;
                        match event {
                            Event::Connected => self.connected[side] = true,
                            Event::RtpPacket(packet) => {
                                if self.received[side].len() >= 16 {
                                    return Err(driver_failure("unexpected RTP event count"));
                                }
                                self.received[side].push((*packet.seq_no, packet.payload));
                            }
                            Event::RawPacket(raw) => {
                                if matches!(*raw, RawPacket::RtcpRx(_)) {
                                    self.rtcp_received[side] += 1;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            if !reached_timeout {
                return Err(driver_failure("crypto output limit reached"));
            }
        }
        Ok((deadlines, progressed))
    }

    fn until(
        &mut self,
        capture: bool,
        complete: impl Fn(&Self) -> bool,
    ) -> Result<(), RtcError> {
        for _ in 0..CYCLE_LIMIT {
            if self.now >= self.start + VIRTUAL_LIMIT {
                return Err(driver_failure("crypto fixture exceeded its protocol deadline"));
            }
            let (deadlines, progressed) = self.cycle(capture)?;
            if complete(self) {
                return Ok(());
            }
            if !progressed {
                self.now = deadlines[0]
                    .min(deadlines[1])
                    .max(self.now + Duration::from_micros(1))
                    .min(self.start + VIRTUAL_LIMIT);
                for peer in &mut self.peers {
                    peer.handle_input(Input::Timeout(self.now))?;
                }
            }
        }
        Err(driver_failure("crypto cycle limit reached"))
    }

    fn settle(&mut self) -> Result<(), RtcError> {
        for _ in 0..64 {
            if !self.cycle(true)?.1 {
                return Ok(());
            }
        }
        Err(driver_failure("crypto fixture did not settle"))
    }

    fn send_markers(&mut self) -> Result<(), String> {
        for side in 0..2 {
            let pt = self.peers[side]
                .codec_config()
                .params()
                .iter()
                .find(|params| params.spec().codec == Codec::Opus)
                .ok_or_else(|| "Opus RTP parameters are unavailable".to_owned())?
                .pt();
            let ssrc: Ssrc = (42 + side as u32).into();
            let mut direct = self.peers[side].direct_api();
            let stream = direct
                .stream_tx(&ssrc)
                .ok_or_else(|| "fixture send stream is unavailable".to_owned())?;
            for index in 0..PACKETS_PER_PEER {
                let bytes: Arc<[u8]> = Arc::from(payload(side, index).as_slice());
                stream.write_rtp(RtpWrite::new(
                    pt,
                    (FIRST_SEQUENCE + index as u64).into(),
                    (index as u32 + 1) * 960,
                    self.now,
                    bytes,
                ));
            }
        }
        Ok(())
    }

    fn counts(&self) -> [usize; 2] {
        [self.received[0].len(), self.received[1].len()]
    }

    fn has_samples(&self) -> bool {
        let mut rtp = [0; 2];
        let mut rtcp = [0; 2];
        for wire in &self.captured {
            match kind(&wire.packet.contents) {
                Kind::Rtp => rtp[wire.from] += 1,
                Kind::Rtcp => rtcp[wire.from] += 1,
                Kind::Other => {}
            }
        }
        rtp == [PACKETS_PER_PEER; 2] && rtcp.iter().all(|count| *count > 0)
    }
}

fn verify_certificate(certificate: &DtlsCert) -> Result<(), String> {
    let parsed = Certificate::from_der(&certificate.certificate).map_err(message)?;
    require(
        parsed.tbs_certificate.issuer == parsed.tbs_certificate.subject,
        "certificate is not self-issued",
    )?;
    let spki = parsed
        .tbs_certificate
        .subject_public_key_info
        .to_der()
        .map_err(message)?;
    let public_key = VerifyingKey::from_public_key_der(&spki).map_err(message)?;
    let private_key = SigningKey::from_pkcs8_der(&certificate.private_key).map_err(message)?;
    require(
        private_key.verifying_key() == &public_key,
        "P-256 certificate and private key do not match",
    )?;
    let signature_bytes = parsed
        .signature
        .as_bytes()
        .ok_or_else(|| "certificate signature is not octet-aligned".to_owned())?;
    let signature = DerSignature::from_bytes(signature_bytes).map_err(message)?;
    public_key
        .verify(
            &parsed.tbs_certificate.to_der().map_err(message)?,
            &signature,
        )
        .map_err(message)
}

fn run_fixture() -> Result<Report, String> {
    let provider = super::crypto::provider();
    let generate = || {
        provider
            .dtls_provider
            .generate_certificate()
            .ok_or_else(|| "P-256 certificate generation failed".to_owned())
    };
    let certificates = [generate()?, generate()?];
    for certificate in &certificates {
        verify_certificate(certificate)?;
    }
    let fingerprints: [Fingerprint; 2] = std::array::from_fn(|side| Fingerprint {
        hash_func: "sha-256".into(),
        bytes: provider
            .sha256_provider
            .sha256(&certificates[side].certificate)
            .to_vec(),
    });
    require(
        fingerprints[0] != fingerprints[1],
        "ephemeral certificate fingerprints are identical",
    )?;

    let mut pair = Pair::new(&certificates, None, true).map_err(message)?;
    pair.until(false, |pair| pair.connected == [true; 2])
        .map_err(message)?;
    for side in 0..2 {
        require(pair.peers[side].is_connected(), "peer is not connected")?;
        let direct = pair.peers[side].direct_api();
        require(
            direct.dtls_protocol_version() == Some(ProtocolVersion::DTLS1_2),
            "fixture did not negotiate DTLS 1.2",
        )?;
        require(
            direct.local_dtls_fingerprint() == &fingerprints[side]
                && direct.remote_dtls_fingerprint() == Some(&fingerprints[1 - side]),
            "authenticated fingerprints do not match certificate DER",
        )?;
    }
    require(pair.dropped_dtls == 1, "DTLS loss was not exercised")?;
    require(pair.counts() == [0; 2], "unexpected initial RTP payload")?;
    let initial_rtcp = pair.rtcp_received;

    pair.send_markers()?;
    pair.until(true, Pair::has_samples).map_err(message)?;
    let mut rtp = Vec::new();
    let mut rtcp: [Option<Wire>; 2] = [None, None];
    for wire in std::mem::take(&mut pair.captured) {
        let side = wire.from;
        match kind(&wire.packet.contents) {
            Kind::Rtp => rtp.push(wire),
            Kind::Rtcp if rtcp[side].is_none() => rtcp[side] = Some(wire),
            _ => {}
        }
    }
    let protected_rtp = [
        rtp.iter().filter(|wire| wire.from == 0).count(),
        rtp.iter().filter(|wire| wire.from == 1).count(),
    ];
    require(
        protected_rtp == [PACKETS_PER_PEER; 2],
        "incorrect protected RTP packet count",
    )?;

    let mut tampered_packets = 0;
    for wire in rtp.iter().chain(rtcp.iter().flatten()) {
        let mut modified = wire.packet.contents.to_vec();
        require(modified.len() > 12, "protected datagram is too short")?;
        // Inside the authentication tag for RTP and both SRTCP layouts.
        let offset = modified.len() - 5;
        modified[offset] ^= 1;
        pair.receive(wire, &modified).map_err(message)?;
        pair.settle().map_err(message)?;
        tampered_packets += 1;
    }
    let tamper_rejected = pair.counts() == [0; 2] && pair.rtcp_received == initial_rtcp;
    require(
        tamper_rejected,
        "tampered SRTP or SRTCP reached an authenticated receive event",
    )?;

    let mut replayed_packets = 0;
    for wire in &rtp {
        pair.receive(wire, &wire.packet.contents).map_err(message)?;
        pair.settle().map_err(message)?;
        let before = pair.counts();
        for _ in 0..2 {
            pair.receive(wire, &wire.packet.contents).map_err(message)?;
            pair.settle().map_err(message)?;
            replayed_packets += 1;
        }
        require(pair.counts() == before, "SRTP replay was delivered")?;
    }
    require(
        pair.counts() == [PACKETS_PER_PEER; 2],
        "authenticated SRTP payload count is incorrect",
    )?;
    for side in 0..2 {
        for (index, (sequence, bytes)) in pair.received[side].iter().enumerate() {
            require(
                *sequence == FIRST_SEQUENCE + index as u64
                    && bytes.as_ref() == payload(1 - side, index).as_slice(),
                "SRTP payload or rollover sequence is incorrect",
            )?;
        }
    }
    let rollover_verified = pair
        .received
        .iter()
        .all(|packets| packets.last().is_some_and(|packet| packet.0 == 65_536));
    require(rollover_verified, "SRTP rollover was not exercised")?;

    for wire in rtcp.iter().flatten() {
        pair.receive(wire, &wire.packet.contents).map_err(message)?;
        pair.settle().map_err(message)?;
    }
    let authenticated_rtcp = [
        pair.rtcp_received[0] - initial_rtcp[0],
        pair.rtcp_received[1] - initial_rtcp[1],
    ];
    require(
        authenticated_rtcp.iter().all(|count| *count > 0),
        "valid SRTCP did not authenticate after tampering",
    )?;

    let mut wrong_fingerprints_rejected = 0;
    for side in 0..2 {
        let mut rejected = Pair::new(&certificates, Some(side), false).map_err(message)?;
        match rejected.until(false, |pair| pair.connected[side]) {
            Err(RtcError::RemoteSdp(reason)) if reason == "remote fingerprint no match" => {
                require(
                    !rejected.connected[side] && rejected.counts() == [0; 2],
                    "wrong fingerprint exposed connected state or media",
                )?;
                wrong_fingerprints_rejected += 1;
            }
            Err(error) => return Err(format!("wrong fingerprint failed for another reason: {error}")),
            Ok(()) => return Err("wrong certificate fingerprint was accepted".into()),
        }
    }

    Ok(Report {
        certificates_verified: certificates.len(),
        fingerprints: fingerprints.map(|fingerprint| fingerprint.to_string()),
        dtls_connected: pair.connected,
        dtls_version: "DTLS1.2",
        dropped_dtls_packets: pair.dropped_dtls,
        retransmission_verified: pair.dropped_dtls == 1 && pair.connected == [true; 2],
        protected_rtp,
        authenticated_rtp: pair.counts(),
        authenticated_rtcp,
        tampered_packets,
        tamper_rejected,
        replayed_packets,
        replay_rejected: pair.counts() == [PACKETS_PER_PEER; 2],
        rollover_verified,
        wrong_fingerprints_rejected,
    })
}

fn probe(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let report = run_fixture()
        .map_err(|error| JsNativeError::error().with_message(error))?;
    let json = serde_json::to_string(&report)
        .map_err(|error| JsNativeError::error().with_message(error.to_string()))?;
    Ok(JsString::from(json).into())
}

pub(super) fn install(document: &mut ScriptDocument) {
    document.with_js_context(|context| {
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzWebrtcCryptoProbe"),
                0,
                NativeFunction::from_fn_ptr(probe),
            )
            .expect("WebRTC crypto native fixture bridge");
    });
}

