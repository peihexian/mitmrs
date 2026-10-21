use crate::decode::util::is_probably_text;
use crate::model::*;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{
    params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension, Row,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
pub struct Store {
    db_path: Arc<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExportBundle {
    version: u32,
    app: String,
    exported_at: DateTime<Utc>,
    count: usize,
    flows: Vec<ExportFlow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExportFlow {
    id: String,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    duration_ms: Option<i64>,
    client: String,
    scheme: String,
    host: String,
    port: u16,
    method: String,
    path: String,
    query: Option<String>,
    http_version: String,
    status: Option<u16>,
    reason: Option<String>,
    request_headers: Vec<HeaderEntry>,
    response_headers: Vec<HeaderEntry>,
    request_trailers: Vec<HeaderEntry>,
    response_trailers: Vec<HeaderEntry>,
    request_body_base64: String,
    response_body_base64: String,
    request_body_size: u64,
    response_body_size: u64,
    request_truncated: bool,
    response_truncated: bool,
    request_mime: Option<String>,
    response_mime: Option<String>,
    error: Option<String>,
    tls_sni: Option<String>,
    protocol: String,
    tags: Vec<String>,
}

impl From<&FlowRecord> for ExportFlow {
    fn from(f: &FlowRecord) -> Self {
        Self {
            id: f.id.clone(),
            started_at: f.started_at,
            ended_at: f.ended_at,
            duration_ms: f.duration_ms,
            client: f.client.clone(),
            scheme: f.scheme.clone(),
            host: f.host.clone(),
            port: f.port,
            method: f.method.clone(),
            path: f.path.clone(),
            query: f.query.clone(),
            http_version: f.http_version.clone(),
            status: f.status,
            reason: f.reason.clone(),
            request_headers: f.request_headers.clone(),
            response_headers: f.response_headers.clone(),
            request_trailers: f.request_trailers.clone(),
            response_trailers: f.response_trailers.clone(),
            request_body_base64: B64.encode(&f.request_body),
            response_body_base64: B64.encode(&f.response_body),
            request_body_size: f.request_body_size,
            response_body_size: f.response_body_size,
            request_truncated: f.request_truncated,
            response_truncated: f.response_truncated,
            request_mime: f.request_mime.clone(),
            response_mime: f.response_mime.clone(),
            error: f.error.clone(),
            tls_sni: f.tls_sni.clone(),
            protocol: f.protocol.clone(),
            tags: f.tags.clone(),
        }
    }
}

impl ExportFlow {
    fn into_record(self) -> Result<FlowRecord> {
        let request_body = B64
            .decode(self.request_body_base64.as_bytes())
            .context("invalid request_body_base64")?;
        let response_body = B64
            .decode(self.response_body_base64.as_bytes())
            .context("invalid response_body_base64")?;
        Ok(FlowRecord {
            id: self.id,
            started_at: self.started_at,
            ended_at: self.ended_at,
            duration_ms: self.duration_ms,
            client: self.client,
            scheme: self.scheme,
            host: self.host,
            port: self.port,
            method: self.method,
            path: self.path,
            query: self.query,
            http_version: self.http_version,
            status: self.status,
            reason: self.reason,
            request_headers: self.request_headers,
            response_headers: self.response_headers,
            request_trailers: self.request_trailers,
            response_trailers: self.response_trailers,
            request_body,
            response_body,
            request_body_size: self.request_body_size,
            response_body_size: self.response_body_size,
            request_truncated: self.request_truncated,
            response_truncated: self.response_truncated,
            request_mime: self.request_mime,
            response_mime: self.response_mime,
            error: self.error,
            tls_sni: self.tls_sni,
            protocol: self.protocol,
            tags: self.tags,
        })
    }
}

fn open(path: &Path) -> Result<Connection> {
    let conn =
        Connection::open(path).with_context(|| format!("open database {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS flows (
    seq                 INTEGER PRIMARY KEY AUTOINCREMENT,
    id                  TEXT NOT NULL UNIQUE,
    started_at          TEXT NOT NULL,
    ended_at            TEXT,
    duration_ms         INTEGER,
    client              TEXT NOT NULL,
    scheme              TEXT NOT NULL,
    host                TEXT NOT NULL,
    port                INTEGER NOT NULL,
    method              TEXT NOT NULL,
    path                TEXT NOT NULL,
    query               TEXT,
    http_version        TEXT NOT NULL,
    status              INTEGER,
    reason              TEXT,
    request_headers     TEXT NOT NULL,
    response_headers    TEXT NOT NULL,
    request_trailers    TEXT NOT NULL,
    response_trailers   TEXT NOT NULL,
    request_body        BLOB NOT NULL,
    response_body       BLOB NOT NULL,
    request_body_text   TEXT NOT NULL,
    response_body_text  TEXT NOT NULL,
    request_body_size   INTEGER NOT NULL,
    response_body_size  INTEGER NOT NULL,
    request_truncated   INTEGER NOT NULL,
    response_truncated  INTEGER NOT NULL,
    request_mime        TEXT,
    response_mime       TEXT,
    error               TEXT,
    tls_sni             TEXT,
    protocol            TEXT NOT NULL,
    tags                TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_flows_started ON flows(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_flows_host ON flows(host);
CREATE INDEX IF NOT EXISTS idx_flows_status ON flows(status);
CREATE INDEX IF NOT EXISTS idx_flows_method ON flows(method);
CREATE INDEX IF NOT EXISTS idx_flows_protocol ON flows(protocol);
"#,
    )?;
    Ok(())
}

fn mime_from_headers(headers: &[HeaderEntry]) -> Option<String> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .map(|h| {
            h.value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|s| !s.is_empty())
}

fn body_text(body: &[u8]) -> String {
    String::from_utf8_lossy(body).into_owned()
}

fn json_headers(headers: &[HeaderEntry]) -> String {
    serde_json::to_string(headers).unwrap_or_else(|_| "[]".into())
}

fn parse_headers(s: &str) -> Vec<HeaderEntry> {
    serde_json::from_str(s).unwrap_or_default()
}

fn parse_tags(s: &str) -> Vec<String> {
    serde_json::from_str(s).unwrap_or_default()
}

fn parse_time(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

impl Store {
    pub async fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let p = path.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = open(&p)?;
            init_schema(&conn)?;
            Ok(())
        })
        .await??;
        Ok(Self {
            db_path: Arc::new(path),
        })
    }

    pub fn db_path(&self) -> &Path {
        self.db_path.as_path()
    }

    pub async fn insert(&self, flow: FlowRecord) -> Result<i64> {
        let path = self.db_path.clone();
        tokio::task::spawn_blocking(move || -> Result<i64> {
            let conn = open(&path)?;
            let request_text = body_text(&flow.request_body);
            let response_text = body_text(&flow.response_body);
            conn.execute(
                r#"INSERT INTO flows (
                    id, started_at, ended_at, duration_ms, client, scheme, host, port, method, path, query,
                    http_version, status, reason, request_headers, response_headers, request_trailers,
                    response_trailers, request_body, response_body, request_body_text, response_body_text,
                    request_body_size, response_body_size, request_truncated, response_truncated,
                    request_mime, response_mime, error, tls_sni, protocol, tags
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32)"#,
                params![
                    flow.id,
                    flow.started_at.to_rfc3339(),
                    flow.ended_at.map(|d| d.to_rfc3339()),
                    flow.duration_ms,
                    flow.client,
                    flow.scheme,
                    flow.host,
                    flow.port as i64,
                    flow.method,
                    flow.path,
                    flow.query,
                    flow.http_version,
                    flow.status.map(|s| s as i64),
                    flow.reason,
                    json_headers(&flow.request_headers),
                    json_headers(&flow.response_headers),
                    json_headers(&flow.request_trailers),
                    json_headers(&flow.response_trailers),
                    flow.request_body,
                    flow.response_body,
                    request_text,
                    response_text,
                    flow.request_body_size as i64,
                    flow.response_body_size as i64,
                    flow.request_truncated as i64,
                    flow.response_truncated as i64,
                    flow.request_mime,
                    flow.response_mime,
                    flow.error,
                    flow.tls_sni,
                    flow.protocol,
                    serde_json::to_string(&flow.tags).unwrap_or_else(|_| "[]".into()),
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await?
    }

    pub async fn update_response(
        &self,
        id: String,
        status: u16,
        reason: String,
        headers: Vec<HeaderEntry>,
        trailers: Vec<HeaderEntry>,
        body: Vec<u8>,
        body_size: u64,
        truncated: bool,
        ended_at: DateTime<Utc>,
        duration_ms: i64,
        error: Option<String>,
    ) -> Result<()> {
        let path = self.db_path.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = open(&path)?;
            let text = body_text(&body);
            let mime = mime_from_headers(&headers);
            conn.execute(
                r#"UPDATE flows SET status=?2, reason=?3, response_headers=?4, response_trailers=?5,
                   response_body=?6, response_body_text=?7, response_body_size=?8, response_truncated=?9,
                   ended_at=?10, duration_ms=?11, error=?12, response_mime=?13 WHERE id=?1"#,
                params![
                    id,
                    status as i64,
                    reason,
                    json_headers(&headers),
                    json_headers(&trailers),
                    body,
                    text,
                    body_size as i64,
                    truncated as i64,
                    ended_at.to_rfc3339(),
                    duration_ms,
                    error,
                    mime,
                ],
            )?;
            Ok(())
        })
        .await?
    }

    pub async fn update_error(
        &self,
        id: String,
        error: String,
        ended_at: DateTime<Utc>,
        duration_ms: i64,
    ) -> Result<()> {
        let path = self.db_path.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = open(&path)?;
            conn.execute(
                "UPDATE flows SET error=?2, ended_at=?3, duration_ms=?4 WHERE id=?1",
                params![id, error, ended_at.to_rfc3339(), duration_ms],
            )?;
            Ok(())
        })
        .await?
    }

    pub async fn search(&self, query: &SearchQuery) -> Result<SearchResult> {
        let path = self.db_path.clone();
        let query = query.clone();
        tokio::task::spawn_blocking(move || search_blocking(&path, &query)).await?
    }

    pub async fn all_matching(&self, query: &SearchQuery, max: usize) -> Result<Vec<FlowRecord>> {
        let mut q = query.clone();
        q.limit = Some(max.min(20_000));
        q.offset = Some(0);
        let path = self.db_path.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<FlowRecord>> {
            let conn = open(&path)?;
            let (where_sql, params) = build_where(&q);
            let sql = format!(
                "SELECT * FROM flows {} ORDER BY seq {} LIMIT ? OFFSET ?",
                where_sql,
                if q.descending() { "DESC" } else { "ASC" }
            );
            let mut values = params;
            let limit = q.limit.unwrap_or(max).clamp(1, max.max(1));
            let offset = q.offset.unwrap_or(0);
            values.push(SqlValue::Integer(limit as i64));
            values.push(SqlValue::Integer(offset as i64));
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(values), row_to_record)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await?
    }

    pub async fn get(
        &self,
        id: &str,
        body_mode: &str,
        max_bytes: usize,
    ) -> Result<Option<FlowDetail>> {
        let path = self.db_path.clone();
        let id = id.to_string();
        let body_mode = body_mode.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<FlowDetail>> {
            let conn = open(&path)?;
            let rec = conn
                .query_row(
                    "SELECT * FROM flows WHERE id=?1",
                    params![id],
                    row_to_record,
                )
                .optional()?;
            let Some(rec) = rec else { return Ok(None) };
            let seq = conn
                .query_row("SELECT seq FROM flows WHERE id=?1", params![id], |r| {
                    r.get::<_, i64>(0)
                })
                .optional()?
                .unwrap_or(0);
            Ok(Some(build_detail(&rec, seq, &body_mode, max_bytes)))
        })
        .await?
    }

    pub async fn get_record(&self, id: &str) -> Result<Option<FlowRecord>> {
        let path = self.db_path.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<FlowRecord>> {
            let conn = open(&path)?;
            Ok(conn
                .query_row(
                    "SELECT * FROM flows WHERE id=?1",
                    params![id],
                    row_to_record,
                )
                .optional()?)
        })
        .await?
    }

    pub async fn get_summary(&self, id: &str) -> Result<Option<FlowSummary>> {
        let path = self.db_path.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<FlowSummary>> {
            let conn = open(&path)?;
            Ok(conn
                .query_row(
                    "SELECT seq,id,started_at,duration_ms,client,scheme,host,port,method,path,http_version,status,request_body_size,response_body_size,request_mime,response_mime,error,protocol,tags FROM flows WHERE id=?1",
                    params![id],
                    row_to_summary,
                )
                .optional()?)
        })
        .await?
    }

    pub async fn delete(&self, id: &str) -> Result<bool> {
        let path = self.db_path.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || -> Result<bool> {
            let conn = open(&path)?;
            let n = conn.execute("DELETE FROM flows WHERE id=?1", params![id])?;
            Ok(n > 0)
        })
        .await?
    }

    pub async fn clear(&self) -> Result<usize> {
        let path = self.db_path.clone();
        tokio::task::spawn_blocking(move || -> Result<usize> {
            let conn = open(&path)?;
            let n = conn.execute("DELETE FROM flows", [])?;
            conn.execute("DELETE FROM sqlite_sequence WHERE name='flows'", [])?;
            Ok(n)
        })
        .await?
    }

    pub async fn stats(&self) -> Result<Stats> {
        let path = self.db_path.clone();
        tokio::task::spawn_blocking(move || -> Result<Stats> {
            let conn = open(&path)?;
            let total: i64 = conn.query_row("SELECT COUNT(*) FROM flows", [], |r| r.get(0))?;
            let (req_bytes, resp_bytes): (i64, i64) = conn.query_row(
                "SELECT COALESCE(SUM(request_body_size),0), COALESCE(SUM(response_body_size),0) FROM flows",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let mut hosts = Vec::new();
            {
                let mut stmt = conn.prepare(
                    "SELECT host, COUNT(*) c FROM flows GROUP BY host ORDER BY c DESC, host ASC LIMIT 200",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok(HostCount {
                        host: r.get(0)?,
                        count: r.get(1)?,
                    })
                })?;
                for r in rows {
                    hosts.push(r?);
                }
            }
            let mut status_classes = Vec::new();
            {
                let mut stmt = conn.prepare(
                    "SELECT CASE WHEN status IS NULL THEN 'error' ELSE (status/100)||'xx' END cls, COUNT(*) c
                     FROM flows GROUP BY cls ORDER BY c DESC",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok(StatusClassCount {
                        class: r.get(0)?,
                        count: r.get(1)?,
                    })
                })?;
                for r in rows {
                    status_classes.push(r?);
                }
            }
            let first: Option<String> =
                conn.query_row("SELECT MIN(started_at) FROM flows", [], |r| r.get(0))?;
            let last: Option<String> =
                conn.query_row("SELECT MAX(started_at) FROM flows", [], |r| r.get(0))?;
            Ok(Stats {
                total,
                hosts,
                status_classes,
                bytes: ByteTotals {
                    request: req_bytes,
                    response: resp_bytes,
                },
                first: first.as_deref().map(parse_time),
                last: last.as_deref().map(parse_time),
            })
        })
        .await?
    }

    pub async fn export_json(&self, query: &SearchQuery, max: usize) -> Result<String> {
        let records = self.all_matching(query, max).await?;
        let bundle = ExportBundle {
            version: 1,
            app: format!("MitmRs {}", env!("CARGO_PKG_VERSION")),
            exported_at: Utc::now(),
            count: records.len(),
            flows: records.iter().map(ExportFlow::from).collect(),
        };
        Ok(serde_json::to_string_pretty(&bundle)?)
    }

    pub async fn export_har(&self, query: &SearchQuery, max: usize) -> Result<String> {
        let records = self.all_matching(query, max).await?;
        let entries: Vec<serde_json::Value> = records.iter().map(har_entry).collect();
        let har = json!({
            "log": {
                "version": "1.2",
                "creator": { "name": "MitmRs", "version": env!("CARGO_PKG_VERSION") },
                "pages": [],
                "entries": entries
            }
        });
        Ok(serde_json::to_string_pretty(&har)?)
    }

    pub async fn import_json(&self, text: &str) -> Result<ImportResult> {
        let mut records: Vec<FlowRecord> = Vec::new();
        let value: serde_json::Value = serde_json::from_str(text).context("invalid JSON")?;
        if value.is_array() {
            let arr: Vec<ExportFlow> = serde_json::from_value(value)?;
            for f in arr {
                records.push(f.into_record()?);
            }
        } else if value.get("flows").is_some() {
            let bundle: ExportBundle = serde_json::from_value(value)?;
            for f in bundle.flows {
                records.push(f.into_record()?);
            }
        } else if value.get("log").is_some() {
            records = import_har(value)?;
        } else {
            return Err(anyhow!(
                "unrecognized import format: expected MitmRs JSON, array, or HAR"
            ));
        }
        let total = records.len();
        let path = self.db_path.clone();
        let imported = tokio::task::spawn_blocking(move || -> Result<usize> {
            let mut conn = open(&path)?;
            let tx = conn.transaction()?;
            let mut imported = 0usize;
            for mut flow in records {
                if tx
                    .query_row("SELECT 1 FROM flows WHERE id=?1", params![flow.id], |_| Ok(()))
                    .optional()?
                    .is_some()
                {
                    flow.id = uuid::Uuid::now_v7().to_string();
                }
                tx.execute(
                    r#"INSERT INTO flows (
                        id, started_at, ended_at, duration_ms, client, scheme, host, port, method, path, query,
                        http_version, status, reason, request_headers, response_headers, request_trailers,
                        response_trailers, request_body, response_body, request_body_text, response_body_text,
                        request_body_size, response_body_size, request_truncated, response_truncated,
                        request_mime, response_mime, error, tls_sni, protocol, tags
                    ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32)"#,
                    params![
                        flow.id,
                        flow.started_at.to_rfc3339(),
                        flow.ended_at.map(|d| d.to_rfc3339()),
                        flow.duration_ms,
                        flow.client,
                        flow.scheme,
                        flow.host,
                        flow.port as i64,
                        flow.method,
                        flow.path,
                        flow.query,
                        flow.http_version,
                        flow.status.map(|s| s as i64),
                        flow.reason,
                        json_headers(&flow.request_headers),
                        json_headers(&flow.response_headers),
                        json_headers(&flow.request_trailers),
                        json_headers(&flow.response_trailers),
                        flow.request_body,
                        flow.response_body,
                        body_text(&flow.request_body),
                        body_text(&flow.response_body),
                        flow.request_body_size as i64,
                        flow.response_body_size as i64,
                        flow.request_truncated as i64,
                        flow.response_truncated as i64,
                        flow.request_mime,
                        flow.response_mime,
                        flow.error,
                        flow.tls_sni,
                        flow.protocol,
                        serde_json::to_string(&flow.tags).unwrap_or_else(|_| "[]".into()),
                    ],
                )?;
                imported += 1;
            }
            tx.commit()?;
            Ok(imported)
        })
        .await??;
        Ok(ImportResult {
            ok: true,
            imported,
            skipped: total.saturating_sub(imported),
        })
    }
}

fn build_where(q: &SearchQuery) -> (String, Vec<SqlValue>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut values: Vec<SqlValue> = Vec::new();
    if let Some(qtext) = q.q.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let like = format!("%{}%", escape_like(qtext));
        clauses.push(
            "(host LIKE ? ESCAPE '\\' OR path LIKE ? ESCAPE '\\' OR method LIKE ? ESCAPE '\\' OR request_body_text LIKE ? ESCAPE '\\' OR response_body_text LIKE ? ESCAPE '\\' OR request_headers LIKE ? ESCAPE '\\' OR response_headers LIKE ? ESCAPE '\\')".into(),
        );
        for _ in 0..7 {
            values.push(SqlValue::Text(like.clone()));
        }
    }
    if let Some(host) = q.host.as_ref().filter(|s| !s.is_empty()) {
        clauses.push("host LIKE ? ESCAPE '\\'".into());
        values.push(SqlValue::Text(format!("%{}%", escape_like(host))));
    }
    if let Some(method) = q.method.as_ref().filter(|s| !s.is_empty()) {
        clauses.push("UPPER(method) = ?".into());
        values.push(SqlValue::Text(method.to_ascii_uppercase()));
    }
    if let Some(status) = q
        .status
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        if status.eq_ignore_ascii_case("error") {
            clauses.push("(error IS NOT NULL OR status IS NULL OR status = 0)".into());
        } else if status.ends_with("xx") {
            if let Ok(cls) = status[..1].parse::<i64>() {
                clauses.push("status >= ? AND status <= ?".into());
                values.push(SqlValue::Integer(cls * 100));
                values.push(SqlValue::Integer(cls * 100 + 99));
            }
        } else if let Ok(code) = status.parse::<i64>() {
            clauses.push("status = ?".into());
            values.push(SqlValue::Integer(code));
        }
    }
    if let Some(protocol) = q.protocol.as_ref().filter(|s| !s.is_empty()) {
        let p = protocol.to_ascii_lowercase();
        if p == "grpc" {
            clauses.push(
                "(protocol = 'grpc' OR response_mime LIKE '%grpc%' OR request_mime LIKE '%grpc%')"
                    .into(),
            );
        } else if p == "http/2" || p == "h2" {
            clauses.push("http_version = 'HTTP/2.0'".into());
        } else if p == "http/1.1" || p == "h1" {
            clauses.push("http_version = 'HTTP/1.1'".into());
        } else if p == "other" {
            clauses.push("protocol NOT IN ('http','http2','grpc','websocket')".into());
        } else {
            clauses.push("protocol = ?".into());
            values.push(SqlValue::Text(p));
        }
    }
    if let Some(mime) = q.mime.as_ref().filter(|s| !s.is_empty()) {
        clauses
            .push("(request_mime LIKE ? ESCAPE '\\' OR response_mime LIKE ? ESCAPE '\\')".into());
        let like = format!("%{}%", escape_like(mime));
        values.push(SqlValue::Text(like.clone()));
        values.push(SqlValue::Text(like));
    }
    if let Some(needle) = q
        .body_contains
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        let like = format!("%{}%", escape_like(needle));
        let dir = q.direction.as_deref().unwrap_or("both");
        match dir {
            "request" => {
                clauses.push("request_body_text LIKE ? ESCAPE '\\'".into());
                values.push(SqlValue::Text(like));
            }
            "response" => {
                clauses.push("response_body_text LIKE ? ESCAPE '\\'".into());
                values.push(SqlValue::Text(like));
            }
            _ => {
                clauses.push("(request_body_text LIKE ? ESCAPE '\\' OR response_body_text LIKE ? ESCAPE '\\')".into());
                values.push(SqlValue::Text(like.clone()));
                values.push(SqlValue::Text(like));
            }
        }
    }
    if let Some(since) = q.since.as_deref().and_then(parse_time_loose) {
        clauses.push("started_at >= ?".into());
        values.push(SqlValue::Text(since.to_rfc3339()));
    }
    if let Some(until) = q.until.as_deref().and_then(parse_time_loose) {
        clauses.push("started_at <= ?".into());
        values.push(SqlValue::Text(until.to_rfc3339()));
    }
    if q.has_error == Some(true) {
        clauses.push("error IS NOT NULL".into());
    }
    if let Some(tag) = q.tag.as_ref().filter(|s| !s.is_empty()) {
        clauses.push("tags LIKE ? ESCAPE '\\'".into());
        values.push(SqlValue::Text(format!("%\"{}\"%", escape_like(tag))));
    }
    if clauses.is_empty() {
        (String::new(), values)
    } else {
        (format!("WHERE {}", clauses.join(" AND ")), values)
    }
}

fn parse_time_loose(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
                .ok()
                .map(|d| d.and_utc())
        })
        .or_else(|| {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|d| d.and_utc())
        })
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn search_blocking(path: &Path, q: &SearchQuery) -> Result<SearchResult> {
    let conn = open(path)?;
    let (where_sql, params) = build_where(q);
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM flows {}", where_sql),
        params_from_iter(params.iter().cloned()),
        |r| r.get(0),
    )?;
    let sql = format!(
        "SELECT seq,id,started_at,duration_ms,client,scheme,host,port,method,path,http_version,status,request_body_size,response_body_size,request_mime,response_mime,error,protocol,tags FROM flows {} ORDER BY seq {} LIMIT ? OFFSET ?",
        where_sql,
        if q.descending() { "DESC" } else { "ASC" }
    );
    let mut values = params;
    values.push(SqlValue::Integer(q.normalized_limit() as i64));
    values.push(SqlValue::Integer(q.normalized_offset() as i64));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), row_to_summary)?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row?);
    }
    Ok(SearchResult {
        items,
        total,
        limit: q.normalized_limit(),
        offset: q.normalized_offset(),
    })
}

fn row_to_summary(r: &Row<'_>) -> rusqlite::Result<FlowSummary> {
    let started: String = r.get(2)?;
    Ok(FlowSummary {
        id: r.get(1)?,
        seq: r.get(0)?,
        started_at: parse_time(&started),
        duration_ms: r.get(3)?,
        client: r.get(4)?,
        scheme: r.get(5)?,
        host: r.get(6)?,
        port: r.get::<_, i64>(7)? as u16,
        method: r.get(8)?,
        path: r.get(9)?,
        http_version: r.get(10)?,
        status: r.get::<_, Option<i64>>(11)?.map(|v| v as u16),
        request_size: r.get::<_, i64>(12)? as u64,
        response_size: r.get::<_, i64>(13)? as u64,
        request_mime: r.get(14)?,
        response_mime: r.get(15)?,
        error: r.get(16)?,
        protocol: r.get(17)?,
        tags: parse_tags(&r.get::<_, String>(18)?),
    })
}

fn row_to_record(r: &Row<'_>) -> rusqlite::Result<FlowRecord> {
    let started: String = r.get("started_at")?;
    let ended: Option<String> = r.get("ended_at")?;
    Ok(FlowRecord {
        id: r.get("id")?,
        started_at: parse_time(&started),
        ended_at: ended.as_deref().map(parse_time),
        duration_ms: r.get("duration_ms")?,
        client: r.get("client")?,
        scheme: r.get("scheme")?,
        host: r.get("host")?,
        port: r.get::<_, i64>("port")? as u16,
        method: r.get("method")?,
        path: r.get("path")?,
        query: r.get("query")?,
        http_version: r.get("http_version")?,
        status: r.get::<_, Option<i64>>("status")?.map(|v| v as u16),
        reason: r.get("reason")?,
        request_headers: parse_headers(&r.get::<_, String>("request_headers")?),
        response_headers: parse_headers(&r.get::<_, String>("response_headers")?),
        request_trailers: parse_headers(&r.get::<_, String>("request_trailers")?),
        response_trailers: parse_headers(&r.get::<_, String>("response_trailers")?),
        request_body: r.get("request_body")?,
        response_body: r.get("response_body")?,
        request_body_size: r.get::<_, i64>("request_body_size")? as u64,
        response_body_size: r.get::<_, i64>("response_body_size")? as u64,
        request_truncated: r.get::<_, i64>("request_truncated")? != 0,
        response_truncated: r.get::<_, i64>("response_truncated")? != 0,
        request_mime: r.get("request_mime")?,
        response_mime: r.get("response_mime")?,
        error: r.get("error")?,
        tls_sni: r.get("tls_sni")?,
        protocol: r.get("protocol")?,
        tags: parse_tags(&r.get::<_, String>("tags")?),
    })
}

fn build_detail(rec: &FlowRecord, seq: i64, body_mode: &str, max_bytes: usize) -> FlowDetail {
    FlowDetail {
        summary: rec.summary(seq),
        request: body_view(
            &rec.request_headers,
            &rec.request_trailers,
            &rec.request_body,
            rec.request_body_size,
            rec.request_truncated,
            rec.request_mime.as_deref(),
            body_mode,
            max_bytes,
        ),
        response: body_view(
            &rec.response_headers,
            &rec.response_trailers,
            &rec.response_body,
            rec.response_body_size,
            rec.response_truncated,
            rec.response_mime.as_deref(),
            body_mode,
            max_bytes,
        ),
        tls_sni: rec.tls_sni.clone(),
        error_detail: rec.error.clone(),
    }
}

fn body_view(
    headers: &[HeaderEntry],
    trailers: &[HeaderEntry],
    body: &[u8],
    body_size: u64,
    truncated: bool,
    content_type: Option<&str>,
    mode: &str,
    max_bytes: usize,
) -> BodyView {
    let take = body.len().min(max_bytes);
    let slice = &body[..take];
    let (body_base64, body_text) = match mode {
        "none" => (None, None),
        "base64" => (Some(B64.encode(slice)), None),
        "both" => (
            Some(B64.encode(slice)),
            Some(String::from_utf8_lossy(slice).into_owned()),
        ),
        _ => (None, Some(String::from_utf8_lossy(slice).into_owned())),
    };
    let encoding = headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-encoding"))
        .map(|h| h.value.clone());
    BodyView {
        headers: headers.to_vec(),
        trailers: trailers.to_vec(),
        body_size,
        truncated: truncated || take < body.len(),
        body_base64,
        body_text,
        body_encoding: encoding,
        content_type: content_type.map(|s| s.to_string()),
    }
}

fn har_entry(rec: &FlowRecord) -> serde_json::Value {
    let url = format!(
        "{}://{}{}",
        rec.scheme,
        if (rec.scheme == "https" && rec.port == 443) || (rec.scheme == "http" && rec.port == 80) {
            rec.host.clone()
        } else {
            format!("{}:{}", rec.host, rec.port)
        },
        rec.path
    );
    let req_headers: Vec<serde_json::Value> = rec
        .request_headers
        .iter()
        .map(|h| json!({"name": h.name, "value": h.value}))
        .collect();
    let resp_headers: Vec<serde_json::Value> = rec
        .response_headers
        .iter()
        .map(|h| json!({"name": h.name, "value": h.value}))
        .collect();
    let req_text = String::from_utf8_lossy(&rec.request_body).into_owned();
    let resp_binary = !is_probably_text(&rec.response_body);
    let resp_text = if resp_binary {
        B64.encode(&rec.response_body)
    } else {
        String::from_utf8_lossy(&rec.response_body).into_owned()
    };
    let req_mime = rec.request_mime.clone().unwrap_or_default();
    let resp_mime = rec.response_mime.clone().unwrap_or_default();
    let request = if rec.request_body.is_empty() {
        json!({
            "method": rec.method,
            "url": url,
            "httpVersion": rec.http_version,
            "headers": req_headers,
            "queryString": [],
            "cookies": [],
            "headersSize": -1,
            "bodySize": rec.request_body_size,
        })
    } else {
        json!({
            "method": rec.method,
            "url": url,
            "httpVersion": rec.http_version,
            "headers": req_headers,
            "queryString": [],
            "cookies": [],
            "headersSize": -1,
            "bodySize": rec.request_body_size,
            "postData": { "mimeType": req_mime, "text": req_text }
        })
    };
    let response = json!({
        "status": rec.status.unwrap_or(0),
        "statusText": rec.reason.clone().unwrap_or_default(),
        "httpVersion": rec.http_version,
        "headers": resp_headers,
        "cookies": [],
        "content": {
            "size": rec.response_body_size,
            "mimeType": resp_mime,
            "text": resp_text,
            "encoding": if resp_binary { "base64" } else { "" }
        },
        "redirectURL": "",
        "headersSize": -1,
        "bodySize": rec.response_body_size,
    });
    json!({
        "startedDateTime": rec.started_at.to_rfc3339(),
        "time": rec.duration_ms.unwrap_or(0),
        "request": request,
        "response": response,
        "cache": {},
        "timings": { "send": 0, "wait": rec.duration_ms.unwrap_or(0), "receive": 0 },
        "serverIPAddress": rec.host,
        "connection": rec.client,
        "comment": rec.error.clone().unwrap_or_default(),
    })
}

fn import_har(value: serde_json::Value) -> Result<Vec<FlowRecord>> {
    let entries = value
        .pointer("/log/entries")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("HAR is missing log.entries"))?;
    let mut out = Vec::new();
    for e in entries {
        let req = &e["request"];
        let resp = &e["response"];
        let url_str = req["url"].as_str().unwrap_or("");
        let url = url::Url::parse(url_str).ok();
        let scheme = url
            .as_ref()
            .map(|u| u.scheme().to_string())
            .unwrap_or_else(|| "http".into());
        let host = url
            .as_ref()
            .and_then(|u| u.host_str().map(|s| s.to_string()))
            .unwrap_or_default();
        let port = url
            .as_ref()
            .and_then(|u| u.port_or_known_default())
            .unwrap_or(if scheme == "https" { 443 } else { 80 });
        let path = url
            .as_ref()
            .map(|u| {
                let mut p = u.path().to_string();
                if let Some(q) = u.query() {
                    p.push('?');
                    p.push_str(q);
                }
                p
            })
            .unwrap_or_else(|| "/".into());
        let started = e["startedDateTime"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let request_headers = har_headers(&req["headers"]);
        let response_headers = har_headers(&resp["headers"]);
        let request_body = req
            .pointer("/postData/text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .as_bytes()
            .to_vec();
        let resp_text = resp
            .pointer("/content/text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let response_body = if resp
            .pointer("/content/encoding")
            .and_then(|v| v.as_str())
            .map(|e| e.eq_ignore_ascii_case("base64"))
            .unwrap_or(false)
        {
            B64.decode(resp_text.as_bytes()).unwrap_or_default()
        } else {
            resp_text.as_bytes().to_vec()
        };
        let status = resp["status"].as_u64().map(|s| s as u16);
        let mut rec = FlowRecord::new(
            uuid::Uuid::now_v7().to_string(),
            started,
            "imported".into(),
            scheme,
            host,
            port,
            req["method"].as_str().unwrap_or("GET").to_string(),
            path,
            None,
            req["httpVersion"]
                .as_str()
                .unwrap_or("HTTP/1.1")
                .to_string(),
            None,
        );
        rec.request_headers = request_headers;
        rec.response_headers = response_headers;
        rec.request_body_size = request_body.len() as u64;
        rec.response_body_size = response_body.len() as u64;
        rec.request_mime = mime_from_headers(&rec.request_headers);
        rec.response_mime = mime_from_headers(&rec.response_headers);
        rec.request_body = request_body;
        rec.response_body = response_body;
        rec.status = status;
        rec.reason = resp["statusText"].as_str().map(|s| s.to_string());
        rec.duration_ms = e["time"].as_i64();
        out.push(rec);
    }
    Ok(out)
}

fn har_headers(v: &serde_json::Value) -> Vec<HeaderEntry> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|h| {
                    Some(HeaderEntry {
                        name: h["name"].as_str()?.to_string(),
                        value: h["value"].as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}
