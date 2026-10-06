//! Proton's CAPTCHA, in a window of its own.
//!
//! Proton puts a CAPTCHA in front of a sign-in it does not recognise — a new
//! network, a VPN exit. The page reports success by posting a message to its
//! host rather than by redirecting, so the system browser has nowhere to send
//! it back to: it needs a webview this app controls, as on Android and in
//! `proton-drive-linux`.
//!
//! The webview runs in a child process — this same executable, started with
//! [`ARG`] — because its windowing library and eframe's cannot both own the
//! event loop of one process. The child prints the solved token on stdout and
//! exits; closing its window exits with nothing printed, which is a cancel.

use std::io::{Read as _, Write as _};

/// The argument that starts this executable as the verification window,
/// followed by the page's URL.
pub const ARG: &str = "--human-verification";

/// Forwards every message the page posts to this process. The page targets
/// `window.parent`, which for a top-level page is the page itself, so a
/// listener on its own window hears it.
const BRIDGE: &str = r#"
window.addEventListener('message', function (event) {
    try {
        window.ipc.postMessage(typeof event.data === 'string' ? event.data : JSON.stringify(event.data));
    } catch (e) {}
});
"#;

/// Run the verification window if this process was started for it, returning
/// the process's exit code; `None` when it was started as the app.
pub fn run_if_requested() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(ARG) {
        return None;
    }
    let Some(url) = args.next().filter(|url| is_proton_page(url)) else {
        eprintln!("{ARG} needs a verification page on proton.me");
        return Some(2);
    };
    Some(match window(&url) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("the verification window failed: {error}");
            1
        }
    })
}

/// Present `url` in the verification window and wait for the token it yields.
///
/// `None` when the viewer closed the window, or it could not be shown.
/// Blocking: call it off the UI thread and outside the async runtime.
pub fn solve(url: &str) -> Option<String> {
    let exe = std::env::current_exe()
        .inspect_err(|error| tracing::warn!("locate this executable: {error}"))
        .ok()?;
    let mut child = std::process::Command::new(exe)
        .arg(ARG)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .inspect_err(|error| tracing::warn!("open the verification window: {error}"))
        .ok()?;
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    let status = child.wait().ok()?;
    if !status.success() {
        tracing::warn!("the verification window exited with {status}");
        return None;
    }
    out.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_owned)
}

fn window(url: &str) -> anyhow::Result<()> {
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;
    use wry::{NewWindowResponse, WebViewBuilder};

    let event_loop = EventLoopBuilder::<String>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("Confirm you are human — Proton Stream")
        .with_inner_size(tao::dpi::LogicalSize::new(440.0, 680.0))
        .build(&event_loop)?;

    let proxy = event_loop.create_proxy();
    let builder = WebViewBuilder::new()
        .with_url(url)
        .with_initialization_script(BRIDGE)
        .with_ipc_handler(move |request| {
            // Every message the page posts lands here, most of them its own
            // chatter; only a completion carries a token.
            if let Some(token) = pstr_core::account::verification_token(request.body()) {
                let _ = proxy.send_event(token);
            }
        })
        // The bridge above is reachable by whatever the page shows, so the
        // page stays on Proton.
        .with_navigation_handler(|url| is_allowed_navigation(&url))
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny);

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _webview = {
        use tao::platform::unix::WindowExtUnix as _;
        use wry::WebViewBuilderExtUnix as _;
        let vbox = window
            .default_vbox()
            .ok_or_else(|| anyhow::anyhow!("the window has no GTK container"))?;
        builder.build_gtk(vbox)?
    };
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let _webview = builder.build(&window)?;

    let mut answered = false;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(token) if !answered => {
                answered = true;
                let mut stdout = std::io::stdout().lock();
                let _ = writeln!(stdout, "{token}");
                let _ = stdout.flush();
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    })
}

/// Whether `url` is a page of Proton's own, over HTTPS.
fn is_proton_page(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // A user-info part would let `proton.me@evil.example` pass.
    if authority.contains('@') {
        return false;
    }
    let host = authority.split(':').next().unwrap_or_default();
    host == "proton.me" || host.ends_with(".proton.me")
}

/// Whether the webview may go to `url`: Proton's pages, and the documents a
/// page makes for itself, which have no origin of their own to leave to.
fn is_allowed_navigation(url: &str) -> bool {
    is_proton_page(url) || url == "about:blank" || url.starts_with("about:srcdoc")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_proton_pages_over_https_are_proton_pages() {
        assert!(is_proton_page(
            "https://verify.proton.me/?embed=1&methods=captcha&token=x"
        ));
        assert!(is_proton_page("https://proton.me"));
        assert!(!is_proton_page("http://verify.proton.me/"));
        assert!(!is_proton_page("https://proton.me.evil.example/"));
        assert!(!is_proton_page("https://evilproton.me/"));
        assert!(!is_proton_page("https://proton.me@evil.example/"));
        assert!(!is_proton_page(
            "https://evil.example/?next=https://proton.me"
        ));
    }

    #[test]
    fn the_window_stays_on_proton() {
        assert!(is_allowed_navigation(
            "https://verify-api.proton.me/core/v4/captcha"
        ));
        assert!(is_allowed_navigation("about:blank"));
        assert!(!is_allowed_navigation("https://example.com/"));
        assert!(!is_allowed_navigation("file:///etc/passwd"));
    }
}
