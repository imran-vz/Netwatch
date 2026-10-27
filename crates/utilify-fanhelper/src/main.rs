//! `utilify-fanhelper`: the root LaunchDaemon that lets Utilify set fan
//! speeds. SMC writes need root; this process holds that privilege and
//! nothing else, takes requests from admin users over a Unix socket (see
//! `utilify_smc::protocol`), only ever writes `Ftst`, `F{n}Md` and `F{n}Tg`,
//! and puts the fans back to automatic whenever its clients go away.
//!
//! ```text
//! utilify-fanhelper [--socket <path>] [--no-root-check] [--fake]
//! ```
//!
//! `--no-root-check` and `--fake` (an in-memory two-fan SMC) are for
//! running it as a normal user while testing.

mod control;
mod server;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use utilify_smc::protocol::{IDLE_TIMEOUT, SOCKET_PATH};
use utilify_smc::{Smc, SmcIo};

/// Logs a line to stderr, which launchd sends to
/// /var/log/utilify-fanhelper.log.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        eprintln!("{} {}", $crate::timestamp(), format_args!($($arg)*))
    };
}

/// Local time as `2026-10-22 14:03:07`.
pub fn timestamp() -> String {
    // SAFETY: time and localtime_r fill the locals passed.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return t.to_string();
        }
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: the handler only stores to an atomic. No SA_RESTART, so
        // poll returns early and the loop sees STOP.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as *const () as usize;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

struct Args {
    socket: PathBuf,
    root_check: bool,
    fake: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        socket: SOCKET_PATH.into(),
        root_check: true,
        fake: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--socket" => args.socket = it.next().ok_or("--socket needs a path")?.into(),
            "--no-root-check" => args.root_check = false,
            "--fake" => args.fake = true,
            "-h" | "--help" => {
                println!("usage: utilify-fanhelper [--socket <path>] [--no-root-check] [--fake]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

/// Where the held fans are recorded for crash recovery: next to the socket.
fn record_path(socket: &std::path::Path) -> PathBuf {
    socket.with_extension("state")
}

fn run<S: SmcIo>(smc: S, args: &Args) -> std::io::Result<()> {
    let controller = control::Controller::new(
        smc,
        control::Config::default(),
        Some(record_path(&args.socket)),
    );
    log!("{} fans", controller.fans());
    let listener = server::bind(&args.socket)?;
    log!("listening on {}", args.socket.display());
    let mut server = server::Server::new(
        listener,
        controller,
        IDLE_TIMEOUT,
        Box::new(server::is_admin),
    );
    let result = loop {
        if STOP.load(Ordering::SeqCst) {
            log!("stopping");
            break Ok(());
        }
        if let Err(e) = server.step(Duration::from_secs(1)) {
            break Err(e);
        }
    };
    server.shutdown();
    let _ = std::fs::remove_file(&args.socket);
    result
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("utilify-fanhelper: {e}");
            std::process::exit(2);
        }
    };
    // SAFETY: geteuid has no preconditions.
    if args.root_check && unsafe { libc::geteuid() } != 0 {
        eprintln!("utilify-fanhelper: must run as root (or pass --no-root-check to test)");
        std::process::exit(1);
    }
    install_signal_handlers();
    log!("utilify-fanhelper {} starting", env!("CARGO_PKG_VERSION"));
    let result = if args.fake {
        log!("using a fake SMC");
        run(
            utilify_smc::fake::FakeSmc::apple_silicon(&[(1350.0, 5777.0), (1350.0, 5777.0)]),
            &args,
        )
    } else {
        match Smc::open() {
            Ok(smc) => run(smc, &args),
            Err(e) => {
                log!("can't open the SMC: {e}");
                std::process::exit(1);
            }
        }
    };
    if let Err(e) = result {
        log!("error: {e}");
        std::process::exit(1);
    }
}
