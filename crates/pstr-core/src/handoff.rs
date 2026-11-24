//! Adding a share from a phone, for a device with no keyboard worth typing a
//! share link on — a TV.
//!
//! The TV listens on its local network address at a one-time path and shows
//! that address as a QR code. The phone's camera opens it in a browser, which
//! gets a small form; the link pasted there is handed to the TV as an
//! [`Offer`], and the TV answers it with whether the share opened. The page
//! shows that answer, so a wrong password is fixed on the phone rather than
//! found out on the TV.
//!
//! The phone needs nothing installed, which is the point: many viewers have
//! nothing but the link someone sent them.
//!
//! ## What protects it
//!
//! - **The path is the key.** Twelve characters from a 32-letter alphabet,
//!   60 bits, drawn fresh each time. Anything else gets a 404, and the
//!   listener is bound to the one address shown, not every interface.
//! - **It does not last.** It answers for [`LIFETIME`] and stops when the
//!   front end closes it, which it does once a share is added.
//! - **The page runs no script** and its CSP forbids any, so there is no
//!   script to subvert.
//!
//! It is plain HTTP: a TV cannot hold a certificate a phone trusts for a LAN
//! address. The link crosses the home network unencrypted by this layer, the
//! same as a smart-TV remote app; the user guide says so.

use crate::error::{Error, Result};
use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};

/// How long a code answers. Long enough to find the phone and the link, short
/// enough that a code photographed off the screen is soon worth nothing.
pub const LIFETIME: Duration = Duration::from_secs(10 * 60);

/// Letters a person can read back off a TV: no `l`, `o`, `0` or `1`.
const ALPHABET: &[u8; 32] = b"abcdefghijkmnpqrstuvwxyz23456789";
const TOKEN_LENGTH: usize = 12;

/// A request head is a request line and a few headers; anything longer is
/// not a browser posting this form.
const HEAD_LIMIT: usize = 8 * 1024;
/// A share link, a name and a password, form-encoded, with room to spare.
const BODY_LIMIT: usize = 16 * 1024;
/// A browser sends a request at once. One that opens a connection and says
/// nothing (a speculative preconnect) is let go after this.
const READ_TIMEOUT: Duration = Duration::from_secs(20);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the phone's page waits for the TV to open the link: a handshake,
/// and a second one when the link turns out to need a password.
const REPLY_TIMEOUT: Duration = Duration::from_secs(120);

/// A listening code. Dropping it stops the listener.
pub struct Handoff {
    url: String,
    deadline: Instant,
    offers: tokio::sync::Mutex<mpsc::Receiver<Offer>>,
    closed: watch::Sender<bool>,
    accept: JoinHandle<()>,
}

/// A link a phone sent, waiting on [`Offer::reply`].
pub struct Offer {
    pub name: String,
    pub url: String,
    pub custom_password: Option<String>,
    reply: oneshot::Sender<Option<String>>,
}

/// The QR code for a [`Handoff`]'s address: `width` × `width` modules, row
/// by row, `true` for dark. The front end adds the quiet zone around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Qr {
    pub width: usize,
    pub dark: Vec<bool>,
}

struct Shared {
    token: String,
    deadline: Instant,
    offers: mpsc::Sender<Offer>,
}

impl Handoff {
    /// Listen on `host`, which must be an address the phone can reach — the
    /// TV's own on its home network, not a VPN's.
    ///
    /// Must be called within a tokio runtime: the listener runs on it.
    pub fn start(host: IpAddr) -> Result<Self> {
        Self::start_for(host, LIFETIME)
    }

    fn start_for(host: IpAddr, lifetime: Duration) -> Result<Self> {
        let listener = std::net::TcpListener::bind(SocketAddr::new(host, 0))?;
        listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(listener)?;
        let address = listener.local_addr()?;
        let token = token()?;
        let url = format!("http://{address}/{token}");
        let deadline = Instant::now() + lifetime;
        let (offers, receiver) = mpsc::channel(4);
        let shared = Arc::new(Shared {
            token,
            deadline,
            offers,
        });
        let accept = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, Arc::clone(&shared)));
            }
        });
        Ok(Self {
            url,
            deadline,
            offers: tokio::sync::Mutex::new(receiver),
            closed: watch::Sender::new(false),
            accept,
        })
    }

    /// The address the QR code carries, for showing as text beside it.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The QR code for [`Self::url`].
    pub fn qr(&self) -> Result<Qr> {
        let code =
            qrcode::QrCode::with_error_correction_level(self.url.as_bytes(), qrcode::EcLevel::M)
                .map_err(|error| Error::Config(format!("QR code: {error}")))?;
        Ok(Qr {
            width: code.width(),
            dark: code
                .to_colors()
                .into_iter()
                .map(|color| color == qrcode::Color::Dark)
                .collect(),
        })
    }

    /// The next link a phone sends, or `None` once the code has expired or
    /// been closed.
    pub async fn next(&self) -> Option<Offer> {
        let mut closed = self.closed.subscribe();
        if *closed.borrow_and_update() {
            return None;
        }
        let mut offers = self.offers.lock().await;
        tokio::select! {
            offer = offers.recv() => offer,
            _ = closed.wait_for(|closed| *closed) => None,
            () = tokio::time::sleep_until(self.deadline) => None,
        }
    }

    /// Stop answering. A phone mid-way through a submission still gets its
    /// page; a new one is refused.
    pub fn close(&self) {
        self.accept.abort();
        self.closed.send_replace(true);
    }
}

impl Drop for Handoff {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl fmt::Debug for Handoff {
    // The URL is the key; it is not for logs.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Handoff").finish_non_exhaustive()
    }
}

impl Offer {
    /// Answer the phone: `None` once the share is added, otherwise why it was
    /// not, which the phone shows over the form it sent.
    pub fn reply(self, refusal: Option<String>) {
        // The phone may have given up waiting; nothing to tell it then.
        let _ = self.reply.send(refusal);
    }
}

impl fmt::Debug for Offer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Offer")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

fn token() -> Result<String> {
    let mut bytes = [0_u8; TOKEN_LENGTH];
    getrandom::fill(&mut bytes)
        .map_err(|error| Error::Config(format!("no randomness for a hand-off code: {error}")))?;
    Ok(bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte & 31)]))
        .collect())
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

async fn serve(mut stream: TcpStream, shared: Arc<Shared>) {
    let Ok(Ok(Some(request))) = timeout(READ_TIMEOUT, read_request(&mut stream)).await else {
        return;
    };
    let response = respond(&shared, request).await;
    let _ = timeout(WRITE_TIMEOUT, async {
        stream.write_all(&response).await?;
        stream.shutdown().await
    })
    .await;
}

/// One request, or `None` for something that is not an HTTP request of a
/// size this form sends.
async fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    let head_end = loop {
        if let Some(end) = find(&buffer, b"\r\n\r\n") {
            if end > HEAD_LIMIT {
                return Ok(None);
            }
            break end;
        }
        if buffer.len() > HEAD_LIMIT {
            return Ok(None);
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let Ok(head) = std::str::from_utf8(&buffer[..head_end]) else {
        return Ok(None);
    };
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split(' ');
    let (Some(method), Some(target)) = (request_line.next(), request_line.next()) else {
        return Ok(None);
    };
    let length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse::<usize>())
        .transpose();
    let Ok(length) = length else {
        return Ok(None);
    };
    let length = length.unwrap_or(0);
    if length > BODY_LIMIT {
        return Ok(None);
    }
    let method = method.to_owned();
    let path = target.split('?').next().unwrap_or_default().to_owned();
    let mut body = buffer.split_off(head_end + 4);
    while body.len() < length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(length);
    Ok(Some(Request { method, path, body }))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Compare without stopping at the first difference, so the time a wrong
/// guess takes says nothing about how much of it was right.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0_u8, |difference, (x, y)| difference | (x ^ y))
            == 0
}

async fn respond(shared: &Shared, request: Request) -> Vec<u8> {
    let token = request.path.strip_prefix('/').unwrap_or_default();
    if !same(token, &shared.token) {
        return page(
            "404 Not Found",
            &message("Nothing here", "Scan the code on your TV again."),
        );
    }
    if Instant::now() >= shared.deadline {
        return page(
            "410 Gone",
            &message(
                "This code has expired",
                "Start again on the TV for a new one.",
            ),
        );
    }
    match request.method.as_str() {
        "GET" | "HEAD" => page("200 OK", &form(None, "", "")),
        "POST" => submit(shared, &request.body).await,
        _ => page(
            "405 Method Not Allowed",
            &message("Nothing here", "Scan the code on your TV again."),
        ),
    }
}

async fn submit(shared: &Shared, body: &[u8]) -> Vec<u8> {
    let mut fields = parse_form(body);
    let mut take = |key: &str| fields.remove(key).unwrap_or_default().trim().to_owned();
    let (url, name, password) = (take("link"), take("name"), take("password"));
    if url.is_empty() {
        return page(
            "200 OK",
            &form(Some("Paste the share link first."), &name, ""),
        );
    }
    let (reply, answer) = oneshot::channel();
    let offer = Offer {
        name: name.clone(),
        url: url.clone(),
        custom_password: (!password.is_empty()).then_some(password),
        reply,
    };
    if shared.offers.send(offer).await.is_err() {
        return stopped();
    }
    match timeout(REPLY_TIMEOUT, answer).await {
        Ok(Ok(None)) => page(
            "200 OK",
            &message(
                "Added",
                "Your TV is reading the share now. You can close this page.",
            ),
        ),
        Ok(Ok(Some(refusal))) => page("200 OK", &form(Some(&refusal), &name, &url)),
        _ => stopped(),
    }
}

fn stopped() -> Vec<u8> {
    page(
        "503 Service Unavailable",
        &message(
            "The TV stopped listening",
            "Start again on the TV for a new code.",
        ),
    )
}

/// `application/x-www-form-urlencoded`, which is what a form without an
/// `enctype` posts. A malformed escape is kept as written rather than
/// failing the whole submission.
fn parse_form(body: &[u8]) -> HashMap<String, String> {
    body.split(|&byte| byte == b'&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let mut parts = pair.splitn(2, |&byte| byte == b'=');
            let key = decode(parts.next().unwrap_or_default());
            let value = decode(parts.next().unwrap_or_default());
            (key, value)
        })
        .collect()
}

fn decode(encoded: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        match encoded[index] {
            b'+' => bytes.push(b' '),
            b'%' if let (Some(high), Some(low)) = (
                encoded.get(index + 1).copied().and_then(hex),
                encoded.get(index + 2).copied().and_then(hex),
            ) =>
            {
                bytes.push(high << 4 | low);
                index += 2;
            }
            byte => bytes.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn hex(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn page(status: &str, body: &str) -> Vec<u8> {
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <meta name=\"color-scheme\" content=\"dark light\">\
         <title>Proton Stream</title><style>{STYLE}</style></head>\
         <body><main>{body}</main></body></html>"
    );
    format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Referrer-Policy: no-referrer\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'\r\n\
         Connection: close\r\n\r\n{html}",
        html.len()
    )
    .into_bytes()
}

fn message(heading: &str, text: &str) -> String {
    format!(
        "<h1>{}</h1><p class=\"lead\">{}</p>",
        escape(heading),
        escape(text)
    )
}

fn form(error: Option<&str>, name: &str, url: &str) -> String {
    let error = error
        .map(|error| format!("<p class=\"error\" role=\"alert\">{}</p>", escape(error)))
        .unwrap_or_default();
    format!(
        "<h1>Add a share to your TV</h1>\
         <p class=\"lead\">Paste a Proton Drive share link. Proton Stream on your TV opens it \
         and adds it to the library.</p>{error}\
         <form method=\"post\" autocomplete=\"off\" accept-charset=\"utf-8\">\
         <label>Share link<input name=\"link\" type=\"url\" value=\"{}\" required \
         autocapitalize=\"off\" spellcheck=\"false\" \
         placeholder=\"https://drive.proton.me/urls/…\"></label>\
         <label>Name <span>optional</span><input name=\"name\" value=\"{}\" \
         placeholder=\"The shared folder's name\"></label>\
         <label>Custom password <span>if the link has one</span>\
         <input name=\"password\" type=\"password\"></label>\
         <button type=\"submit\">Add to TV</button></form>\
         <p class=\"note\">This page talks to your TV over your home network. It stops working \
         once a share is added, or after ten minutes.</p>",
        escape(url),
        escape(name),
    )
}

/// The suite's look (`docs/DESIGN-TOKENS.md`), in a page with no assets.
const STYLE: &str = "\
:root{--page:#100E17;--card:#17141F;--field:#0B0910;--line:rgba(255,255,255,.1);\
--text:#F4F2FA;--muted:#AAA4BD;--error:#FF8FA3;color-scheme:dark}\
@media (prefers-color-scheme:light){:root{--page:#FAF9FC;--card:#FFFFFF;--field:#F4F1FA;\
--line:rgba(27,19,64,.14);--text:#1B1340;--muted:#615970;--error:#B3261E;color-scheme:light}}\
*{box-sizing:border-box}\
body{margin:0;min-height:100vh;background:var(--page);color:var(--text);\
font:16px/1.5 Inter,system-ui,-apple-system,'Segoe UI',Roboto,sans-serif;\
display:flex;justify-content:center;padding:24px 16px}\
main{width:100%;max-width:440px;background:var(--card);border:1px solid var(--line);\
border-radius:16px;padding:24px}\
h1{font-size:22px;line-height:1.25;margin:0 0 8px}\
.lead,.note{color:var(--muted);margin:0 0 20px}\
.note{font-size:13px;margin:20px 0 0}\
.error{color:var(--error);font-weight:600;margin:0 0 16px}\
label{display:block;font-weight:600;font-size:14px;margin:0 0 16px}\
label span{color:var(--muted);font-weight:400}\
input{display:block;width:100%;margin-top:6px;padding:12px 14px;font:inherit;\
color:var(--text);background:var(--field);border:1px solid var(--line);border-radius:12px}\
input:focus{outline:2px solid #8A6DFF;outline-offset:1px}\
button{width:100%;margin-top:4px;padding:14px;border:0;border-radius:999px;font:inherit;\
font-weight:600;color:#fff;background:#6D4AFF;cursor:pointer}\
button:focus-visible{outline:2px solid #D69B77;outline-offset:2px}";

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    /// Send `request` raw and read the whole answer.
    async fn ask(handoff: &Handoff, request: String) -> String {
        let address = handoff
            .url()
            .strip_prefix("http://")
            .and_then(|rest| rest.split('/').next())
            .unwrap()
            .to_owned();
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        answer
    }

    fn path(handoff: &Handoff) -> String {
        let rest = handoff.url().strip_prefix("http://").unwrap();
        rest[rest.find('/').unwrap()..].to_owned()
    }

    fn get(path: &str) -> String {
        format!("GET {path} HTTP/1.1\r\nHost: tv\r\n\r\n")
    }

    fn post(path: &str, body: &str) -> String {
        format!(
            "POST {path} HTTP/1.1\r\nHost: tv\r\n\
             Content-Type: application/x-www-form-urlencoded\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn a_phone_gets_the_form_at_the_code_and_nothing_anywhere_else() {
        let handoff = Handoff::start(LOCAL).unwrap();
        let page = ask(&handoff, get(&path(&handoff))).await;
        assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");
        assert!(page.contains("<form method=\"post\""));
        assert!(!page.contains("<script"));

        for elsewhere in ["/", "/favicon.ico", &format!("{}x", path(&handoff))] {
            let page = ask(&handoff, get(elsewhere)).await;
            assert!(page.starts_with("HTTP/1.1 404"), "{elsewhere}: {page}");
            assert!(!page.contains("<form"));
        }
    }

    #[tokio::test]
    async fn a_submitted_link_reaches_the_tv_with_its_name_and_password() {
        let handoff = Arc::new(Handoff::start(LOCAL).unwrap());
        let phone = {
            let handoff = Arc::clone(&handoff);
            tokio::spawn(async move {
                let body = "link=https%3A%2F%2Fdrive.proton.me%2Furls%2FABC%23secret\
                            &name=Anime+%26+films&password=p%2Bw";
                ask(&handoff, post(&path(&handoff), body)).await
            })
        };
        let offer = handoff.next().await.unwrap();
        assert_eq!(offer.url, "https://drive.proton.me/urls/ABC#secret");
        assert_eq!(offer.name, "Anime & films");
        assert_eq!(offer.custom_password.as_deref(), Some("p+w"));
        offer.reply(None);
        let page = phone.await.unwrap();
        assert!(page.contains("<h1>Added</h1>"), "{page}");
    }

    #[tokio::test]
    async fn a_refusal_shows_the_form_again_with_the_reason_and_the_link_kept() {
        let handoff = Arc::new(Handoff::start(LOCAL).unwrap());
        let phone = {
            let handoff = Arc::clone(&handoff);
            tokio::spawn(async move {
                let body = "link=https%3A%2F%2Fdrive.proton.me%2Furls%2FA%22B%23s&name=&password=";
                ask(&handoff, post(&path(&handoff), body)).await
            })
        };
        let offer = handoff.next().await.unwrap();
        assert_eq!(offer.custom_password, None);
        offer.reply(Some("This link needs its <password>".to_owned()));
        let page = phone.await.unwrap();
        assert!(
            page.contains("This link needs its &lt;password&gt;"),
            "{page}"
        );
        assert!(page.contains("value=\"https://drive.proton.me/urls/A&quot;B#s\""));
        assert!(page.contains("<form"));
    }

    #[tokio::test]
    async fn an_empty_link_is_answered_on_the_phone_without_bothering_the_tv() {
        let handoff = Handoff::start(LOCAL).unwrap();
        let page = ask(&handoff, post(&path(&handoff), "link=++&name=Films")).await;
        assert!(page.contains("Paste the share link first."), "{page}");
        assert!(page.contains("value=\"Films\""));
        let waited = timeout(Duration::from_millis(100), handoff.next()).await;
        assert!(waited.is_err(), "the TV was handed an empty link");
    }

    #[tokio::test]
    async fn closing_ends_the_wait_for_a_phone_and_the_listener() {
        let handoff = Arc::new(Handoff::start(LOCAL).unwrap());
        let waiting = {
            let handoff = Arc::clone(&handoff);
            tokio::spawn(async move { handoff.next().await.is_none() })
        };
        tokio::task::yield_now().await;
        handoff.close();
        assert!(waiting.await.unwrap());
        assert!(handoff.next().await.is_none());
        tokio::task::yield_now().await;
        let address = handoff.url().strip_prefix("http://").unwrap();
        let address = address.split('/').next().unwrap();
        assert!(TcpStream::connect(address).await.is_err());
    }

    #[tokio::test]
    async fn an_expired_code_turns_the_phone_away_and_stops_waiting() {
        let handoff = Handoff::start_for(LOCAL, Duration::from_millis(50)).unwrap();
        assert!(handoff.next().await.is_none());
        let page = ask(&handoff, get(&path(&handoff))).await;
        assert!(page.starts_with("HTTP/1.1 410"), "{page}");
        assert!(!page.contains("<form"));
    }

    #[tokio::test]
    async fn a_request_larger_than_a_form_is_dropped_unanswered() {
        let handoff = Handoff::start(LOCAL).unwrap();
        let request = format!(
            "POST {} HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            path(&handoff),
            BODY_LIMIT + 1
        );
        assert_eq!(ask(&handoff, request).await, "");
        let request = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(HEAD_LIMIT));
        assert_eq!(ask(&handoff, request).await, "");
    }

    #[tokio::test]
    async fn the_address_is_the_host_given_and_never_logged() {
        let handoff = Handoff::start(LOCAL).unwrap();
        assert!(handoff.url().starts_with("http://127.0.0.1:"));
        let token = path(&handoff);
        assert_eq!(token.len(), TOKEN_LENGTH + 1);
        assert!(token[1..].bytes().all(|byte| ALPHABET.contains(&byte)));
        assert!(!format!("{handoff:?}").contains(&token[1..]));
    }

    #[tokio::test]
    async fn the_qr_code_is_a_square_of_modules() {
        let handoff = Handoff::start(LOCAL).unwrap();
        let qr = handoff.qr().unwrap();
        // Version 1 is 21 modules; every version adds 4.
        assert!(qr.width >= 21 && (qr.width - 21).is_multiple_of(4));
        assert_eq!(qr.dark.len(), qr.width * qr.width);
        // The finder pattern's top-left corner is dark.
        assert!(qr.dark[0]);
    }

    #[test]
    fn form_fields_decode_escapes_and_plus_signs_and_keep_a_broken_escape() {
        let fields = parse_form(b"a=one+two&b=%E2%9C%93%26&c=100%&d=%zz&=x&e");
        assert_eq!(fields["a"], "one two");
        assert_eq!(fields["b"], "\u{2713}&");
        assert_eq!(fields["c"], "100%");
        assert_eq!(fields["d"], "%zz");
        assert_eq!(fields["e"], "");
    }

    #[test]
    fn a_wrong_token_of_the_right_length_does_not_match() {
        assert!(same("abcdefghijkm", "abcdefghijkm"));
        assert!(!same("abcdefghijkm", "abcdefghijkn"));
        assert!(!same("abc", "abcd"));
    }

    #[test]
    fn tokens_differ_each_time() {
        assert_ne!(token().unwrap(), token().unwrap());
    }
}
