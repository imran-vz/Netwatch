//! The app's side of the fan helper's socket (see
//! `utilify_smc::protocol`): connect and say HELLO, send requests, and tell
//! apart the ways it can be unavailable, so the UI can say what to do.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use utilify_smc::protocol::{self, Fans, Request, Response};

/// The LaunchDaemon `install.sh` puts in place; its absence means the
/// helper isn't installed (as opposed to installed but not running).
pub const HELPER_PLIST: &str = "/Library/LaunchDaemons/dev.utilify.fanhelper.plist";
/// What the UI tells people to run to get fan control.
pub const INSTALL_COMMAND: &str = "./install.sh";
/// How long a request may take before the helper counts as gone. Taking a
/// fan from the system can take the helper up to 12 s.
const IO_TIMEOUT: Duration = Duration::from_secs(20);
/// Reconnect attempts back off from this...
const BACKOFF_MIN: Duration = Duration::from_secs(1);
/// ...doubling up to this.
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Why the helper can't be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    /// No LaunchDaemon: never installed (or removed).
    NotInstalled,
    /// Installed, but nothing is listening on the socket.
    NotRunning,
    /// The socket or the helper refused this user (not an admin).
    PermissionDenied,
    /// The helper answered `ERR <message>`, e.g. the SMC refused a write.
    Refused(String),
    /// The connection broke or the helper said something unexpected.
    Broken(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotInstalled => write!(f, "the fan helper isn't installed"),
            ClientError::NotRunning => write!(f, "the fan helper isn't running"),
            ClientError::PermissionDenied => {
                write!(f, "the fan helper only accepts admin users")
            }
            ClientError::Refused(msg) => write!(f, "the fan helper refused: {msg}"),
            ClientError::Broken(msg) => write!(f, "lost the fan helper: {msg}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Where to find the helper. Tests point it at their own socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperPaths {
    pub socket: PathBuf,
    /// Checked when the socket is missing; `None` skips the check.
    pub plist: Option<PathBuf>,
}

impl Default for HelperPaths {
    fn default() -> Self {
        Self {
            socket: protocol::SOCKET_PATH.into(),
            plist: Some(HELPER_PLIST.into()),
        }
    }
}

struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

/// One connection to the helper, made on demand, with backoff after
/// failures.
pub struct HelperClient {
    paths: HelperPaths,
    conn: Option<Conn>,
    /// Fan count from HELLO.
    fans: u8,
    last_request: Option<Instant>,
    /// No connecting before this, after a failure.
    retry_at: Option<Instant>,
    backoff: Duration,
    /// Why the last connect failed, repeated while backing off.
    last_failure: Option<ClientError>,
}

impl HelperClient {
    pub fn new(paths: HelperPaths) -> Self {
        Self {
            paths,
            conn: None,
            fans: 0,
            last_request: None,
            retry_at: None,
            backoff: BACKOFF_MIN,
            last_failure: None,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.conn.is_some()
    }

    /// The fan count the helper reported.
    pub fn fan_count(&self) -> u8 {
        self.fans
    }

    /// Connects (and says HELLO) unless already connected. While backing
    /// off from a failure, returns that failure without trying.
    pub fn ensure_connected(&mut self, now: Instant) -> Result<(), ClientError> {
        if self.conn.is_some() {
            return Ok(());
        }
        if let (Some(at), Some(e)) = (self.retry_at, &self.last_failure)
            && now < at
        {
            return Err(e.clone());
        }
        match self.connect() {
            Ok(()) => {
                self.retry_at = None;
                self.backoff = BACKOFF_MIN;
                self.last_failure = None;
                self.touch(now);
                Ok(())
            }
            Err(e) => {
                self.retry_at = Some(now + self.backoff);
                self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
                self.last_failure = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Forget the backoff, so the next [`ensure_connected`] tries at once
    /// (the user just asked for manual control).
    ///
    /// [`ensure_connected`]: HelperClient::ensure_connected
    pub fn retry_now(&mut self) {
        self.retry_at = None;
    }

    fn connect(&mut self) -> Result<(), ClientError> {
        let stream = UnixStream::connect(&self.paths.socket).map_err(|e| self.connect_error(&e))?;
        let setup = |s: &UnixStream| {
            s.set_read_timeout(Some(IO_TIMEOUT))?;
            s.set_write_timeout(Some(IO_TIMEOUT))?;
            s.try_clone()
        };
        let writer = setup(&stream).map_err(|e| ClientError::Broken(e.to_string()))?;
        self.conn = Some(Conn {
            reader: BufReader::new(stream),
            writer,
        });
        match self.exchange(Request::Hello(protocol::VERSION)) {
            Ok(Response::Hello { version, fans }) if version == protocol::VERSION => {
                self.fans = fans;
                log::info!("Connected to the fan helper ({fans} fans)");
                Ok(())
            }
            Ok(other) => {
                self.disconnect();
                Err(ClientError::Broken(format!(
                    "unexpected HELLO reply: {other}"
                )))
            }
            Err(e) => {
                self.disconnect();
                // The helper turns non-admins away with an ERR before HELLO.
                Err(match e {
                    ClientError::Refused(m) if m.contains("admin") => ClientError::PermissionDenied,
                    e => e,
                })
            }
        }
    }

    fn connect_error(&self, e: &std::io::Error) -> ClientError {
        match e.kind() {
            ErrorKind::PermissionDenied => ClientError::PermissionDenied,
            ErrorKind::NotFound
                if self
                    .paths
                    .plist
                    .as_deref()
                    .is_some_and(|p| !Path::new(p).exists()) =>
            {
                ClientError::NotInstalled
            }
            ErrorKind::NotFound | ErrorKind::ConnectionRefused => ClientError::NotRunning,
            _ => ClientError::Broken(e.to_string()),
        }
    }

    /// One request, one response line. A broken connection is dropped.
    fn exchange(&mut self, req: Request) -> Result<Response, ClientError> {
        let Some(conn) = self.conn.as_mut() else {
            return Err(ClientError::NotRunning);
        };
        let result = (|| {
            writeln!(conn.writer, "{req}")?;
            let mut line = String::new();
            if conn.reader.read_line(&mut line)? == 0 {
                return Err(std::io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "the helper closed the connection",
                ));
            }
            Ok(line)
        })();
        let line = match result {
            Ok(line) => line,
            Err(e) => {
                self.disconnect();
                return Err(ClientError::Broken(e.to_string()));
            }
        };
        self.touch(Instant::now());
        match Response::parse(&line) {
            Ok(Response::Err(msg)) => Err(ClientError::Refused(msg)),
            Ok(resp) => Ok(resp),
            Err(e) => {
                self.disconnect();
                Err(ClientError::Broken(e.0))
            }
        }
    }

    /// Sets a fan's speed; the helper answers with the speed it applied
    /// (clamped to the fan's range).
    pub fn set(&mut self, fan: u8, rpm: f32) -> Result<f32, ClientError> {
        match self.exchange(Request::Set { fan, rpm })? {
            Response::Applied(rpm) => Ok(rpm),
            other => Err(ClientError::Broken(format!(
                "unexpected SET reply: {other}"
            ))),
        }
    }

    pub fn auto(&mut self, fans: Fans) -> Result<(), ClientError> {
        self.expect_ok(Request::Auto(fans))
    }

    pub fn ping(&mut self) -> Result<(), ClientError> {
        self.expect_ok(Request::Ping)
    }

    fn expect_ok(&mut self, req: Request) -> Result<(), ClientError> {
        match self.exchange(req)? {
            Response::Ok => Ok(()),
            other => Err(ClientError::Broken(format!("unexpected reply: {other}"))),
        }
    }

    /// Records a request at `now`, for [`keep_alive`]. Requests record
    /// themselves; this lets a caller with its own clock say when.
    ///
    /// [`keep_alive`]: HelperClient::keep_alive
    pub fn touch(&mut self, now: Instant) {
        self.last_request = Some(self.last_request.map_or(now, |t| t.max(now)));
    }

    /// PINGs if nothing was sent for [`protocol::PING_INTERVAL`], so the
    /// helper doesn't count us idle and let go of the fans.
    pub fn keep_alive(&mut self, now: Instant) -> Result<(), ClientError> {
        let due = self
            .last_request
            .is_none_or(|t| now.duration_since(t) >= protocol::PING_INTERVAL);
        if self.conn.is_some() && due {
            self.ping()?;
        }
        Ok(())
    }

    /// Closing the connection makes the helper return any fans it holds
    /// for us to automatic.
    pub fn disconnect(&mut self) {
        self.conn = None;
    }

    /// Whether the helper looks usable without connecting: for the UI
    /// while nothing needs it.
    pub fn probe(&mut self, now: Instant) -> Result<(), ClientError> {
        if self.conn.is_some() {
            return Ok(());
        }
        let r = self.ensure_connected(now);
        self.disconnect();
        r
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex};

    /// A stand-in helper on its own socket: answers each line with
    /// `answer` and records what it got. One connection at a time.
    pub(crate) struct FakeHelper {
        pub path: PathBuf,
        pub lines: Arc<Mutex<Vec<String>>>,
        _dir: tempfile::TempDir,
    }

    impl FakeHelper {
        pub(crate) fn start(answer: impl Fn(&str) -> Option<String> + Send + 'static) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("helper.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let lines = Arc::new(Mutex::new(Vec::new()));
            let log = lines.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { return };
                    let mut writer = stream.try_clone().unwrap();
                    for line in BufReader::new(stream).lines() {
                        let Ok(line) = line else { break };
                        log.lock().unwrap().push(line.clone());
                        match answer(&line) {
                            Some(reply) => {
                                if writeln!(writer, "{reply}").is_err() {
                                    break;
                                }
                            }
                            // Hang up.
                            None => break,
                        }
                    }
                    log.lock().unwrap().push("<closed>".into());
                }
            });
            Self {
                path,
                lines,
                _dir: dir,
            }
        }

        /// Answers like the real helper with two fans, clamping to
        /// 1000..5000.
        pub(crate) fn two_fans() -> Self {
            Self::start(|line| {
                Some(match Request::parse(line) {
                    Ok(Request::Hello(1)) => "OK 1 2".into(),
                    Ok(Request::Set { rpm, .. }) => format!("OK {}", rpm.clamp(1000.0, 5000.0)),
                    Ok(Request::Auto(_) | Request::Ping) => "OK".into(),
                    Ok(Request::Hello(_)) => "ERR unsupported protocol version".into(),
                    Err(e) => format!("ERR {e}"),
                })
            })
        }

        pub(crate) fn paths(&self) -> HelperPaths {
            HelperPaths {
                socket: self.path.clone(),
                plist: None,
            }
        }

        /// The lines received so far, once `n` have arrived.
        pub(crate) fn wait_for(&self, n: usize) -> Vec<String> {
            for _ in 0..200 {
                let lines = self.lines.lock().unwrap().clone();
                if lines.len() >= n {
                    return lines;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            self.lines.lock().unwrap().clone()
        }
    }

    #[test]
    fn talks_to_the_helper() {
        let h = FakeHelper::two_fans();
        let mut c = HelperClient::new(h.paths());
        let now = Instant::now();
        c.ensure_connected(now).unwrap();
        assert_eq!(c.fan_count(), 2);
        assert_eq!(c.set(0, 9000.0), Ok(5000.0));
        c.auto(Fans::One(0)).unwrap();
        c.auto(Fans::All).unwrap();
        // Just sent something: no ping yet; after the interval, one.
        c.keep_alive(Instant::now()).unwrap();
        c.keep_alive(Instant::now() + protocol::PING_INTERVAL)
            .unwrap();
        c.disconnect();
        assert_eq!(
            h.wait_for(6),
            [
                "HELLO 1",
                "SET 0 9000",
                "AUTO 0",
                "AUTO ALL",
                "PING",
                "<closed>"
            ]
        );
    }

    #[test]
    fn helper_errors_come_back_as_refusals() {
        let h = FakeHelper::start(|line| {
            Some(if line.starts_with("HELLO") {
                "OK 1 2".into()
            } else {
                "ERR the SMC didn't take manual mode".into()
            })
        });
        let mut c = HelperClient::new(h.paths());
        c.ensure_connected(Instant::now()).unwrap();
        assert_eq!(
            c.set(1, 3000.0),
            Err(ClientError::Refused(
                "the SMC didn't take manual mode".into()
            ))
        );
        // Still connected after an ERR.
        assert!(c.is_connected());
    }

    #[test]
    fn non_admins_are_told_so() {
        let h = FakeHelper::start(|_| Some("ERR not an admin user".into()));
        let mut c = HelperClient::new(h.paths());
        assert_eq!(
            c.ensure_connected(Instant::now()),
            Err(ClientError::PermissionDenied)
        );
        assert!(!c.is_connected());
    }

    #[test]
    fn missing_helper_is_not_installed_or_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("none.sock");
        let plist = dir.path().join("helper.plist");
        let mut c = HelperClient::new(HelperPaths {
            socket: socket.clone(),
            plist: Some(plist.clone()),
        });
        let t = Instant::now();
        assert_eq!(c.ensure_connected(t), Err(ClientError::NotInstalled));
        std::fs::write(&plist, "").unwrap();
        // Backing off: the old answer, without trying.
        assert_eq!(c.ensure_connected(t), Err(ClientError::NotInstalled));
        assert_eq!(
            c.ensure_connected(t + BACKOFF_MIN),
            Err(ClientError::NotRunning)
        );
        // The backoff doubled: still waiting at +1 s after that failure.
        let t2 = t + BACKOFF_MIN;
        assert_eq!(c.retry_at, Some(t2 + 2 * BACKOFF_MIN));
        // A stale socket file with nobody listening.
        drop(UnixListener::bind(&socket).unwrap());
        c.retry_now();
        assert_eq!(c.ensure_connected(t2), Err(ClientError::NotRunning));
    }

    #[test]
    fn a_hangup_drops_the_connection_and_reconnects() {
        let h = FakeHelper::start(|line| (line != "PING").then(|| "OK 1 2".to_string()));
        let mut c = HelperClient::new(h.paths());
        let t = Instant::now();
        c.ensure_connected(t).unwrap();
        assert!(matches!(c.ping(), Err(ClientError::Broken(_))));
        assert!(!c.is_connected());
        // The fake takes the next connection.
        c.ensure_connected(t).unwrap();
        assert!(c.is_connected());
    }

    #[test]
    fn probe_leaves_no_connection() {
        let h = FakeHelper::two_fans();
        let mut c = HelperClient::new(h.paths());
        c.probe(Instant::now()).unwrap();
        assert!(!c.is_connected());
        assert_eq!(h.wait_for(2), ["HELLO 1", "<closed>"]);
    }
}
