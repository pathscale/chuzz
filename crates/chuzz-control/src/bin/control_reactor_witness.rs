use std::sync::Arc;
use std::time::Duration;

use blitz_control_protocol::latest::Once;
use blitz_control_protocol::{NagoyaStream, framed_json_neutral};
use chuzz_control::server::{ArcControlBridge, ControlServer};
use chuzz_control::{AgentControlRequest, AttrScope, ControlResponse};
use endpoint_libs::libs::ws::{MessageStream, WireMessage, transport::TransportStream};
use nagoya::reactor::{Addr, Reactor, TcpStream};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bridge: ArcControlBridge = Arc::new(|request| {
        let answer = Once::new();
        if matches!(request.as_ref(), AgentControlRequest::Inspect { max_depth: 3, .. }) {
            answer.fill(Arc::new(ControlResponse::Ok));
        }
        answer
    });
    let server = ControlServer::start_arc(bridge)?;
    let socket = server.socket_path().to_path_buf();
    let descriptor = socket.with_extension("json");
    let reactor = Reactor::start()?;
    nagoya::block_on(async {
        use std::os::unix::ffi::OsStrExt;
        let stream = TcpStream::connect(Addr::path(socket.as_os_str().as_bytes())?, &reactor.handle()).await?;
        let mut stream = TransportStream::new(framed_json_neutral(NagoyaStream::new(stream)));
        let request = AgentControlRequest::Inspect { root: None, max_depth: 3, include_attrs: AttrScope::None };
        stream.send(WireMessage::Text(serde_json::to_string(&request)?.into())).await
            .map_err(|error| format!("control-send: {error}"))?;
        let response = nagoya::timeout(Duration::from_secs(2), stream.recv()).await
            .map_err(|_| "control-reply: no framed answer")?
            .ok_or("control-reply: socket closed")?
            .map_err(|error| format!("control-reply: {error}"))?;
        let WireMessage::Text(response) = response else { return Err("control-reply: expected text".into()); };
        if serde_json::from_str::<ControlResponse>(&response)? != ControlResponse::Ok {
            return Err("control-reply: wrong Inspect answer".into());
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    drop(server);
    if socket.exists() || descriptor.exists() { return Err("control-teardown: paths survived Drop".into()); }
    println!("control-reply control-teardown: passed");
    Ok(())
}
