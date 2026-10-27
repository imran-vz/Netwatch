//! The socket side: accept admin users, read request lines, answer them,
//! and drop clients that go quiet. Single-threaded, around `poll(2)`.

use crate::control::{ClientId, Controller};
use crate::log;
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};
use utilify_smc::SmcIo;
use utilify_smc::protocol::{MAX_LINE, Request, Response};

struct Client {
    id: ClientId,
    stream: UnixStream,
    buf: Vec<u8>,
    last_request: Instant,
}

pub struct Server<S> {
    listener: UnixListener,
    controller: Controller<S>,
    clients: Vec<Client>,
    next_id: ClientId,
    idle_timeout: Duration,
    is_admin: Box<dyn Fn(u32) -> bool>,
}

/// Binds the socket at `path`, replacing a stale one, as `root:admin` mode
/// 0660. Fails if another helper is already listening there.
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    if UnixStream::connect(path).is_ok() {
        return Err(io::Error::new(
            ErrorKind::AddrInUse,
            "another helper is listening",
        ));
    }
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    // Owner-only until the group is set, so there's no window where the
    // socket is open to everyone.
    // SAFETY: umask only swaps the process's file mode mask.
    let old = unsafe { libc::umask(0o177) };
    let listener = UnixListener::bind(path);
    unsafe { libc::umask(old) };
    let listener = listener?;
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "socket path has a NUL"))?;
    // SAFETY: a NUL-terminated path.
    if unsafe { libc::chown(c_path.as_ptr(), 0, admin_gid()) } != 0 {
        log!(
            "can't make the socket root:admin ({}); leaving it to its owner",
            io::Error::last_os_error()
        );
    }
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o660))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn admin_gid() -> libc::gid_t {
    // SAFETY: getgrnam returns null or a pointer into static storage, read
    // straight away.
    let gr = unsafe { libc::getgrnam(c"admin".as_ptr()) };
    if gr.is_null() {
        80
    } else {
        unsafe { (*gr).gr_gid }
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn mbr_uid_to_uuid(uid: libc::uid_t, uu: *mut u8) -> libc::c_int;
    fn mbr_gid_to_uuid(gid: libc::gid_t, uu: *mut u8) -> libc::c_int;
    fn mbr_check_membership(
        user: *const u8,
        group: *const u8,
        is_member: *mut libc::c_int,
    ) -> libc::c_int;
}

/// Whether `uid` is root or in the admin group (as Directory Services
/// sees it, nested groups included).
#[cfg(target_os = "macos")]
pub fn is_admin(uid: u32) -> bool {
    if uid == 0 {
        return true;
    }
    let (mut user, mut group, mut member) = ([0u8; 16], [0u8; 16], 0);
    // SAFETY: 16-byte uuid buffers and an int out-parameter.
    unsafe {
        mbr_uid_to_uuid(uid, user.as_mut_ptr()) == 0
            && mbr_gid_to_uuid(admin_gid(), group.as_mut_ptr()) == 0
            && mbr_check_membership(user.as_ptr(), group.as_ptr(), &mut member) == 0
            && member != 0
    }
}

#[cfg(not(target_os = "macos"))]
pub fn is_admin(uid: u32) -> bool {
    uid == 0
}

fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: a live socket and two out-parameters.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

enum Outcome {
    Keep,
    Close(&'static str),
}

impl<S: SmcIo> Server<S> {
    pub fn new(
        listener: UnixListener,
        controller: Controller<S>,
        idle_timeout: Duration,
        is_admin: Box<dyn Fn(u32) -> bool>,
    ) -> Server<S> {
        Server {
            listener,
            controller,
            clients: Vec::new(),
            next_id: 1,
            idle_timeout,
            is_admin,
        }
    }

    #[cfg(test)]
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// Waits up to `max_wait` for something to happen and handles it:
    /// connections, requests, idle clients.
    pub fn step(&mut self, max_wait: Duration) -> io::Result<()> {
        let now = Instant::now();
        let next_idle = self
            .clients
            .iter()
            .map(|c| (c.last_request + self.idle_timeout).saturating_duration_since(now))
            .min()
            .unwrap_or(max_wait);
        let wait = max_wait.min(next_idle);

        let mut fds: Vec<libc::pollfd> = std::iter::once(self.listener.as_raw_fd())
            .chain(self.clients.iter().map(|c| c.stream.as_raw_fd()))
            .map(|fd| libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        // SAFETY: `fds` is a live array of `fds.len()` pollfds.
        let n = unsafe {
            libc::poll(
                fds.as_mut_ptr(),
                fds.len() as libc::nfds_t,
                wait.as_millis() as i32,
            )
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            // A signal: the caller checks why.
            return if e.kind() == ErrorKind::Interrupted {
                Ok(())
            } else {
                Err(e)
            };
        }

        // Clients first, by index, before accepting changes the list.
        let ready: Vec<bool> = fds[1..].iter().map(|p| p.revents != 0).collect();
        let mut closed = Vec::new();
        for (i, ready) in ready.into_iter().enumerate() {
            if ready && let Outcome::Close(why) = self.service(i) {
                closed.push((i, why));
            }
        }
        let now = Instant::now();
        for (i, c) in self.clients.iter().enumerate() {
            if now.duration_since(c.last_request) >= self.idle_timeout
                && !closed.iter().any(|&(j, _)| j == i)
            {
                closed.push((i, "idle"));
            }
        }
        closed.sort_by_key(|&(i, _)| std::cmp::Reverse(i));
        for (i, why) in closed {
            let client = self.clients.remove(i);
            log!("client {} closed ({why})", client.id);
            self.controller.release(client.id);
        }
        if self.clients.is_empty() && !self.controller.held().is_empty() {
            self.controller.restore_all();
        }

        if fds[0].revents != 0 {
            self.accept();
        }
        Ok(())
    }

    fn accept(&mut self) {
        loop {
            let stream = match self.listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(e) => {
                    log!("accept failed: {e}");
                    return;
                }
            };
            let mut stream = stream;
            match peer_uid(&stream) {
                Ok(uid) if (self.is_admin)(uid) => {}
                Ok(uid) => {
                    log!("refused uid {uid}: not an admin");
                    let _ = writeln!(stream, "{}", Response::Err("not an admin user".into()));
                    continue;
                }
                Err(e) => {
                    log!("refused a client: can't identify it ({e})");
                    continue;
                }
            }
            if stream.set_nonblocking(true).is_err() {
                continue;
            }
            let id = self.next_id;
            self.next_id += 1;
            log!("client {id} connected");
            self.clients.push(Client {
                id,
                stream,
                buf: Vec::new(),
                last_request: Instant::now(),
            });
        }
    }

    /// Reads what client `i` sent and answers each complete line.
    fn service(&mut self, i: usize) -> Outcome {
        let mut chunk = [0u8; 512];
        loop {
            match self.clients[i].stream.read(&mut chunk) {
                Ok(0) => return Outcome::Close("disconnected"),
                Ok(n) => self.clients[i].buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Outcome::Close("read error"),
            }
            // Don't buffer a flood: anything past one line's worth without a
            // newline is already over the limit.
            if self.clients[i].buf.len() > 4 * MAX_LINE {
                break;
            }
        }
        while let Some(pos) = self.clients[i].buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.clients[i].buf.drain(..=pos).collect();
            let line = &line[..line.len() - 1];
            if line.len() > MAX_LINE {
                return Outcome::Close("line too long");
            }
            let now = Instant::now();
            self.clients[i].last_request = now;
            let response = match std::str::from_utf8(line).map(Request::parse) {
                Ok(Ok(req)) => self.controller.handle(self.clients[i].id, req, now),
                Ok(Err(e)) => Response::Err(e.0),
                Err(_) => Response::Err("not UTF-8".into()),
            };
            if writeln!(self.clients[i].stream, "{response}").is_err() {
                return Outcome::Close("write error");
            }
        }
        if self.clients[i].buf.len() > MAX_LINE {
            return Outcome::Close("line too long");
        }
        Outcome::Keep
    }

    /// Every fan back to automatic and every client dropped.
    pub fn shutdown(&mut self) {
        self.clients.clear();
        self.controller.restore_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Config;
    use std::io::{BufRead, BufReader};
    use std::path::PathBuf;
    use utilify_smc::Key;
    use utilify_smc::fake::FakeSmc;

    struct Harness<'a> {
        server: Server<&'a FakeSmc>,
        path: PathBuf,
    }

    impl Drop for Harness<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn harness<'a>(smc: &'a FakeSmc, name: &str, idle: Duration, admin: bool) -> Harness<'a> {
        let path = std::env::temp_dir().join(format!("fh-{name}-{}.sock", std::process::id()));
        let listener = bind(&path).unwrap();
        let config = Config {
            mode_retry_delay: Duration::ZERO,
            ..Config::default()
        };
        let controller = Controller::new(smc, config, None);
        Harness {
            server: Server::new(listener, controller, idle, Box::new(move |_| admin)),
            path,
        }
    }

    struct Conn(BufReader<UnixStream>);

    impl Harness<'_> {
        fn connect(&mut self) -> Conn {
            let stream = UnixStream::connect(&self.path).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            self.server.step(Duration::from_millis(50)).unwrap();
            Conn(BufReader::new(stream))
        }

        fn ask(&mut self, conn: &mut Conn, line: impl AsRef<[u8]>) -> String {
            conn.0.get_mut().write_all(line.as_ref()).unwrap();
            self.server.step(Duration::from_millis(200)).unwrap();
            read_line(conn)
        }
    }

    fn read_line(conn: &mut Conn) -> String {
        let mut out = String::new();
        conn.0.read_line(&mut out).unwrap();
        out.trim_end().to_string()
    }

    fn fake() -> FakeSmc {
        FakeSmc::apple_silicon(&[(1350.0, 5777.0), (1350.0, 5777.0)])
    }

    fn md(smc: &FakeSmc, fan: u8) -> Option<f64> {
        smc.get_f64(Key::new(&format!("F{fan}Md")).unwrap())
    }

    #[test]
    fn socket_is_0660() {
        use std::os::unix::fs::PermissionsExt;
        let smc = fake();
        let h = harness(&smc, "mode", Duration::from_secs(15), true);
        let mode = std::fs::metadata(&h.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o660);
        // A second helper on the same socket refuses to start.
        assert!(bind(&h.path).is_err());
    }

    #[test]
    fn answers_requests() {
        let smc = fake();
        let mut h = harness(&smc, "answers", Duration::from_secs(15), true);
        let mut c = h.connect();
        assert_eq!(h.ask(&mut c, "HELLO 1\n"), "OK 1 2");
        assert_eq!(h.ask(&mut c, "PING\n"), "OK");
        assert_eq!(h.ask(&mut c, "SET 0 9000\n"), "OK 5777");
        assert_eq!(h.ask(&mut c, "AUTO ALL\n"), "OK");
        assert_eq!(md(&smc, 0), Some(0.0));
    }

    #[test]
    fn malformed_lines_get_err_and_long_lines_close() {
        let smc = fake();
        let mut h = harness(&smc, "malformed", Duration::from_secs(15), true);
        let mut c = h.connect();
        assert!(h.ask(&mut c, "WRITE F0Mx 1\n").starts_with("ERR"));
        assert!(h.ask(&mut c, "SET 0 lots\n").starts_with("ERR"));
        assert!(h.ask(&mut c, b"\xff\n").starts_with("ERR"));
        assert_eq!(h.server.client_count(), 1);
        let long = format!("PING{}\n", " ".repeat(300));
        c.0.get_mut().write_all(long.as_bytes()).unwrap();
        h.server.step(Duration::from_millis(200)).unwrap();
        assert_eq!(h.server.client_count(), 0);
        // Closed without an answer.
        assert_eq!(read_line(&mut c), "");
    }

    #[test]
    fn restores_on_last_disconnect() {
        let smc = fake();
        let mut h = harness(&smc, "disconnect", Duration::from_secs(15), true);
        let mut a = h.connect();
        let mut b = h.connect();
        assert_eq!(h.ask(&mut a, "SET 0 3000\n"), "OK 3000");
        assert_eq!(h.ask(&mut b, "SET 1 3000\n"), "OK 3000");
        drop(a);
        h.server.step(Duration::from_millis(200)).unwrap();
        assert_eq!(md(&smc, 0), Some(0.0));
        assert_eq!(md(&smc, 1), Some(1.0));
        drop(b);
        h.server.step(Duration::from_millis(200)).unwrap();
        assert_eq!(md(&smc, 1), Some(0.0));
        assert_eq!(smc.get_f64(utilify_smc::FAN_FORCE), Some(0.0));
    }

    #[test]
    fn idle_clients_are_dropped_and_fans_restored() {
        let smc = fake();
        let mut h = harness(&smc, "idle", Duration::from_millis(300), true);
        let mut c = h.connect();
        assert_eq!(h.ask(&mut c, "SET 0 3000\n"), "OK 3000");
        // Pings keep it alive past the timeout...
        for _ in 0..3 {
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(h.ask(&mut c, "PING\n"), "OK");
        }
        assert_eq!(md(&smc, 0), Some(1.0));
        // ...silence doesn't.
        let start = Instant::now();
        while h.server.client_count() > 0 && start.elapsed() < Duration::from_secs(2) {
            h.server.step(Duration::from_millis(100)).unwrap();
        }
        assert_eq!(h.server.client_count(), 0);
        assert_eq!(md(&smc, 0), Some(0.0));
    }

    #[test]
    fn refuses_non_admins() {
        let smc = fake();
        let mut h = harness(&smc, "nonadmin", Duration::from_secs(15), false);
        let mut c = h.connect();
        assert_eq!(read_line(&mut c), "ERR not an admin user");
        assert_eq!(h.server.client_count(), 0);
        assert!(smc.writes().is_empty());
    }

    #[test]
    fn this_user_is_checked_against_admin() {
        // Root always passes; nobody (-2) never does.
        assert!(is_admin(0));
        assert!(!is_admin(u32::MAX - 1));
    }
}
