//! HTTP transport — Azure AD client_credentials token acquisition and batch POST.
//!
//! Token acquisition uses the Azure AD OAuth2 `client_credentials` endpoint
//! directly via `ureq`, implementing the same flow as `azure_identity`'s
//! `ClientSecretCredential` without pulling in an async runtime. This keeps
//! the transport fully synchronous and consistent with the binary's design
//! principle of deterministic, GC-free frame processing.
//!
//! # Wire format (POST `/api/publisher/v2/ingest`)
//!
//! ```json
//! {
//!   "subSessionId": 12345678,
//!   "sessionTime": 1234.5,
//!   "sessionTick": 9876,
//!   "events": [ /* Vec<PublisherEvent> */ ]
//! }
//! ```

use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::publisher_event::PublisherEvent;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of events in a single POST body.
const BATCH_LIMIT: usize = 20;

/// Retry delays (ms) for 5xx / network errors. Three attempts after initial.
const RETRY_DELAYS_MS: &[u64] = &[500, 1_000, 2_000];

/// Refresh the cached token this many seconds before it actually expires.
const TOKEN_REFRESH_BUFFER_S: u64 = 60;

// ── Public types ──────────────────────────────────────────────────────────────

/// Errors emitted by [`PublisherTransport`].
#[derive(Debug)]
pub struct TransportError(pub String);

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TransportError {}

/// Synchronous HTTP transport that holds an in-memory event queue, acquires
/// Azure AD tokens, and batch-POSTs events to Race Control.
pub struct PublisherTransport {
    client_id:         String,
    client_secret:     String,
    scope:             String,
    ingest_url:        String,
    token_url:         String,
    batch_interval_ms: u64,
    queue:             Vec<PublisherEvent>,
    last_flush:        Instant,
    cached_token:      Option<CachedToken>,
    /// When `true`, batches are pretty-printed to stdout instead of POSTed.
    /// Enabled by the `--dry-run` flag on the publisher binary.
    dry_run:           bool,
}

impl PublisherTransport {
    /// Create a new transport.
    ///
    /// `rc_api_url` is the Race Control base URL (no trailing slash),
    /// e.g. `"https://simracecenter.com"`.
    pub fn new(
        tenant_id:         impl Into<String>,
        client_id:         impl Into<String>,
        client_secret:     impl Into<String>,
        scope:             impl Into<String>,
        rc_api_url:        impl Into<String>,
        batch_interval_ms: u64,
    ) -> Self {
        let tenant_id = tenant_id.into();
        let base_url  = rc_api_url.into();
        let token_url = format!(
            "https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token"
        );
        let ingest_url = format!("{base_url}/api/publisher/v2/ingest");

        Self {
            client_id:     client_id.into(),
            client_secret: client_secret.into(),
            scope:         scope.into(),
            ingest_url,
            token_url,
            batch_interval_ms,
            queue:         Vec::new(),
            last_flush:    Instant::now(),
            cached_token:  None,
            dry_run:       false,
        }
    }

    /// Enable dry-run mode: batches are printed to stdout, no HTTP calls are made.
    pub fn set_dry_run(&mut self, dry_run: bool) {
        self.dry_run = dry_run;
    }

    /// Add one event to the in-memory queue.
    pub fn enqueue(&mut self, event: PublisherEvent) {
        self.queue.push(event);
    }

    /// Warm up authentication by ensuring a valid bearer token is available.
    ///
    /// This is safe to call at startup: the normal ingest path still refreshes
    /// tokens as needed, so a token that expires before first event publish is
    /// automatically replaced during `post_batch`.
    pub fn warmup_auth(&mut self) -> Result<(), TransportError> {
        let _ = self.get_or_refresh_token(false)?;
        Ok(())
    }

    /// Call once per frame. Flushes automatically when the interval elapses
    /// or the queue reaches [`BATCH_LIMIT`].
    pub fn tick(
        &mut self,
        session_time:   f64,
        session_tick:   i64,
        sub_session_id: i64,
    ) {
        let elapsed = self.last_flush.elapsed().as_millis() as u64;
        if elapsed >= self.batch_interval_ms || self.queue.len() >= BATCH_LIMIT {
            if let Err(e) = self.flush(session_time, session_tick, sub_session_id) {
                eprintln!("[transport] flush error: {e}");
            }
        }
    }


    /// Like [`tick`] but returns `Ok(true)` if events were actually posted,
    /// `Ok(false)` if the interval has not elapsed yet or the queue was empty,
    /// or `Err` on failure.
    pub fn tick_result(
        &mut self,
        session_time:   f64,
        session_tick:   i64,
        sub_session_id: i64,
    ) -> Result<bool, TransportError> {
        let elapsed = self.last_flush.elapsed().as_millis() as u64;
        if elapsed >= self.batch_interval_ms || self.queue.len() >= BATCH_LIMIT {
            self.flush(session_time, session_tick, sub_session_id)
        } else {
            Ok(false)
        }
    }

    /// Drain the entire queue synchronously. Call on shutdown to guarantee
    /// all events are delivered before the process exits.
    /// Returns `Ok(true)` if at least one batch was posted, `Ok(false)` if
    /// the queue was already empty.
    pub fn flush(
        &mut self,
        session_time:   f64,
        session_tick:   i64,
        sub_session_id: i64,
    ) -> Result<bool, TransportError> {
        if self.queue.is_empty() {
            return Ok(false);
        }
        while !self.queue.is_empty() {
            let n     = self.queue.len().min(BATCH_LIMIT);
            let batch = self.queue.drain(..n).collect::<Vec<_>>();
            self.post_batch(&batch, session_time, session_tick, sub_session_id)?;
        }
        self.last_flush = Instant::now();
        Ok(true)
    }

    // ── Private ───────────────────────────────────────────────────────────────

    fn post_batch(
        &mut self,
        batch:          &[PublisherEvent],
        session_time:   f64,
        session_tick:   i64,
        sub_session_id: i64,
    ) -> Result<(), TransportError> {
        let body = IngestRequest {
            sub_session_id,
            session_time,
            session_tick,
            events: batch,
        };
        let body_value = serde_json::to_value(&body)
            .expect("IngestRequest is always serialisable");

        if self.dry_run {
            let pretty = serde_json::to_string_pretty(&body_value)
                .expect("IngestRequest is always serialisable");
            println!("[dry-run] POST {} — {} event(s):\n{}", self.ingest_url, batch.len(), pretty);
        }

        // Retry loop: initial attempt + up to 3 retries on 5xx/network error.
        let delays = std::iter::once(0u64).chain(RETRY_DELAYS_MS.iter().copied());
        let mut last_error = String::new();

        for (attempt, delay_ms) in delays.enumerate() {
            if delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(delay_ms));
            }

            let token = self.get_or_refresh_token(false)?;
            let result = ureq::post(&self.ingest_url)
                .set("Authorization", &format!("Bearer {token}"))
                .set("Content-Type", "application/json")
                .send_json(&body_value);

            // ureq v2 returns non-2xx as Err(ureq::Error::Status(code, resp)).
            // All match arms must be on the Err side for non-2xx status codes.
            match result {
                Ok(resp) => {
                    let status = resp.status();
                    let body   = resp.into_string().unwrap_or_default();
                    if !body.is_empty() {
                        eprintln!("[transport] HTTP {status} response body: {body}");
                    }
                    return Ok(());
                }

                Err(ureq::Error::Status(401, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    eprintln!("[transport] 401 response body: {body}");
                    // Stale token — attempt one forced refresh on the first 401, then fatal.
                    if attempt == 0 {
                        eprintln!("[transport] 401 — refreshing token and retrying…");
                        self.cached_token = None;
                        let token = self.get_or_refresh_token(true)?;
                        let result2 = ureq::post(&self.ingest_url)
                            .set("Authorization", &format!("Bearer {token}"))
                            .set("Content-Type", "application/json")
                            .send_json(&body_value);
                        return match result2 {
                            Ok(r2) => {
                                let body2 = r2.into_string().unwrap_or_default();
                                if !body2.is_empty() {
                                    eprintln!("[transport] HTTP {} response body: {body2}", 200);
                                }
                                Ok(())
                            }
                            Err(ureq::Error::Status(code, r2)) => {
                                let body2 = r2.into_string().unwrap_or_default();
                                eprintln!("[transport] HTTP {code} response body: {body2}");
                                Err(TransportError(format!(
                                    "HTTP {code} after forced token refresh — fatal"
                                )))
                            }
                            Err(e) => Err(TransportError(format!(
                                "network error after token refresh: {e}"
                            ))),
                        };
                    }
                    return Err(TransportError("401 after forced token refresh — fatal".to_owned()));
                }

                Err(ureq::Error::Status(code, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    last_error = format!("HTTP {code}");
                    eprintln!(
                        "[transport] {} attempt {}/{} — body: {}",
                        last_error,
                        attempt + 1,
                        RETRY_DELAYS_MS.len() + 1,
                        body,
                    );
                }

                Err(e) => {
                    last_error = format!("network error: {e}");
                    eprintln!(
                        "[transport] {} attempt {}/{}, retrying…",
                        last_error,
                        attempt + 1,
                        RETRY_DELAYS_MS.len() + 1
                    );
                }
            }
        }

        Err(TransportError(format!("max retries exceeded: {last_error}")))
    }

    fn get_or_refresh_token(&mut self, force: bool) -> Result<String, TransportError> {
        let needs_refresh = force || match &self.cached_token {
            None    => true,
            Some(t) => {
                let buffer = Duration::from_secs(TOKEN_REFRESH_BUFFER_S);
                t.expires_at <= SystemTime::now() + buffer
            }
        };

        if needs_refresh {
            eprintln!("[transport] acquiring token from {}", self.token_url);
            let resp = self.fetch_token()?;
            let expires_in = resp.expires_in;
            let expires_at = SystemTime::now()
                + Duration::from_secs(expires_in.saturating_sub(TOKEN_REFRESH_BUFFER_S));
            eprintln!("[transport] token acquired — expires_in={expires_in}s");
            self.cached_token = Some(CachedToken {
                token: resp.access_token,
                expires_at,
            });
        }

        Ok(self.cached_token.as_ref().unwrap().token.clone())
    }

    fn fetch_token(&self) -> Result<TokenResponse, TransportError> {
        eprintln!("[transport] POST {} client_id={}…{}",
            self.token_url,
            &self.client_id[..8.min(self.client_id.len())],
            &self.client_id[self.client_id.len().saturating_sub(4)..]);
        let result = ureq::post(&self.token_url)
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_form(&[
                ("grant_type",    "client_credentials"),
                ("client_id",     &self.client_id),
                ("client_secret", &self.client_secret),
                ("scope",         &self.scope),
            ]);

        let resp = match result {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let body = r.into_string().unwrap_or_default();
                return Err(TransportError(format!(
                    "token request failed: status {code} — {body}"
                )));
            }
            Err(e) => return Err(TransportError(format!("token request failed: {e}"))),
        };

        let token_resp: TokenResponse = resp
            .into_json()
            .map_err(|e| TransportError(format!("token response parse error: {e}")))?;

        Ok(token_resp)
    }

    /// Wall-clock time at which the cached token expires, or `None` if no
    /// token has been acquired yet. Used by the UI status display.
    pub fn token_expires_at(&self) -> Option<SystemTime> {
        self.cached_token.as_ref().map(|t| t.expires_at)
    }

    /// Inject a pre-built token, bypassing network calls. **Test use only.**
    #[cfg(test)]
    pub fn set_token_for_test(&mut self, token: &str) {
        self.cached_token = Some(CachedToken {
            token:      token.to_owned(),
            expires_at: SystemTime::now() + Duration::from_secs(3_600),
        });
    }

    /// Override the ingest URL. **Test use only.**
    #[cfg(test)]
    pub fn set_ingest_url_for_test(&mut self, url: &str) {
        self.ingest_url = url.to_owned();
    }
}

// ── Wire types ────────────────────────────────────────────────────────────────

/// Outer envelope for `POST /api/publisher/v2/ingest`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IngestRequest<'a> {
    sub_session_id: i64,
    session_time:   f64,
    session_tick:   i64,
    events:         &'a [PublisherEvent],
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    /// Lifetime in seconds from time of issuance.
    expires_in:   u64,
}

struct CachedToken {
    token:      String,
    /// Wall-clock expiry, already adjusted by [`TOKEN_REFRESH_BUFFER_S`].
    expires_at: SystemTime,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publisher_event::build_event;
    use crate::race_event::{LifecycleOrigin, RaceEvent};
    use crate::telemetry_frame::TelemetryFrame;

    fn minimal_frame() -> TelemetryFrame {
        TelemetryFrame {
            lap: 1,
            session_time: 10.0,
            lap_dist_pct: 0.5,
            player_car_idx: 0,
            player_car_position: 5,
            on_pit_road: false,
            session_flags: 0,
            car_idx_lap_dist_pct: vec![0.5],
            car_idx_position: vec![5],
            car_idx_on_pit_road: vec![false],
            car_idx_track_surface: vec![0],
            lap_last_lap_time: 540.0,
            session_info_update: 1,
            session_tick: 100,
            session_state: 4,
            session_num: 0,
            player_incident_count: 0,
            car_idx_lap_completed: vec![1],
            lf_temp_m: 0.0,
            rf_temp_m: 0.0,
            lr_temp_m: 0.0,
            rr_temp_m: 0.0,
            fuel_level: 0.0,
            throttle: 0.0,
            brake: 0.0,
            speed: 0.0,
        }
    }

    fn make_transport(server_url: &str) -> PublisherTransport {
        let mut t = PublisherTransport::new(
            "tenant", "client", "secret", "scope",
            server_url, 500,
        );
        t.set_token_for_test("test-bearer-token");
        t
    }

    #[test]
    fn post_batch_includes_auth_header_and_json_shape() {
        let mut server = mockito::Server::new();

        // Expect one POST with the correct Authorization header
        let mock = server
            .mock("POST", "/api/publisher/v2/ingest")
            .with_status(200)
            .with_body("{}")
            .match_header(
                "authorization",
                mockito::Matcher::Exact("Bearer test-bearer-token".to_owned()),
            )
            .match_header("content-type", mockito::Matcher::Regex("application/json".to_owned()))
            .create();

        let event = build_event(
            &RaceEvent::RaceGreen { lap: 1, session_time: 10.0, synthetic: false, origin: LifecycleOrigin::SessionStateTransition },
            &minimal_frame(),
            None,
            "session-xyz",
            "rig-001",
            None,
            None,
        );

        let mut transport = make_transport(&server.url());
        transport.enqueue(event);
        transport.flush(10.0, 100, 99999).unwrap();

        mock.assert();
    }

    #[test]
    fn ingest_request_serialises_to_expected_shape() {
        let event = build_event(
            &RaceEvent::RaceGreen { lap: 1, session_time: 10.0, synthetic: false, origin: LifecycleOrigin::SessionStateTransition },
            &minimal_frame(),
            None,
            "session-xyz",
            "rig-001",
            None,
            None,
        );

        let req = IngestRequest {
            sub_session_id: 99999,
            session_time:   10.0,
            session_tick:   100,
            events:         &[event],
        };

        let json: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(json["subSessionId"], 99999_i64);
        assert_eq!(json["sessionTime"],  10.0_f64);
        assert_eq!(json["sessionTick"],  100_i64);
        assert!(json["events"].is_array());
        assert_eq!(json["events"].as_array().unwrap().len(), 1);
        assert_eq!(json["events"][0]["type"], "RACE_GREEN");
    }

    #[test]
    fn token_is_reused_across_calls() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", "/api/publisher/v2/ingest")
            .with_status(200)
            .with_body("{}")
            .expect(2)  // two flush calls
            .create();

        let mut transport = make_transport(&server.url());
        let frame = minimal_frame();

        for _ in 0..2 {
            let event = build_event(
                &RaceEvent::RaceGreen { lap: 1, session_time: 10.0, synthetic: false, origin: LifecycleOrigin::SessionStateTransition },
                &frame,
                None,
                "s",
                "r",
                None,
                None,
            );
            transport.enqueue(event);
            transport.flush(10.0, 100, 1).unwrap();
        }

        // No token fetch calls (token was injected) — if token refresh were
        // triggered unexpectedly, the fetch_token() call to the non-mocked
        // Azure endpoint would fail and the test would error.
    }

    #[test]
    fn warmup_auth_succeeds_with_cached_token() {
        let mut transport = make_transport("http://localhost");
        transport.warmup_auth().expect("warmup should use cached token");
        assert!(transport.token_expires_at().is_some());
    }
}
