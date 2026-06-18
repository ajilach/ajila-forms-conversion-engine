//! AEM CRX Package Manager HTTP client (AEM 6.5, HTTP basic auth).
//!
//! Uploads a generated FileVault package to an AEM instance and installs it via
//! the Package Manager service API:
//!
//! - upload:  `POST {host}/crx/packmgr/service/.json/?cmd=upload` (multipart)
//! - install: `POST {host}/crx/packmgr/service/.json{path}?cmd=install`
//!
//! Networking is desktop-only; the wasm build provides a stub that errors.

use blueprint::AemConnection;

/// Upload and install a FileVault package on the configured AEM instance.
///
/// `zip` is the raw package bytes, `package_name` becomes the uploaded file
/// name (`{package_name}.zip`). Returns `Ok(())` on a successful install, or an
/// `Err` carrying the CRX message / HTTP status on failure.
#[cfg(not(target_arch = "wasm32"))]
pub async fn upload_and_install_package(
    conn: &AemConnection,
    zip: Vec<u8>,
    package_name: &str,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let host = conn.host.trim_end_matches('/');

    // ── Upload ──────────────────────────────────────────────────────────
    let file_name = format!("{package_name}.zip");
    let part = reqwest::multipart::Part::bytes(zip)
        .file_name(file_name)
        .mime_str("application/zip")
        .map_err(|e| format!("Failed to build upload part: {e}"))?;
    let form = reqwest::multipart::Form::new()
        .part("package", part)
        .text("force", "true");

    let upload_url = format!("{host}/crx/packmgr/service/.json/?cmd=upload");
    let resp = client
        .post(&upload_url)
        .basic_auth(&conn.username, Some(&conn.password))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("AEM upload request failed: {e}"))?;

    let path = parse_crx_response(resp, "upload")
        .await?
        .ok_or_else(|| "AEM upload succeeded but returned no package path".to_string())?;

    // ── Install ─────────────────────────────────────────────────────────
    let install_url = format!("{host}/crx/packmgr/service/.json{path}?cmd=install");
    let resp = client
        .post(&install_url)
        .basic_auth(&conn.username, Some(&conn.password))
        .send()
        .await
        .map_err(|e| format!("AEM install request failed: {e}"))?;

    parse_crx_response(resp, "install").await?;
    Ok(())
}

/// Parse a CRX Package Manager `.json` response.
///
/// Returns the package `path` (present on upload) on success. CRX returns an
/// HTML login page on auth failure, so a non-JSON body or a non-success status
/// is surfaced as a clear error.
#[cfg(not(target_arch = "wasm32"))]
async fn parse_crx_response(
    resp: reqwest::Response,
    action: &str,
) -> Result<Option<String>, String> {
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("AEM {action} response read failed: {e}"))?;

    let json: serde_json::Value = serde_json::from_str(&body).map_err(|_| {
        if status.as_u16() == 401 || status.as_u16() == 403 {
            format!("AEM {action} failed: authentication rejected (HTTP {status})")
        } else {
            let snippet: String = body.chars().take(200).collect();
            format!("AEM {action} failed (HTTP {status}): {snippet}")
        }
    })?;

    if json["success"].as_bool() == Some(true) {
        return Ok(json["path"].as_str().map(String::from));
    }

    let msg = json["msg"].as_str().unwrap_or("unknown error");
    Err(format!("AEM {action} failed: {msg}"))
}

#[cfg(target_arch = "wasm32")]
pub async fn upload_and_install_package(
    _conn: &AemConnection,
    _zip: Vec<u8>,
    _package_name: &str,
) -> Result<(), String> {
    Err("AEM upload is only supported in the desktop app.".to_string())
}
