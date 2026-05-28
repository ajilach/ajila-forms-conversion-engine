//! Local HTTP server for live HTML preview with auto-reload.
//!
//! Starts a background tokio task that serves the current HTML preview on a local port.
//! Includes a small injected script that polls for version changes and reloads automatically.

use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::settings::AppSettings;

static HTML_CONTENT: std::sync::OnceLock<Arc<RwLock<String>>> = std::sync::OnceLock::new();
static VERSION: AtomicU64 = AtomicU64::new(0);
static SERVER_STARTED: AtomicBool = AtomicBool::new(false);
static ACTIVE: AtomicBool = AtomicBool::new(false);

fn content_store() -> &'static Arc<RwLock<String>> {
    HTML_CONTENT.get_or_init(|| Arc::new(RwLock::new(String::new())))
}

fn port() -> u16 {
    AppSettings::load().live_preview_port
}

/// Start the live preview: sets the initial HTML, starts the server, and opens the browser.
pub fn start_preview(html: String) {
    {
        let mut store = content_store().write().unwrap();
        *store = html;
    }
    VERSION.fetch_add(1, Ordering::SeqCst);
    ACTIVE.store(true, Ordering::SeqCst);

    if !SERVER_STARTED.swap(true, Ordering::SeqCst) {
        let p = port();
        tokio::spawn(run_server(p));
    }

    open_browser();
}

/// Stop the live preview (server keeps running but serves a closed page).
pub fn stop_preview() {
    ACTIVE.store(false, Ordering::SeqCst);
    VERSION.fetch_add(1, Ordering::SeqCst);
}

/// Push an HTML update to the live preview. No-op if preview is not active or content unchanged.
pub fn update_preview(html: String) {
    if !ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    let mut store = content_store().write().unwrap();
    if *store == html {
        return;
    }
    *store = html;
    drop(store);
    VERSION.fetch_add(1, Ordering::SeqCst);
}

/// Returns whether the live preview is currently active.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}
pub fn preview_url() -> String {
    format!("http://localhost:{}", port())
}

fn open_browser() {
    let url = preview_url();
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", &url])
            .spawn();
    }
}

const RELOAD_SCRIPT: &str = r#"<script>
(function() {
    let currentVersion = null;
    async function checkVersion() {
        try {
            const resp = await fetch('/version');
            const v = await resp.text();
            if (currentVersion === null) {
                currentVersion = v;
            } else if (v !== currentVersion) {
                location.reload();
            }
        } catch(e) {}
    }
    setInterval(checkVersion, 500);
})();
</script>"#;

async fn run_server(port: u16) {
    let listener = match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Preview server failed to bind on port {port}: {e}");
            SERVER_STARTED.store(false, Ordering::SeqCst);
            return;
        }
    };

    println!("✓ Live preview server running at http://localhost:{port}");

    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(_) => continue,
        };

        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let n = match stream.read(&mut buf).await {
                Ok(n) if n > 0 => n,
                _ => return,
            };

            let request = String::from_utf8_lossy(&buf[..n]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");

            let response = match path {
                "/version" => {
                    let v = VERSION.load(Ordering::SeqCst);
                    let body = v.to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
                        body.len(),
                        body
                    )
                }
                _ => {
                    let body = if !ACTIVE.load(Ordering::SeqCst) {
                        format!(
                            "<html><body style=\"font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;color:#666\"><p>Live preview stopped.</p>{RELOAD_SCRIPT}</body></html>"
                        )
                    } else {
                        let html = content_store().read().unwrap().clone();
                        // Inject the reload script before </body> or at the end
                        if let Some(pos) = html.rfind("</body>") {
                            format!("{}{}{}", &html[..pos], RELOAD_SCRIPT, &html[pos..])
                        } else {
                            format!("{html}{RELOAD_SCRIPT}")
                        }
                    };
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-cache\r\n\r\n{}",
                        body.len(),
                        body
                    )
                }
            };

            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.flush().await;
        });
    }
}
