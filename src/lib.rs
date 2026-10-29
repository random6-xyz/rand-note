use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use uuid::Uuid;
use wasm_bindgen::JsValue;
use worker::*;

const MAX_NOTE_CHARS: usize = 250;
const MAX_NOTE_LINES: usize = 10;
const MAX_REQUEST_BYTES: usize = 4_096;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_UPLOAD_REQUEST_BYTES: usize = MAX_IMAGE_BYTES + 16 * 1024;
const IMAGE_CACHE_CONTROL: &str = "public, max-age=3600";
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
    image_type: Option<String>,
}

struct NewNote {
    text: String,
    image: Option<NoteImage>,
}

struct NoteImage {
    content_type: &'static str,
    bytes: Vec<u8>,
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

    if method == Method::Get {
        if let Some(note_id) = path.strip_prefix("/api/images/") {
            return get_image(&env, note_id, origin).await;
        }
    }
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
            "SELECT id, text, created_at, board_date, COALESCE(image_type, '') FROM notes \
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
        "SELECT id, text, created_at, board_date, COALESCE(image_type, '') FROM notes \
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
            let [id, text, created_at, board_date, image_type]: [String; 5] = row.try_into().ok()?;
            Some(Note {
                id,
                text,
                created_at,
                board_date,
                image_type: (!image_type.is_empty()).then_some(image_type),
            })
        })
        .collect())
}

async fn create_note(mut request: Request, env: Env, origin: Option<&str>) -> Result<Response> {
    let is_multipart = request
        .headers()
        .get("Content-Type")?
        .is_some_and(|content_type| {
            content_type
                .to_ascii_lowercase()
                .starts_with("multipart/form-data")
        });
    let content_length = request.headers().get("Content-Length")?;
    if is_multipart {
        let within_limit = content_length
            .and_then(|length| length.parse::<usize>().ok())
            .is_some_and(|length| length <= MAX_UPLOAD_REQUEST_BYTES);
        if !within_limit {
            return api_response(
                &ApiError {
                    error: "image_too_large",
                },
                413,
                origin,
            );
        }
    } else if let Some(content_length) = content_length {
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

    let new_note = if is_multipart {
        read_form_note(&mut request).await
    } else {
        match request.json::<CreateNoteRequest>().await {
            Ok(body) => Ok(NewNote {
                text: body.text,
                image: None,
            }),
            Err(_) => Err("invalid_json"),
        }
    };
    let new_note = match new_note {
        Ok(new_note) => new_note,
        Err(error) => return api_response(&ApiError { error }, 400, origin),
    };
    let text = match validate_note_text(&new_note.text, new_note.image.is_some()) {
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
    let image_type = new_note.image.as_ref().map(|image| image.content_type);
    if let Some(image) = new_note.image {
        env.bucket("IMAGES")?
            .put(image_key(&id), image.bytes)
            .http_metadata(HttpMetadata {
                content_type: Some(image.content_type.to_owned()),
                ..Default::default()
            })
            .execute()
            .await?;
    }

    let database = env.d1("DB")?;
    let id_value = JsValue::from_str(&id);
    let text_value = JsValue::from_str(&text);
    let created_at_value = JsValue::from_str(&created_at);
    let board_date_value = JsValue::from_str(&board_date);
    let image_type_value = image_type.map_or(JsValue::NULL, JsValue::from_str);
    let inserted = database
        .prepare(
            "INSERT INTO notes (id, text, created_at, board_date, image_type) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(&[
            id_value,
            text_value,
            created_at_value,
            board_date_value,
            image_type_value,
        ])?
        .run()
        .await;
    if let Err(error) = inserted {
        if image_type.is_some() {
            if let Err(cleanup_error) = env.bucket("IMAGES")?.delete(image_key(&id)).await {
                console_error!("Image cleanup failed: {:?}", cleanup_error);
            }
        }
        return Err(error);
    }

    api_response(
        &NoteResponse {
            note: Note {
                id,
                text,
                created_at,
                board_date,
                image_type: image_type.map(str::to_owned),
            },
        },
        201,
        origin,
    )
}

async fn read_form_note(request: &mut Request) -> std::result::Result<NewNote, &'static str> {
    let form = request.form_data().await.map_err(|_| "invalid_form")?;
    let text = match form.get("text") {
        Some(FormEntry::Field(text)) => text,
        None => String::new(),
        Some(FormEntry::File(_)) => return Err("invalid_form"),
    };
    let image = match form.get("image") {
        Some(FormEntry::File(file)) => {
            if file.size() == 0 || file.size() > MAX_IMAGE_BYTES {
                return Err("image_too_large");
            }
            let bytes = file.bytes().await.map_err(|_| "invalid_form")?;
            let content_type = detect_image_type(&bytes).ok_or("unsupported_image")?;
            Some(NoteImage {
                content_type,
                bytes,
            })
        }
        None => None,
        Some(FormEntry::Field(_)) => return Err("invalid_form"),
    };

    Ok(NewNote { text, image })
}

async fn get_image(env: &Env, note_id: &str, origin: Option<&str>) -> Result<Response> {
    if Uuid::parse_str(note_id).is_err() {
        return api_response(&ApiError { error: "not_found" }, 404, origin);
    }
    let bucket = env.bucket("IMAGES")?;
    let Some(object) = bucket.get(image_key(note_id)).execute().await? else {
        return api_response(&ApiError { error: "not_found" }, 404, origin);
    };
    let Some(body) = object.body() else {
        return api_response(&ApiError { error: "not_found" }, 404, origin);
    };
    let content_type = object
        .http_metadata()
        .content_type
        .filter(|content_type| is_allowed_image_type(content_type))
        .unwrap_or_else(|| "application/octet-stream".to_owned());

    let headers = Headers::new();
    headers.set("content-type", &content_type)?;
    headers.set("cache-control", IMAGE_CACHE_CONTROL)?;
    headers.set("etag", &object.http_etag())?;
    headers.set("x-content-type-options", "nosniff")?;
    headers.set("content-security-policy", "default-src 'none'; sandbox")?;
    headers.set("cross-origin-resource-policy", "same-site")?;
    Ok(Response::from_body(body.response_body()?)?
        .with_headers(headers))
}

fn image_key(note_id: &str) -> String {
    format!("images/{note_id}")
}

fn detect_image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn is_allowed_image_type(content_type: &str) -> bool {
    matches!(
        content_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
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

    env.d1("DB")?
        .prepare("DELETE FROM notes WHERE id = ?1")
        .bind(&[JsValue::from_str(note_id)])?
        .run()
        .await?;
    if Uuid::parse_str(note_id).is_ok() {
        env.bucket("IMAGES")?.delete(image_key(note_id)).await?;
    }

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
    let database = env.d1("DB")?;
    let image_note_ids: Vec<Vec<String>> = database
        .prepare("SELECT id FROM notes WHERE board_date = ?1 AND image_type IS NOT NULL")
        .bind(&[JsValue::from_str(&today)])?
        .raw::<String>()
        .await?;
    database
        .prepare("DELETE FROM notes WHERE board_date = ?1")
        .bind(&[JsValue::from_str(&today)])?
        .run()
        .await?;

    let image_keys: Vec<String> = image_note_ids
        .iter()
        .filter_map(|row| row.first())
        .map(|note_id| image_key(note_id))
        .collect();
    let bucket = env.bucket("IMAGES")?;
    for keys in image_keys.chunks(1_000) {
        bucket.delete_multiple(keys.to_vec()).await?;
    }

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

fn validate_note_text(input: &str, has_image: bool) -> std::result::Result<String, &'static str> {
    let text = input.trim();
    if text.is_empty() && !has_image {
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
        assert_eq!(
            range("2026-10-13", "2026-10-20"),
            Err("history_not_available")
        );
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
        assert_eq!(
            validate_note_text("  hello  ", false),
            Ok("hello".to_owned())
        );
        assert_eq!(validate_note_text(" \n\t ", false), Err("empty_text"));
        assert_eq!(
            validate_note_text(&"가".repeat(MAX_NOTE_CHARS), false),
            Ok("가".repeat(MAX_NOTE_CHARS))
        );
        assert_eq!(
            validate_note_text(&"가".repeat(MAX_NOTE_CHARS + 1), false),
            Err("text_too_long")
        );

        let ten_lines = vec!["note"; MAX_NOTE_LINES].join("\n");
        let eleven_lines = vec!["note"; MAX_NOTE_LINES + 1].join("\n");
        assert_eq!(validate_note_text(&ten_lines, false), Ok(ten_lines));
        assert_eq!(
            validate_note_text(&eleven_lines, false),
            Err("too_many_lines")
        );
        assert_eq!(note_line_count("first\r\nsecond"), 2);
    }

    #[test]
    fn note_text_may_be_empty_only_with_an_image() {
        assert_eq!(validate_note_text("  ", true), Ok(String::new()));
        assert_eq!(
            validate_note_text("caption", true),
            Ok("caption".to_owned())
        );
        assert_eq!(
            validate_note_text(&"a".repeat(MAX_NOTE_CHARS + 1), true),
            Err("text_too_long")
        );
    }

    #[test]
    fn image_type_is_detected_from_file_signature() {
        assert_eq!(
            detect_image_type(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"),
            Some("image/png")
        );
        assert_eq!(
            detect_image_type(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10]),
            Some("image/jpeg")
        );
        assert_eq!(detect_image_type(b"GIF89a\x01\0\x01\0"), Some("image/gif"));
        assert_eq!(detect_image_type(b"GIF87a\x01\0\x01\0"), Some("image/gif"));
        assert_eq!(
            detect_image_type(b"RIFF\x24\0\0\0WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(
            detect_image_type(b"<svg xmlns=\"http://www.w3.org/2000/svg\">"),
            None
        );
        assert_eq!(detect_image_type(b"<html><script>"), None);
        assert_eq!(detect_image_type(b"RIFF\x24\0\0\0WAVE"), None);
        assert_eq!(detect_image_type(b""), None);
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
