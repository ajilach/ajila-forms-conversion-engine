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

/// Fetch the rendered Adaptive Form HTML from AEM for verification.
///
/// `form_jcr_path` is the form's JCR node path (e.g.
/// `/content/forms/af/<form_path>/<form_dir>`); the form renders at that path
/// with an `.html` extension. Returns the HTML body.
#[cfg(not(target_arch = "wasm32"))]
pub async fn fetch_form_html(conn: &AemConnection, form_jcr_path: &str) -> Result<String, String> {
    let host = conn.host.trim_end_matches('/');
    let path = form_jcr_path.trim_end_matches('/');
    let url = format!("{host}{path}.html");
    let resp = reqwest::Client::new()
        .get(&url)
        .basic_auth(&conn.username, Some(&conn.password))
        .send()
        .await
        .map_err(|e| format!("AEM form fetch failed: {e}"))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("AEM form fetch read failed: {e}"))?;
    if !status.is_success() {
        let snippet: String = body.chars().take(200).collect();
        return Err(format!("AEM form fetch failed (HTTP {status}): {snippet}"));
    }
    Ok(body)
}

/// Fetch the Document-of-Record (DoR) PDF for a deployed form from AEM.
///
/// Uses the Adaptive Form guide-container DoR selector
/// (`{form}/jcr:content/guideContainer.af.dor.pdf`). Returns the raw PDF bytes;
/// errors (with a body snippet) if the response isn't a PDF — e.g. DoR isn't
/// configured for the form. (Confirm the exact selector against your AEM
/// version's DoR docs if this 404s.)
#[cfg(not(target_arch = "wasm32"))]
pub async fn fetch_dor_pdf(conn: &AemConnection, form_jcr_path: &str) -> Result<Vec<u8>, String> {
    let host = conn.host.trim_end_matches('/');
    let path = form_jcr_path.trim_end_matches('/');
    let url = format!("{host}{path}/jcr:content/guideContainer.af.dor.pdf");
    let resp = reqwest::Client::new()
        .get(&url)
        .basic_auth(&conn.username, Some(&conn.password))
        .send()
        .await
        .map_err(|e| format!("AEM DoR fetch failed: {e}"))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("AEM DoR read failed: {e}"))?
        .to_vec();
    if !status.is_success() {
        let snippet: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(format!("AEM DoR fetch failed (HTTP {status}): {snippet}"));
    }
    if !bytes.starts_with(b"%PDF") {
        let snippet: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(format!(
            "AEM DoR response was not a PDF (DoR may not be configured for this form): {snippet}"
        ));
    }
    Ok(bytes)
}

#[cfg(target_arch = "wasm32")]
pub async fn upload_and_install_package(
    _conn: &AemConnection,
    _zip: Vec<u8>,
    _package_name: &str,
) -> Result<(), String> {
    Err("AEM upload is only supported in the desktop app.".to_string())
}

#[cfg(target_arch = "wasm32")]
pub async fn fetch_form_html(_conn: &AemConnection, _form_jcr_path: &str) -> Result<String, String> {
    Err("AEM fetch is only supported in the desktop app.".to_string())
}

#[cfg(target_arch = "wasm32")]
pub async fn fetch_dor_pdf(_conn: &AemConnection, _form_jcr_path: &str) -> Result<Vec<u8>, String> {
    Err("AEM fetch is only supported in the desktop app.".to_string())
}
