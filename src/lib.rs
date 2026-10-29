use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use uuid::Uuid;
use wasm_bindgen::JsValue;
use worker::*;

const MAX_NOTE_CHARS: usize = 250;
const MAX_NOTE_LINES: usize = 10;
const MAX_REQUEST_BYTES: usize = 4_096;
const KST_OFFSET_SECONDS: i32 = 9 * 60 * 60;
const HISTORY_START_DATE: &str = "2026-10-14";
const MAX_RANGE_DAYS: i64 = 31;
const ADMIN_COOKIE_NAME: &str = "admin_session";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateNoteRequest {
    text: String,
}

#[derive(Debug, Serialize)]
struct Note {
    id: String,
    text: String,
    created_at: String,
    board_date: String,
}

#[derive(Serialize)]
struct NotesResponse {
    date: String,
    notes: Vec<Note>,
}

#[derive(Serialize)]
struct RangeNotesResponse {
    from: String,
    to: String,
    notes: Vec<Note>,
}

#[derive(Serialize)]
struct NoteResponse {
    note: Note,
}

#[derive(Serialize)]
struct ApiError {
    error: &'static str,
}

#[derive(Serialize)]
struct AdminSessionResponse {
    authenticated: bool,
}

#[derive(Serialize)]
struct AdminActionResponse {
    ok: bool,
}

#[event(fetch)]
pub async fn main(request: Request, env: Env, _ctx: Context) -> Result<Response> {
    let origin = match request_origin(&request, &env) {
        Ok(origin) => origin,
        Err(()) => {
            return api_response(
                &ApiError {
                    error: "origin_not_allowed",
                },
                403,
                None,
            );
        }
    };

    if request.method() == Method::Options {
        return preflight_response(origin.as_deref());
    }

    match handle_request(request, env, origin.as_deref()).await {
        Ok(response) => Ok(response),
        Err(error) => {
            console_error!("Request failed: {:?}", error);
            api_response(
                &ApiError {
                    error: "internal_error",
                },
                500,
                origin.as_deref(),
            )
        }
    }
}

fn request_origin(request: &Request, env: &Env) -> std::result::Result<Option<String>, ()> {
    let allowed_origin = env.var("ALLOWED_ORIGIN").map_err(|_| ())?.to_string();
    let origin = request.headers().get("Origin").map_err(|_| ())?;

    if origin
        .as_deref()
        .is_some_and(|origin| origin != allowed_origin)
    {
        return Err(());
    }

    Ok(origin)
}

async fn handle_request(request: Request, env: Env, origin: Option<&str>) -> Result<Response> {
    let url = request.url()?;
    let path = url.path().to_owned();
    let method = request.method();

    if method == Method::Get && path == "/api/admin/session" {
        return admin_session(&request, &env, origin);
    }
    if method == Method::Delete && path == "/api/admin/notes/today" {
        return reset_today(&request, env, origin).await;
    }
    if method == Method::Delete {
        if let Some(note_id) = path
            .strip_prefix("/api/admin/notes/")
            .filter(|note_id| !note_id.is_empty() && !note_id.contains('/'))
        {
            return delete_note(&request, env, note_id, origin).await;
        }
    }

    if path != "/api/notes" {
        return api_response(&ApiError { error: "not_found" }, 404, origin);
    }

    match method {
        Method::Get => get_notes(url, env, origin).await,
        Method::Post => create_note(request, env, origin).await,
        _ => api_response(
            &ApiError {
                error: "method_not_allowed",
            },
            405,
            origin,
        ),
    }
}

async fn get_notes(url: Url, env: Env, origin: Option<&str>) -> Result<Response> {
    let today = current_kst_date()?;
    let query_value = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    let requested_date = query_value("date");
    let requested_from = query_value("from");
    let requested_to = query_value("to");

    if requested_from.is_some() || requested_to.is_some() {
        if requested_date.is_some() {
            return api_response(
                &ApiError {
                    error: "invalid_range",
                },
                400,
                origin,
            );
        }
        let (from, to) = match validate_range(requested_from, requested_to, &today) {
            Ok(range) => range,
            Err(error) => return api_response(&ApiError { error }, 400, origin),
        };
        let notes = query_notes(
            &env,
            "SELECT id, text, created_at, board_date FROM notes \
             WHERE board_date BETWEEN ?1 AND ?2 ORDER BY created_at ASC, id ASC",
            &[JsValue::from_str(&from), JsValue::from_str(&to)],
        )
        .await?;
        return api_response(&RangeNotesResponse { from, to, notes }, 200, origin);
    }

    let date = match validate_date(requested_date, &today) {
        Ok(date) => date,
        Err(error) => return api_response(&ApiError { error }, 400, origin),
    };
    let notes = query_notes(
        &env,
        "SELECT id, text, created_at, board_date FROM notes \
         WHERE board_date = ?1 ORDER BY created_at ASC, id ASC",
        &[JsValue::from_str(&date)],
    )
    .await?;

    api_response(&NotesResponse { date, notes }, 200, origin)
}

async fn query_notes(env: &Env, query: &str, values: &[JsValue]) -> Result<Vec<Note>> {
    let rows: Vec<Vec<String>> = env
        .d1("DB")?
        .prepare(query)
        .bind(values)?
        .raw::<String>()
        .await?;

    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let [id, text, created_at, board_date]: [String; 4] = row.try_into().ok()?;
            Some(Note {
                id,
                text,
                created_at,
                board_date,
            })
        })
        .collect())
}

async fn create_note(mut request: Request, env: Env, origin: Option<&str>) -> Result<Response> {
    if let Some(content_length) = request.headers().get("Content-Length")? {
        if content_length.parse::<usize>().unwrap_or(usize::MAX) > MAX_REQUEST_BYTES {
            return api_response(
                &ApiError {
                    error: "invalid_text",
                },
                400,
                origin,
            );
        }
    }

    let client_ip = request
        .headers()
        .get("CF-Connecting-IP")?
        .unwrap_or_else(|| "local-development".to_owned());
    let rate_limit = env
        .rate_limiter("NOTE_LIMITER")?
        .limit(format!("note-create:{client_ip}"))
        .await?;
    if !rate_limit.success {
        return api_response(
            &ApiError {
                error: "rate_limited",
            },
            429,
            origin,
        );
    }

    let body = match request.json::<CreateNoteRequest>().await {
        Ok(body) => body,
        Err(_) => {
            return api_response(
                &ApiError {
                    error: "invalid_json",
                },
                400,
                origin,
            );
        }
    };
    let text = match validate_note_text(&body.text) {
        Ok(text) => text,
        Err(error) => return api_response(&ApiError { error }, 400, origin),
    };

    let now_millis = js_sys::Date::now() as i64;
    let created_at = match DateTime::<Utc>::from_timestamp_millis(now_millis) {
        Some(timestamp) => timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        None => {
            return api_response(
                &ApiError {
                    error: "internal_error",
                },
                500,
                origin,
            );
        }
    };
    let board_date = match kst_date_from_unix_millis(now_millis) {
        Some(date) => date,
        None => {
            return api_response(
                &ApiError {
                    error: "internal_error",
                },
                500,
                origin,
            );
        }
    };

    let id = Uuid::new_v4().to_string();
    let database = env.d1("DB")?;
    let id_value = JsValue::from_str(&id);
    let text_value = JsValue::from_str(&text);
    let created_at_value = JsValue::from_str(&created_at);
    let board_date_value = JsValue::from_str(&board_date);
    database
        .prepare(
            "INSERT INTO notes (id, text, created_at, board_date) \
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&[id_value, text_value, created_at_value, board_date_value])?
        .run()
        .await?;

    api_response(
        &NoteResponse {
            note: Note {
                id,
                text,
                created_at,
                board_date,
            },
        },
        201,
        origin,
    )
}

fn admin_session(request: &Request, env: &Env, origin: Option<&str>) -> Result<Response> {
    let response = AdminSessionResponse {
        authenticated: is_admin_cookie(request, env)?,
    };
    api_response(&response, 200, origin)
}

async fn delete_note(
    request: &Request,
    env: Env,
    note_id: &str,
    origin: Option<&str>,
) -> Result<Response> {
    if origin.is_none() {
        return api_response(
            &ApiError {
                error: "origin_required",
            },
            403,
            origin,
        );
    }
    if !is_admin_cookie(request, &env)? {
        return api_response(
            &ApiError {
                error: "admin_required",
            },
            401,
            origin,
        );
    }

    let note_id = JsValue::from_str(note_id);
    env.d1("DB")?
        .prepare("DELETE FROM notes WHERE id = ?1")
        .bind(&[note_id])?
        .run()
        .await?;

    api_response(&AdminActionResponse { ok: true }, 200, origin)
}

async fn reset_today(request: &Request, env: Env, origin: Option<&str>) -> Result<Response> {
    if origin.is_none() {
        return api_response(
            &ApiError {
                error: "origin_required",
            },
            403,
            origin,
        );
    }
    if !is_admin_cookie(request, &env)? {
        return api_response(
            &ApiError {
                error: "admin_required",
            },
            401,
            origin,
        );
    }

    let today = current_kst_date()?;
    let today_value = JsValue::from_str(&today);
    env.d1("DB")?
        .prepare("DELETE FROM notes WHERE board_date = ?1")
        .bind(&[today_value])?
        .run()
        .await?;

    api_response(&AdminActionResponse { ok: true }, 200, origin)
}

fn is_admin_cookie(request: &Request, env: &Env) -> Result<bool> {
    let expected = match env.secret("ADMIN_TOKEN") {
        Ok(token) => token.to_string(),
        Err(_) => return Ok(false),
    };
    let Some(cookie_header) = request.headers().get("Cookie")? else {
        return Ok(false);
    };
    let Some(cookie) = cookie_value(&cookie_header, ADMIN_COOKIE_NAME) else {
        return Ok(false);
    };

    Ok(admin_token_matches(&expected, cookie))
}

fn admin_token_matches(expected: &str, provided: &str) -> bool {
    if expected.len() < 32 || expected.len() != provided.len() {
        return false;
    }
    expected.as_bytes().ct_eq(provided.as_bytes()).unwrap_u8() == 1
}

fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|part| {
        let (cookie_name, value) = part.trim().split_once('=')?;
        (cookie_name == name).then_some(value)
    })
}

fn api_response<T: Serialize>(body: &T, status: u16, origin: Option<&str>) -> Result<Response> {
    let headers = cors_headers(origin)?;
    headers.set("content-type", "application/json; charset=utf-8")?;
    Ok(Response::from_json(body)?
        .with_status(status)
        .with_headers(headers))
}

fn preflight_response(origin: Option<&str>) -> Result<Response> {
    Ok(Response::empty()?
        .with_status(204)
        .with_headers(cors_headers(origin)?))
}

fn cors_headers(origin: Option<&str>) -> Result<Headers> {
    let headers = Headers::new();
    headers.set("access-control-allow-methods", "GET, POST, DELETE, OPTIONS")?;
    headers.set("access-control-allow-headers", "content-type")?;
    headers.set("access-control-allow-credentials", "true")?;
    headers.set("access-control-max-age", "600")?;
    headers.set("vary", "Origin")?;
    if let Some(origin) = origin {
        headers.set("access-control-allow-origin", origin)?;
    }
    Ok(headers)
}

fn current_kst_date() -> Result<String> {
    kst_date_from_unix_millis(js_sys::Date::now() as i64)
        .ok_or_else(|| Error::RustError("unable to calculate current KST date".to_owned()))
}

fn kst_date_from_unix_millis(timestamp_millis: i64) -> Option<String> {
    let timestamp = DateTime::<Utc>::from_timestamp_millis(timestamp_millis)?;
    let kst = FixedOffset::east_opt(KST_OFFSET_SECONDS)?;
    Some(timestamp.with_timezone(&kst).format("%Y-%m-%d").to_string())
}

fn validate_date(
    requested_date: Option<String>,
    today: &str,
) -> std::result::Result<String, &'static str> {
    let is_explicit_date = requested_date.is_some();
    let date = requested_date.unwrap_or_else(|| today.to_owned());
    let parsed_date = NaiveDate::parse_from_str(&date, "%Y-%m-%d").map_err(|_| "invalid_date")?;
    if parsed_date.format("%Y-%m-%d").to_string() != date {
        return Err("invalid_date");
    }

    let parsed_today = NaiveDate::parse_from_str(today, "%Y-%m-%d").map_err(|_| "invalid_date")?;
    if parsed_date > parsed_today {
        return Err("future_date");
    }

    let history_start =
        NaiveDate::parse_from_str(HISTORY_START_DATE, "%Y-%m-%d").map_err(|_| "invalid_date")?;
    if is_explicit_date && parsed_date < history_start {
        return Err("history_not_available");
    }

    Ok(date)
}

fn validate_range(
    requested_from: Option<String>,
    requested_to: Option<String>,
    today: &str,
) -> std::result::Result<(String, String), &'static str> {
    let (Some(from), Some(to)) = (requested_from, requested_to) else {
        return Err("invalid_range");
    };
    let from = validate_date(Some(from), today)?;
    let to = validate_date(Some(to), today)?;

    let parsed_from = NaiveDate::parse_from_str(&from, "%Y-%m-%d").map_err(|_| "invalid_date")?;
    let parsed_to = NaiveDate::parse_from_str(&to, "%Y-%m-%d").map_err(|_| "invalid_date")?;
    if parsed_from > parsed_to {
        return Err("invalid_range");
    }
    if (parsed_to - parsed_from).num_days() + 1 > MAX_RANGE_DAYS {
        return Err("range_too_long");
    }

    Ok((from, to))
}

fn note_line_count(text: &str) -> usize {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .count()
}

fn validate_note_text(input: &str) -> std::result::Result<String, &'static str> {
    let text = input.trim();
    if text.is_empty() {
        return Err("empty_text");
    }
    if note_line_count(text) > MAX_NOTE_LINES {
        return Err("too_many_lines");
    }
    if text.chars().count() > MAX_NOTE_CHARS {
        return Err("text_too_long");
    }
    Ok(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kst_date_changes_at_utc_fifteen_hundred() {
        let before = DateTime::parse_from_rfc3339("2024-01-01T14:59:59Z")
            .unwrap()
            .timestamp_millis();
        let at_boundary = DateTime::parse_from_rfc3339("2024-01-01T15:00:00Z")
            .unwrap()
            .timestamp_millis();

        assert_eq!(
            kst_date_from_unix_millis(before).as_deref(),
            Some("2024-01-01")
        );
        assert_eq!(
            kst_date_from_unix_millis(at_boundary).as_deref(),
            Some("2024-01-02")
        );
    }

    #[test]
    fn date_validation_allows_history_from_start_through_today() {
        assert_eq!(
            validate_date(Some("2026-02-30".to_owned()), "2026-10-15"),
            Err("invalid_date")
        );
        assert_eq!(
            validate_date(Some("2026-10-13".to_owned()), "2026-10-15"),
            Err("history_not_available")
        );
        assert_eq!(
            validate_date(Some("2026-10-14".to_owned()), "2026-10-15"),
            Ok("2026-10-14".to_owned())
        );
        assert_eq!(
            validate_date(Some("2026-10-15".to_owned()), "2026-10-15"),
            Ok("2026-10-15".to_owned())
        );
        assert_eq!(
            validate_date(Some("2026-10-16".to_owned()), "2026-10-15"),
            Err("future_date")
        );
    }

    #[test]
    fn date_validation_defaults_to_today_before_history_starts() {
        assert_eq!(
            validate_date(None, "2026-10-13"),
            Ok("2026-10-13".to_owned())
        );
    }

    #[test]
    fn range_validation_requires_ordered_dates_within_history() {
        let today = "2026-11-20";
        let range = |a: &str, b: &str| validate_range(Some(a.into()), Some(b.into()), today);

        assert_eq!(
            range("2026-10-14", "2026-10-20"),
            Ok(("2026-10-14".to_owned(), "2026-10-20".to_owned()))
        );
        assert_eq!(
            range("2026-10-20", "2026-10-20"),
            Ok(("2026-10-20".to_owned(), "2026-10-20".to_owned()))
        );
        assert_eq!(range("2026-10-20", "2026-10-14"), Err("invalid_range"));
        assert_eq!(range("2026-10-13", "2026-10-20"), Err("history_not_available"));
        assert_eq!(range("2026-11-01", "2026-11-21"), Err("future_date"));
        assert_eq!(range("2026-10-15", "2026-02-30"), Err("invalid_date"));
        assert_eq!(
            validate_range(Some("2026-10-14".to_owned()), None, today),
            Err("invalid_range")
        );
    }

    #[test]
    fn range_validation_limits_span_to_max_days() {
        let today = "2026-12-31";
        let range = |a: &str, b: &str| validate_range(Some(a.into()), Some(b.into()), today);

        assert!(range("2026-11-01", "2026-12-01").is_ok());
        assert_eq!(range("2026-11-01", "2026-12-02"), Err("range_too_long"));
    }

    #[test]
    fn note_text_is_trimmed_and_limited_by_unicode_characters_and_lines() {
        assert_eq!(validate_note_text("  hello  "), Ok("hello".to_owned()));
        assert_eq!(validate_note_text(" \n\t "), Err("empty_text"));
        assert_eq!(
            validate_note_text(&"가".repeat(MAX_NOTE_CHARS)),
            Ok("가".repeat(MAX_NOTE_CHARS))
        );
        assert_eq!(
            validate_note_text(&"가".repeat(MAX_NOTE_CHARS + 1)),
            Err("text_too_long")
        );

        let ten_lines = vec!["note"; MAX_NOTE_LINES].join("\n");
        let eleven_lines = vec!["note"; MAX_NOTE_LINES + 1].join("\n");
        assert_eq!(validate_note_text(&ten_lines), Ok(ten_lines));
        assert_eq!(validate_note_text(&eleven_lines), Err("too_many_lines"));
        assert_eq!(note_line_count("first\r\nsecond"), 2);
    }

    #[test]
    fn admin_cookie_is_parsed_by_exact_name_and_token_match() {
        let header = "theme=dark; admin_session=0123456789abcdef0123456789abcdef; path=/";
        let cookie = cookie_value(header, ADMIN_COOKIE_NAME).unwrap();
        assert_eq!(cookie, "0123456789abcdef0123456789abcdef");
        assert_eq!(cookie_value(header, "admin"), None);
        assert!(admin_token_matches(cookie, cookie));
        assert!(!admin_token_matches(
            cookie,
            "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
        ));
        assert!(!admin_token_matches("short", "short"));
    }
}
