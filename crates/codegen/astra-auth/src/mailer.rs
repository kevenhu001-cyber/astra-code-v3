//! Verification-email delivery. `ConsoleMailer` prints the link to the server
//! log (the default for local/self-hosted); `SMTPMailer` sends via SMTP with
//! optional HTTP CONNECT proxy support (mirrors the Go implementation, which
//! uses the blocking `net/smtp` package).

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
/// SMTP_HOST / SMTP_PORT / SMTP_USER / SMTP_PASS / SMTP_FROM / SMTP_PROXY.
#[derive(Clone)]
pub struct SMTPMailer {
    host: String,
    port: String,
    user: String,
    pass: String,
    from: String,
    proxy: Option<String>,
}

impl SMTPMailer {
    pub fn from_env() -> Self {
        SMTPMailer {
            host: getenv("SMTP_HOST"),
            port: getenv("SMTP_PORT"),
            user: getenv("SMTP_USER"),
            pass: getenv("SMTP_PASS"),
            from: getenv("SMTP_FROM"),
            proxy: optenv("SMTP_PROXY"),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.host.is_empty()
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    fn addr(&self) -> String {
        let port = if self.port.is_empty() { "25" } else { &self.port };
        format!("{}:{port}", self.host)
    }
}

impl Mailer for SMTPMailer {
    fn send_verification(&self, email: &str, verify_url: &str) -> Result<(), String> {
        let from = if self.from.is_empty() {
            "no-reply@astra.dev".to_string()
        } else {
            self.from.clone()
        };
        let subject = "Verify your Astra account";
        let body = format!(
            "Verify your Astra account\r\n\r\nOpen this link to activate your account:\r\n{verify_url}\r\n\r\nIf you did not create an account, you can ignore this email.\r\n"
        );
        let msg = format!(
            "From: {from}\r\nTo: {email}\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\n{body}"
        );

        let stream = self.dial()?;
        let mut conn = SmtpConn::new(stream, &self.host, &self.port)?;

        let _ = conn.command(&format!("EHLO {}", self.host))?;

        if !self.user.is_empty() {
            let resp = conn.command("AUTH PLAIN")?;
            if resp.starts_with("334") {
                let auth = format!("\0{}\0{}", self.user, self.pass);
                let _ = conn.command(&base64_encode(auth.as_bytes()))?;
            } else {
                // Fall back to AUTH LOGIN.
                let _ = conn.command("AUTH LOGIN")?;
                let _ = conn.command(&base64_encode(self.user.as_bytes()))?;
                let _ = conn.command(&base64_encode(self.pass.as_bytes()))?;
            }
        }

        let _ = conn.command(&format!("MAIL FROM:<{from}>"))?;
        let _ = conn.command(&format!("RCPT TO:<{email}>"))?;
        let _ = conn.command("DATA")?;
        let _ = conn.command(&format!("{msg}\r\n."))?;
        let _ = conn.command("QUIT").ok();
        Ok(())
    }
}

impl SMTPMailer {
    fn dial(&self) -> Result<Box<dyn ReadWrite>, String> {
        let addr = self.addr();
        match &self.proxy {
            Some(proxy) => connect_via_proxy(proxy, &addr),
            None => {
                let stream =
                    TcpStream::connect(&addr).map_err(|e| format!("smtp dial: {e}"))?;
                stream.set_nodelay(true).ok();
                Ok(Box::new(stream))
            }
        }
    }
}

/// A stream usable for plain or TLS-wrapped SMTP.
pub trait ReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> ReadWrite for T {}

/// Minimal blocking SMTP connection with reply-line parsing.
struct SmtpConn {
    rw: Box<dyn ReadWrite>,
}

impl SmtpConn {
    fn new(mut raw: Box<dyn ReadWrite>, host: &str, port: &str) -> Result<Self, String> {
        if port == "465" {
            // Implicit TLS: wrap the tunnel before SMTP hello.
            let stream = tls_stream(raw, host)?;
            Ok(SmtpConn {
                rw: Box::new(stream),
            })
        } else {
            // Plain SMTP hello first, then STARTTLS.
            let mut conn = SmtpConn { rw: raw };
            let _ = conn.command(&format!("EHLO {host}"))?;
            let resp = conn.command("STARTTLS")?;
            if !resp.starts_with("220") {
                return Err(format!("smtp STARTTLS: {resp}"));
            }
            let stream = tls_stream(conn.rw, host)?;
            Ok(SmtpConn {
                rw: Box::new(stream),
            })
        }
    }

    fn command(&mut self, cmd: &str) -> Result<String, String> {
        self.rw
            .write_all(format!("{cmd}\r\n").as_bytes())
            .map_err(|e| format!("smtp write: {e}"))?;
        self.read_reply()
    }

    /// Read a (possibly multi-line) SMTP reply. A final reply line starts
    /// with "NNN " (space); continuation lines start with "NNN-".
    fn read_reply(&mut self) -> Result<String, String> {
        let mut reader = BufReader::new(&mut self.rw);
        let mut line = String::new();
        loop {
            line.clear();
            reader
                .read_line(&mut line)
                .map_err(|e| format!("smtp read: {e}"))?;
            let trimmed = line.trim_end();
            if trimmed.len() >= 4 && trimmed.as_bytes()[3] == b' ' {
                return Ok(trimmed.to_string());
            }
            if trimmed.is_empty() {
                return Err("smtp read: connection closed".to_string());
            }
        }
    }
}

fn tls_stream(raw: Box<dyn ReadWrite>, host: &str) -> Result<rustls::StreamOwned<rustls::ClientConnection, Box<dyn ReadWrite>>, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
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
    let req = format!(
        "CONNECT {addr} HTTP/1.1\r\nHost: {addr}\r\nProxy-Connection: Keep-Alive\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("proxy write: {e}"))?;
    stream.flush().ok();

    let mut reader = BufReader::new(stream.try_clone().map_err(|e| format!("proxy clone: {e}"))?);
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
