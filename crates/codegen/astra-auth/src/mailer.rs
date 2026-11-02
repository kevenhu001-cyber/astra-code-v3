//! Verification-email delivery. `ConsoleMailer` prints the link to the server
//! log (the default for local/self-hosted); `SMTPMailer` sends via SMTP with
//! optional HTTP CONNECT proxy support (mirrors the Go implementation, which
//! uses the blocking `net/smtp` package).
//!
//! Configuration comes from the environment:
//! `SMTP_HOST` / `SMTP_PORT` / `SMTP_USER` / `SMTP_PASS` / `SMTP_FROM` /
//! `SMTP_PROXY` / `SMTP_SECURE`.
//!
//! `SMTP_FROM` accepts both a bare address (`user@host`) and a display-name
//! form (`Astra <user@host>`). The display name is used for the `From:` header
//! only — the SMTP envelope (`MAIL FROM`) always carries the bare address, as
//! required by RFC 5321 §4.1.1.2.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use tracing::info;

/// Deliver a verification email for a registration.
pub trait Mailer: Send + Sync {
    fn send_verification(&self, email: &str, verify_url: &str) -> Result<(), String>;
}

/// Print the verification link to the server log.
pub struct ConsoleMailer;

impl Mailer for ConsoleMailer {
    fn send_verification(&self, email: &str, verify_url: &str) -> Result<(), String> {
        info!(target: "astra-auth::mail", to = %email, "Verify your Astra account: {verify_url}");
        Ok(())
    }
}

/// SMTP delivery configured from the environment:
/// SMTP_HOST / SMTP_PORT / SMTP_USER / SMTP_PASS / SMTP_FROM / SMTP_PROXY /
/// SMTP_SECURE.
#[derive(Clone)]
pub struct SMTPMailer {
    host: String,
    port: String,
    user: String,
    pass: String,
    from: String,
    /// Optional display name for the `From:` header (e.g. `Astra`).
    /// Kept separate from `from` so the address stays a bare, unquoted
    /// address in the environment file.
    from_name: Option<String>,
    proxy: Option<String>,
    /// Implicit TLS (SMTPS) — connect with TLS before the SMTP greeting.
    secure: bool,
}

impl SMTPMailer {
    pub fn from_env() -> Self {
        let host = getenv("SMTP_HOST");
        let port = getenv("SMTP_PORT");
        // Fall back to the SMTP account so the envelope sender always matches
        // the authenticated identity — some relays reject a MAIL FROM that
        // differs from the authenticated user.
        let from = match getenv("SMTP_FROM") {
            v if !v.is_empty() => v,
            _ => {
                let user = getenv("SMTP_USER");
                if user.is_empty() {
                    "no-reply@astra.dev".to_string()
                } else {
                    user
                }
            }
        };
        let secure = port.trim() == "465" || is_truthy(&getenv("SMTP_SECURE"));
        SMTPMailer {
            host,
            port,
            user: getenv("SMTP_USER"),
            pass: getenv("SMTP_PASS"),
            from,
            from_name: optenv("SMTP_FROM_NAME"),
            proxy: optenv("SMTP_PROXY"),
            secure,
        }
    }

    pub fn enabled(&self) -> bool {
        !self.host.is_empty()
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    fn addr(&self) -> String {
        let port = if self.port.is_empty() {
            "25"
        } else {
            &self.port
        };
        format!("{}:{port}", self.host)
    }
}

impl Mailer for SMTPMailer {
    fn send_verification(&self, email: &str, verify_url: &str) -> Result<(), String> {
        let (envelope_from, from_header) = match &self.from_name {
            Some(name) if !name.is_empty() => {
                let (addr, _) = split_address(&self.from);
                let header = format!("{name} <{addr}>");
                (addr, header)
            }
            _ => split_address(&self.from),
        };
        let subject = "Verify your Astra account";
        let body = format!(
            "Verify your Astra account\r\n\r\nOpen this link to activate your account:\r\n{verify_url}\r\n\r\nIf you did not create an account, you can ignore this email.\r\n"
        );
        let msg = build_message(&from_header, email, subject, &body);

        let stream = self.dial()?;
        let (mut conn, caps) = SmtpConn::open(stream, &self.host, self.secure)?;

        conn.auth(&caps, &self.user, &self.pass)?;

        // Propagate SMTP-level rejections: a silently-ignored "550 no such
        // user" would report a successful send to the caller.
        let resp = conn.command(&format!("MAIL FROM:<{envelope_from}>"))?;
        if !resp.starts_with("250") {
            return Err(format!("smtp MAIL FROM rejected: {resp}"));
        }
        let resp = conn.command(&format!("RCPT TO:<{email}>"))?;
        if !resp.starts_with("250") && !resp.starts_with("251") {
            return Err(format!("smtp RCPT TO rejected: {resp}"));
        }
        let resp = conn.command("DATA")?;
        if !resp.starts_with("354") {
            return Err(format!("smtp DATA rejected: {resp}"));
        }
        let resp = conn.send_data(&msg)?;
        if !resp.starts_with("250") {
            return Err(format!("smtp send rejected: {resp}"));
        }
        let _ = conn.command("QUIT");
        info!(
            target: "astra-auth::mail",
            to = %email,
            from = %from_header,
            "verification email sent"
        );
        Ok(())
    }
}

impl SMTPMailer {
    fn dial(&self) -> Result<Box<dyn ReadWrite>, String> {
        let addr = self.addr();
        match &self.proxy {
            Some(proxy) => connect_via_proxy(proxy, &addr),
            None => {
                let stream = TcpStream::connect(&addr).map_err(|e| format!("smtp dial: {e}"))?;
                stream.set_nodelay(true).ok();
                Ok(Box::new(stream))
            }
        }
    }
}

/// A stream usable for plain or TLS-wrapped SMTP.
pub trait ReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> ReadWrite for T {}

/// Minimal blocking SMTP connection with reply-line parsing. SMTP is strictly
/// request-response and every reply is consumed in full (multi-line replies
/// end at the "NNN " line), so a fresh `BufReader` per command never loses
/// data; the STARTTLS handshake happens only after the 220 reply, when the
/// server sends nothing unsolicited.
struct SmtpConn {
    rw: Box<dyn ReadWrite>,
}

impl SmtpConn {
    /// Greet the server and complete the TLS handshake if needed.
    ///
    /// Returns the connection plus the EHLO capability list. For STARTTLS the
    /// EHLO is repeated afterwards because RFC 3207 §4 requires discarding
    /// everything learned before the handshake.
    fn open(raw: Box<dyn ReadWrite>, host: &str, secure: bool) -> Result<(Self, String), String> {
        if secure {
            // Implicit TLS: wrap the tunnel before the SMTP hello.
            let stream = tls_stream(raw, host)?;
            let mut conn = SmtpConn {
                rw: Box::new(stream),
            };
            let caps = conn.ehlo(host)?;
            Ok((conn, caps))
        } else {
            // Plain SMTP hello first, then STARTTLS.
            let mut conn = SmtpConn { rw: raw };
            let _ = conn.command(&format!("EHLO {host}"))?;
            let resp = conn.command("STARTTLS")?;
            if !resp.starts_with("220") {
                return Err(format!("smtp STARTTLS: {resp}"));
            }
            let stream = tls_stream(conn.rw, host)?;
            let mut conn = SmtpConn {
                rw: Box::new(stream),
            };
            let caps = conn.ehlo(host)?;
            Ok((conn, caps))
        }
    }

    fn ehlo(&mut self, host: &str) -> Result<String, String> {
        self.command(&format!("EHLO {host}"))
    }

    /// Authenticate with the mechanism the server actually advertises.
    ///
    /// `AUTH PLAIN` is preferred when offered; otherwise `AUTH LOGIN` is
    /// used. Both are widely supported by hosted SMTP relays.
    fn auth(&mut self, caps: &str, user: &str, pass: &str) -> Result<(), String> {
        if user.is_empty() {
            return Ok(());
        }
        let mechs = auth_mechanisms(caps);
        let prefer_plain = mechs.iter().any(|m| m == "PLAIN") || mechs.is_empty();

        if prefer_plain {
            let resp = self.command("AUTH PLAIN")?;
            if resp.starts_with("235") {
                return Ok(());
            }
            if resp.starts_with("334") {
                let creds = format!("\0{user}\0{pass}");
                let resp = self.command(&base64_encode(creds.as_bytes()))?;
                if resp.starts_with("235") {
                    return Ok(());
                }
                return Err(format!("smtp auth: {resp}"));
            }
            // Server refused AUTH PLAIN — fall through to AUTH LOGIN.
        }

        let resp = self.command("AUTH LOGIN")?;
        if !resp.starts_with("334") {
            return Err(format!("smtp AUTH LOGIN: {resp}"));
        }
        let resp = self.command(&base64_encode(user.as_bytes()))?;
        if !resp.starts_with("334") {
            return Err(format!("smtp AUTH LOGIN username: {resp}"));
        }
        let resp = self.command(&base64_encode(pass.as_bytes()))?;
        if !resp.starts_with("235") {
            return Err(format!("smtp auth failed: {resp}"));
        }
        Ok(())
    }

    fn command(&mut self, cmd: &str) -> Result<String, String> {
        self.rw
            .write_all(format!("{cmd}\r\n").as_bytes())
            .map_err(|e| format!("smtp write: {e}"))?;
        self.read_reply()
    }

    /// Send the message body and the terminating dot.
    ///
    /// Dot-stuffs leading dots (RFC 5321 §4.5.2) and ends the data with
    /// exactly one `.<CRLF>` — appending another CRLF would be read by the
    /// server as a spurious empty command.
    fn send_data(&mut self, msg: &str) -> Result<String, String> {
        let mut body = String::with_capacity(msg.len() + 16);
        for line in msg.split('\n') {
            let line = line.trim_end_matches('\r');
            if line == "." {
                body.push_str("..");
            } else {
                body.push_str(line);
            }
            body.push_str("\r\n");
        }
        body.push_str(".\r\n");
        self.rw
            .write_all(body.as_bytes())
            .map_err(|e| format!("smtp write: {e}"))?;
        self.rw.flush().map_err(|e| format!("smtp flush: {e}"))?;
        self.read_reply()
    }

    /// Read a (possibly multi-line) SMTP reply.
    ///
    /// Every line of the reply is collected — an EHLO greeting lists its
    /// capabilities on the `250-` continuation lines, and the final `250 `
    /// line alone does not contain them. A final reply line starts with
    /// "NNN " (space); continuation lines start with "NNN-".
    fn read_reply(&mut self) -> Result<String, String> {
        let mut reader = BufReader::new(&mut self.rw);
        let mut collected = String::new();
        let mut line = String::new();
        loop {
            line.clear();
            reader
                .read_line(&mut line)
                .map_err(|e| format!("smtp read: {e}"))?;
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                return Err("smtp read: connection closed".to_string());
            }
            if !collected.is_empty() {
                collected.push('\n');
            }
            collected.push_str(trimmed);
            if trimmed.len() >= 4 && trimmed.as_bytes()[3] == b' ' {
                return Ok(collected);
            }
        }
    }
}

/// Known SMTP AUTH mechanisms. A capability line may carry further keywords
/// after the mechanism list (`AUTH LOGIN PLAIN SIZE 10485760`), so candidates
/// are filtered against this set rather than taken verbatim.
const AUTH_MECHANISMS: &[&str] = &[
    "LOGIN",
    "PLAIN",
    "CRAM-MD5",
    "EXTERNAL",
    "NTLM",
    "GSSAPI",
    "XOAUTH2",
    "XOAUTH2-RAW",
    "OAUTHBEARER",
];

/// Extract the AUTH mechanisms advertised in an EHLO capability list.
///
/// Handles the multiline form (`250-AUTH LOGIN PLAIN`) as well as a single
/// line (`250 AUTH LOGIN PLAIN`).
fn auth_mechanisms(caps: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in caps.lines() {
        let line = raw.trim();
        let rest = if let Some(r) = line.strip_prefix("250-") {
            r
        } else if let Some(r) = line.strip_prefix("250 ") {
            r
        } else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        if parts.next().is_some_and(|k| k.eq_ignore_ascii_case("AUTH")) {
            out.extend(
                parts
                    .map(|m| m.to_ascii_uppercase())
                    .filter(|m| AUTH_MECHANISMS.contains(&m.as_str())),
            );
        }
    }
    out
}

/// Split a configured `From` value into the bare envelope address and the
/// header value to emit.
///
/// `user@host` -> (`user@host`, `user@host`)
/// `Astra <user@host>` -> (`user@host`, `Astra <user@host>`)
fn split_address(value: &str) -> (String, String) {
    let raw = value.trim();
    if let Some(open) = raw.rfind('<')
        && let Some(offset) = raw[open..].find('>')
    {
        let close = open + offset;
        let addr = raw[open + 1..close].trim();
        if !addr.is_empty() {
            let name = raw[..open].trim().trim_matches('"').trim();
            let header = if name.is_empty() {
                addr.to_string()
            } else {
                format!("{name} <{addr}>")
            };
            return (addr.to_string(), header);
        }
    }
    (raw.to_string(), raw.to_string())
}

/// Assemble the RFC 5322 message. CRLF line endings throughout.
fn build_message(from: &str, to: &str, subject: &str, body: &str) -> String {
    let date = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S %z");
    let msg_id = message_id(from);
    format!(
        "From: {from}\r\n\
         To: {to}\r\n\
         Date: {date}\r\n\
         Message-ID: {msg_id}\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=UTF-8\r\n\
         \r\n\
         {body}"
    )
}

/// A globally unique Message-ID, using the sender's domain.
fn message_id(from: &str) -> String {
    let domain = from
        .rsplit('@')
        .next()
        .filter(|d| !d.is_empty())
        .unwrap_or("astra.local");
    let unique = crate::password::random_hex(12);
    format!("<{unique}@{domain}>")
}

fn tls_stream(
    raw: Box<dyn ReadWrite>,
    host: &str,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, Box<dyn ReadWrite>>, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // `builder_with_provider` (not `builder`) so no process-level default
    // provider install is required; the ring provider is enabled via the
    // crate's `ring` feature.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("smtp tls config: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| format!("smtp tls name: {e}"))?;
    let conn = rustls::ClientConnection::new(Arc::new(config), name)
        .map_err(|e| format!("smtp tls: {e}"))?;
    Ok(rustls::StreamOwned::new(conn, raw))
}

fn connect_via_proxy(proxy: &str, addr: &str) -> Result<Box<dyn ReadWrite>, String> {
    let (scheme, rest) = proxy.split_once("://").unwrap_or(("http", proxy));
    if scheme != "http" && scheme != "https" {
        return Err(format!("unsupported proxy scheme {scheme:?}"));
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !p.contains('/') => (h.to_string(), p.to_string()),
        _ => (rest.to_string(), "8080".to_string()),
    };
    let mut stream =
        TcpStream::connect(format!("{host}:{port}")).map_err(|e| format!("proxy dial: {e}"))?;
    stream.set_nodelay(true).ok();
    let req =
        format!("CONNECT {addr} HTTP/1.1\r\nHost: {addr}\r\nProxy-Connection: Keep-Alive\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("proxy write: {e}"))?;
    stream.flush().ok();

    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|e| format!("proxy clone: {e}"))?,
    );
    // Status line, then headers up to the blank line.
    let mut status = String::new();
    reader
        .read_line(&mut status)
        .map_err(|e| format!("proxy read: {e}"))?;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("proxy read: {e}"))?;
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
    }
    if !status.contains(" 200 ") {
        return Err(format!("proxy connect failed: {}", status.trim_end()));
    }
    // The BufReader may have buffered bytes beyond the headers (e.g. the TLS
    // ServerHello on the implicit-TLS path), so keep it in the read path like
    // Go's bufferedConn.
    Ok(Box::new(BufferedConn { stream, reader }))
}

/// A `TcpStream` whose reads go through a `BufReader` (used after a proxy
/// CONNECT so buffered upstream bytes are not lost).
struct BufferedConn {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Read for BufferedConn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf)
    }
}

impl Write for BufferedConn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn getenv(k: &str) -> String {
    std::env::var(k).unwrap_or_default()
}

fn optenv(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

/// Interpret the usual truthy spellings of a boolean env var.
fn is_truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_address_has_no_display_name() {
        let (addr, header) = split_address("user@host");
        assert_eq!(addr, "user@host");
        assert_eq!(header, "user@host");
    }

    #[test]
    fn display_name_is_split_from_envelope_address() {
        let (addr, header) = split_address("Astra <noreply@topodrive.top>");
        assert_eq!(addr, "noreply@topodrive.top");
        assert_eq!(header, "Astra <noreply@topodrive.top>");
    }

    #[test]
    fn quoted_display_name_is_accepted() {
        let (addr, header) = split_address("\"Astra Code\" <noreply@topodrive.top>");
        assert_eq!(addr, "noreply@topodrive.top");
        assert_eq!(header, "Astra Code <noreply@topodrive.top>");
    }

    #[test]
    fn malformed_angle_bracket_falls_back_to_raw_value() {
        let (addr, header) = split_address("  user@host  ");
        assert_eq!(addr, "user@host");
        assert_eq!(header, "user@host");
    }

    #[test]
    fn auth_mechanisms_are_read_from_multiline_greeting() {
        let caps = "250-mail.example.com\n250-PIPELINING\n250-AUTH LOGIN PLAIN\n250 SIZE 10485760";
        assert_eq!(auth_mechanisms(caps), vec!["LOGIN", "PLAIN"]);
    }

    #[test]
    fn auth_mechanisms_are_read_from_single_line_greeting() {
        assert_eq!(
            auth_mechanisms("250 AUTH LOGIN PLAIN SIZE 10485760"),
            vec!["LOGIN", "PLAIN"]
        );
    }

    #[test]
    fn auth_mechanisms_absent_yields_empty_list() {
        assert!(auth_mechanisms("250-mail.example.com\n250 HELP").is_empty());
    }

    #[test]
    fn truthy_spellings() {
        assert!(is_truthy("true"));
        assert!(is_truthy("TRUE"));
        assert!(is_truthy("1"));
        assert!(is_truthy("yes"));
        assert!(is_truthy("on"));
        assert!(!is_truthy("false"));
        assert!(!is_truthy("0"));
        assert!(!is_truthy(""));
    }

    #[test]
    fn message_uses_crlf_and_carries_required_headers() {
        let msg = build_message(
            "Astra <noreply@topodrive.top>",
            "user@example.com",
            "Verify your Astra account",
            "Hello\r\n",
        );
        assert!(msg.starts_with("From: Astra <noreply@topodrive.top>\r\n"));
        assert!(msg.contains("\r\nTo: user@example.com\r\n"));
        assert!(msg.contains("\r\nDate: "));
        assert!(msg.contains("\r\nMessage-ID: <"));
        assert!(msg.contains("\r\nSubject: Verify your Astra account\r\n"));
        assert!(msg.contains("\r\n\r\n"));
        assert!(!msg.contains("\n\n"));
    }

    #[test]
    fn message_id_uses_sender_domain() {
        let id = message_id("noreply@topodrive.top");
        assert!(id.starts_with('<'));
        assert!(id.ends_with("@topodrive.top>"));
    }
}
