//! The card uses the same preference application interface as Pi, on its own socket.
use super::*;
use std::{
    io::{BufReader, Read},
    os::unix::net::UnixStream,
};
use streams_temper::runtime::native_cards::control::{
    CONTROL_TIMEOUT, ControlEndpoint, MAX_CONTROL_FRAME,
};

pub(super) struct Control {
    endpoint: ControlEndpoint,
    stopped: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Control {
    pub fn start(preference_path: &std::path::Path, sender: mpsc::Sender<Wake>) -> Result<Self> {
        let (endpoint, listener) = ControlEndpoint::bind(preference_path)?;
        let stopped = Arc::new(AtomicBool::new(false));
        let stopping = stopped.clone();
        let thread =
            thread::Builder::new()
                .name("streams-card-control".into())
                .spawn(move || {
                    for stream in listener.incoming() {
                        if stopping.load(Ordering::Acquire) {
                            break;
                        }
                        let Ok(stream) = stream else {
                            break;
                        };
                        let read = (|| -> Result<(RequestId, Request)> {
                            stream.set_read_timeout(Some(CONTROL_TIMEOUT))?;
                            stream.set_write_timeout(Some(CONTROL_TIMEOUT))?;
                            let mut line = String::new();
                            BufReader::new((&stream).take(MAX_CONTROL_FRAME))
                                .read_line(&mut line)?;
                            if !line.ends_with('\n') {
                                return Err(CliError::message("incomplete card control request"));
                            }
                            match child_protocol::decode_client(&line)? {
                                ClientFrame::Request {
                                    id,
                                    request:
                                        request @ (Request::CardSettings {}
                                        | Request::SetCardMode { .. }),
                                } => Ok((id, request)),
                                _ => Err(CliError::message(
                                    "card control accepts only preference requests",
                                )),
                            }
                        })();
                        match read {
                            Ok((id, request)) => {
                                if sender.send(Wake::Control(id, request, stream)).is_err() {
                                    break;
                                }
                            }
                            Err(error) => {
                                let _ = writeln!(&stream, "{}", json_error(error));
                            }
                        }
                    }
                })?;
        Ok(Self {
            endpoint,
            stopped,
            thread: Some(thread),
        })
    }
    pub fn path(&self) -> &std::path::Path {
        self.endpoint.path()
    }
}
fn json_error(error: CliError) -> serde_json::Value {
    serde_json::json!({"type":"response","id":1,"outcome":{"status":"error","error":{"kind":"invalid","message":error.to_string()}}})
}
impl Drop for Control {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = UnixStream::connect(self.endpoint.path());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
