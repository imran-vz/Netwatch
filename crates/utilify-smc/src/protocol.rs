//! The fan helper's wire protocol: UTF-8 lines over a Unix stream socket,
//! one request line answered by one response line.
//!
//! ```text
//! HELLO 1          -> OK 1 <fan count>
//! SET <fan> <rpm>  -> OK <applied rpm>     (clamped to the fan's min..max)
//! AUTO <fan>       -> OK
//! AUTO ALL         -> OK
//! PING             -> OK
//! anything else    -> ERR <message>
//! ```
//!
//! A line longer than [`MAX_LINE`] bytes closes the connection. The helper
//! returns the fans to automatic when the last client goes, or when a client
//! sends nothing for [`IDLE_TIMEOUT`]; clients holding manual control PING
//! every [`PING_INTERVAL`].

use std::fmt;
use std::time::Duration;

pub const VERSION: u32 = 1;
pub const SOCKET_PATH: &str = "/var/run/dev.utilify.fanhelper.sock";
/// The longest request line, not counting the `\n`.
pub const MAX_LINE: usize = 256;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(15);
pub const PING_INTERVAL: Duration = Duration::from_secs(5);
/// How an ERR reply starts when the SMC hasn't taken a SET yet: the
/// system is still handing the fans over. The helper keeps the fan (and
/// `Ftst`) for the client, whose next SET carries on.
pub const BUSY: &str = "busy:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fans {
    One(u8),
    All,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Request {
    Hello(u32),
    Set { fan: u8, rpm: f32 },
    Auto(Fans),
    Ping,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    /// `OK <version> <fans>`, the answer to HELLO.
    Hello { version: u32, fans: u8 },
    /// `OK <rpm>`, the answer to SET.
    Applied(f32),
    /// `OK`.
    Ok,
    /// `ERR <message>`.
    Err(String),
}

/// Why a line didn't parse; the message goes back as `ERR <message>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

fn err(msg: impl Into<String>) -> ParseError {
    ParseError(msg.into())
}

fn trim_eol(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

fn parse_fan(s: &str) -> Result<u8, ParseError> {
    match s.parse::<u8>() {
        Ok(n) if n <= 9 => Ok(n),
        _ => Err(err(format!("bad fan: {s}"))),
    }
}

fn parse_rpm(s: &str) -> Result<f32, ParseError> {
    match s.parse::<f32>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(err(format!("bad rpm: {s}"))),
    }
}

impl Request {
    pub fn parse(line: &str) -> Result<Request, ParseError> {
        let words: Vec<&str> = trim_eol(line).split(' ').collect();
        match words.as_slice() {
            ["HELLO", v] => v
                .parse()
                .map(Request::Hello)
                .map_err(|_| err(format!("bad version: {v}"))),
            ["SET", fan, rpm] => Ok(Request::Set {
                fan: parse_fan(fan)?,
                rpm: parse_rpm(rpm)?,
            }),
            ["AUTO", "ALL"] => Ok(Request::Auto(Fans::All)),
            ["AUTO", fan] => Ok(Request::Auto(Fans::One(parse_fan(fan)?))),
            ["PING"] => Ok(Request::Ping),
            [cmd, ..] if ["HELLO", "SET", "AUTO", "PING"].contains(cmd) => {
                Err(err(format!("wrong arguments for {cmd}")))
            }
            _ => Err(err("unknown request")),
        }
    }
}

impl fmt::Display for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Request::Hello(v) => write!(f, "HELLO {v}"),
            Request::Set { fan, rpm } => write!(f, "SET {fan} {rpm}"),
            Request::Auto(Fans::One(fan)) => write!(f, "AUTO {fan}"),
            Request::Auto(Fans::All) => write!(f, "AUTO ALL"),
            Request::Ping => write!(f, "PING"),
        }
    }
}

impl Response {
    /// Responses are told apart by shape: `OK` has no, one or two values.
    pub fn parse(line: &str) -> Result<Response, ParseError> {
        let line = trim_eol(line);
        if line == "ERR" {
            return Ok(Response::Err(String::new()));
        }
        if let Some(msg) = line.strip_prefix("ERR ") {
            return Ok(Response::Err(msg.to_string()));
        }
        let words: Vec<&str> = line.split(' ').collect();
        match words.as_slice() {
            ["OK"] => Ok(Response::Ok),
            ["OK", rpm] => parse_rpm(rpm).map(Response::Applied),
            ["OK", version, fans] => Ok(Response::Hello {
                version: version
                    .parse()
                    .map_err(|_| err(format!("bad version: {version}")))?,
                fans: fans
                    .parse()
                    .map_err(|_| err(format!("bad fan count: {fans}")))?,
            }),
            _ => Err(err(format!("bad response: {line}"))),
        }
    }
}

impl fmt::Display for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Response::Hello { version, fans } => write!(f, "OK {version} {fans}"),
            Response::Applied(rpm) => write!(f, "OK {rpm}"),
            Response::Ok => write!(f, "OK"),
            // Keep the reply on one line.
            Response::Err(msg) => write!(f, "ERR {}", msg.replace(['\n', '\r'], " ")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requests() {
        assert_eq!(Request::parse("HELLO 1\n"), Ok(Request::Hello(1)));
        assert_eq!(
            Request::parse("SET 0 2500"),
            Ok(Request::Set {
                fan: 0,
                rpm: 2500.0
            })
        );
        assert_eq!(
            Request::parse("SET 1 2500.5\r\n"),
            Ok(Request::Set {
                fan: 1,
                rpm: 2500.5
            })
        );
        assert_eq!(Request::parse("AUTO 1"), Ok(Request::Auto(Fans::One(1))));
        assert_eq!(Request::parse("AUTO ALL"), Ok(Request::Auto(Fans::All)));
        assert_eq!(Request::parse("PING"), Ok(Request::Ping));
    }

    #[test]
    fn rejects_malformed_requests() {
        for line in [
            "",
            "ping",
            "PING 1",
            "HELLO",
            "HELLO x",
            "SET 0",
            "SET 0 fast",
            "SET 0 NaN",
            "SET 0 inf",
            "SET -1 2000",
            "SET 10 2000",
            "SET 0  2000",
            "AUTO",
            "AUTO all",
            "WRITE F0Md 1",
            "SET 0 2000 extra",
        ] {
            assert!(Request::parse(line).is_err(), "{line:?} parsed");
        }
    }

    #[test]
    fn requests_round_trip() {
        for req in [
            Request::Hello(1),
            Request::Set {
                fan: 2,
                rpm: 3150.25,
            },
            Request::Auto(Fans::One(0)),
            Request::Auto(Fans::All),
            Request::Ping,
        ] {
            assert_eq!(Request::parse(&req.to_string()), Ok(req));
        }
    }

    #[test]
    fn parses_responses() {
        assert_eq!(
            Response::parse("OK 1 2\n"),
            Ok(Response::Hello {
                version: 1,
                fans: 2
            })
        );
        assert_eq!(Response::parse("OK 4900"), Ok(Response::Applied(4900.0)));
        assert_eq!(Response::parse("OK"), Ok(Response::Ok));
        assert_eq!(
            Response::parse("ERR not allowed"),
            Ok(Response::Err("not allowed".into()))
        );
        assert_eq!(Response::parse("ERR"), Ok(Response::Err(String::new())));
        assert!(Response::parse("NOPE").is_err());
        assert!(Response::parse("OK x").is_err());
        assert!(Response::parse("OK 1 2 3").is_err());
    }

    #[test]
    fn responses_round_trip() {
        for resp in [
            Response::Hello {
                version: 1,
                fans: 2,
            },
            Response::Applied(1234.5),
            Response::Ok,
            Response::Err("SMC write needs root".into()),
        ] {
            assert_eq!(Response::parse(&resp.to_string()), Ok(resp));
        }
        assert_eq!(Response::Err("a\nb".into()).to_string(), "ERR a b");
    }
}
