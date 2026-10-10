(function () {
  var button = document.getElementById("run");
  var result = document.getElementById("result");
  var evidence = document.getElementById("evidence");
  var rows = [
    "certificates", "dtls", "retransmission", "srtp", "srtcp",
    "fingerprints", "tamper", "replay", "rollover"
  ];

  function show(id, passed, text) {
    var element = document.getElementById(id);
    element.setAttribute("data-status", passed ? "pass" : "fail");
    element.textContent = text + (passed ? " PASS" : " FAIL");
  }

  function both(values, expected) {
    return Array.isArray(values) && values.length === 2
      && values[0] === expected && values[1] === expected;
  }

  function run() {
    result.setAttribute("data-status", "pending");
    result.textContent = "Crypto running";
    evidence.textContent = "Waiting for native evidence";
    rows.forEach(function (id) {
      var element = document.getElementById(id);
      element.setAttribute("data-status", "pending");
      element.textContent = id + " pending";
    });

    try {
      if (typeof globalThis.__chuzzWebrtcCryptoProbe !== "function") {
        throw new Error("Native WebRTC crypto fixture is unavailable");
      }
      var report = JSON.parse(globalThis.__chuzzWebrtcCryptoProbe());
      evidence.textContent = JSON.stringify(report, null, 2);

      var checks = [
        [
          "certificates",
          report.certificatesVerified === 2
            && Array.isArray(report.fingerprints)
            && report.fingerprints.length === 2
            && report.fingerprints[0] !== report.fingerprints[1],
          "P-256 DER certificates and self-signatures"
        ],
        [
          "dtls",
          both(report.dtlsConnected, true) && report.dtlsVersion === "DTLS1.2",
          "Authenticated DTLS 1.2 peers"
        ],
        [
          "retransmission",
          report.droppedDtlsPackets === 1 && report.retransmissionVerified === true,
          "Handshake after dropped DTLS packet"
        ],
        [
          "srtp",
          both(report.protectedRtp, 3) && both(report.authenticatedRtp, 3),
          "SRTP: 3 protected and recovered payloads per peer"
        ],
        [
          "srtcp",
          Array.isArray(report.authenticatedRtcp)
            && report.authenticatedRtcp.length === 2
            && report.authenticatedRtcp[0] > 0
            && report.authenticatedRtcp[1] > 0,
          "Authenticated SRTCP reports from both peers"
        ],
        [
          "fingerprints",
          report.wrongFingerprintsRejected === 2,
          "Wrong fingerprint rejected in both DTLS roles"
        ],
        [
          "tamper",
          report.tamperedPackets === 8 && report.tamperRejected === true,
          "6 modified SRTP and 2 modified SRTCP packets rejected"
        ],
        [
          "replay",
          report.replayedPackets === 12 && report.replayRejected === true,
          "12 duplicate SRTP packets rejected"
        ],
        [
          "rollover",
          report.rolloverVerified === true,
          "SRTP sequence 65534 through 65536 recovered"
        ]
      ];
      var passed = true;
      checks.forEach(function (check) {
        show(check[0], check[1], check[2]);
        passed = passed && check[1];
      });
      result.setAttribute("data-status", passed ? "pass" : "fail");
      result.textContent = passed ? "READY: crypto PASS" : "Crypto FAIL";
    } catch (error) {
      rows.forEach(function (id) {
        show(id, false, id);
      });
      evidence.textContent = String(error && error.message ? error.message : error);
      result.setAttribute("data-status", "fail");
      result.textContent = "Crypto FAIL";
    }
  }

  button.addEventListener("click", run);
})();

