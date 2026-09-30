use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Utc;
use hmac::{Hmac, Mac};
use reqwest::Client;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, Transport};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::{sqlite::SqliteConnectOptions, Row, SqlitePool};
use std::{
    env,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};
use tokio::time::sleep;
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};
use urlencoding::encode as url_encode;
use uuid::Uuid;

const DASHBOARD: &str = include_str!("../index.html");
const ARABICA_LABELS: &[&str] = &[
    "Arabica Mandheling (Sumatra)",
    "Arabica Java Preanger",
    "Arabica Aceh Gayo",
    "Arabica Flores",
    "Arabica IJEN",
    "Arabica Cakrabuna Tasikmalaya",
    "Arabica Commercial Espresso Asli",
    "Arabica Kalosi Bonebone",
];
const COSMOS_API_VERSION: &str = "2018-12-31";

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct AppState {
    db: SqlitePool,
    mqtt_connected: Arc<AtomicBool>,
    cosmos: Option<Arc<CosmosConfig>>,
    cosmos_connected: Arc<AtomicBool>,
}

struct CosmosConfig {
    endpoint: String,
    key: String,
    database: String,
    container: String,
    client: Client,
}

impl CosmosConfig {
    fn from_env() -> anyhow::Result<Option<Self>> {
        let endpoint = env::var("COSMOS_ENDPOINT").unwrap_or_default().trim().to_string();
        let key = env::var("COSMOS_KEY").unwrap_or_default().trim().to_string();
        let database = env::var("COSMOS_DATABASE").unwrap_or_default().trim().to_string();
        let container = env::var("COSMOS_CONTAINER").unwrap_or_default().trim().to_string();

        let values = [&endpoint, &key, &database, &container];
        if values.iter().all(|value| value.is_empty()) {
            return Ok(None);
        }
        if values.iter().any(|value| value.is_empty()) {
            anyhow::bail!("Konfigurasi Cosmos tidak lengkap. Isi COSMOS_ENDPOINT, COSMOS_KEY, COSMOS_DATABASE, dan COSMOS_CONTAINER di .env.");
        }
        if !endpoint.starts_with("https://") {
            anyhow::bail!("COSMOS_ENDPOINT harus memakai https://");
        }
        STANDARD
            .decode(&key)
            .map_err(|_| anyhow::anyhow!("COSMOS_KEY bukan account key Base64 yang valid"))?;

        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;

        Ok(Some(Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            key,
            database,
            container,
            client,
        }))
    }

    async fn upsert_document(&self, document: &Value) -> anyhow::Result<()> {
        let id = document
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Dokumen Cosmos tidak memiliki field id string"))?;
        let device_id = document
            .get("device_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Dokumen Cosmos tidak memiliki field device_id string"))?;

        // Partition key container disetel ke /device_id. Header harus berupa JSON array.
        let partition_key_header = serde_json::to_string(&json!([device_id]))?;
        let resource_link = format!("dbs/{}/colls/{}", self.database, self.container);
        let url = format!(
            "{}/dbs/{}/colls/{}/docs",
            self.endpoint,
            url_encode(&self.database),
            url_encode(&self.container)
        );
        let date = httpdate::fmt_http_date(SystemTime::now());
        let authorization = self.authorization("POST", "docs", &resource_link, &date)?;

        let response = self
            .client
            .post(url)
            .header("authorization", authorization)
            .header("x-ms-date", date.as_str())
            .header("x-ms-version", COSMOS_API_VERSION)
            .header("x-ms-documentdb-is-upsert", "true")
            .header("x-ms-documentdb-partitionkey", partition_key_header)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json")
            .json(document)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Cosmos DB menolak dokumen {id} (HTTP {status}): {body}");
        }
        Ok(())
    }

    fn authorization(
        &self,
        verb: &str,
        resource_type: &str,
        resource_link: &str,
        date: &str,
    ) -> anyhow::Result<String> {
        let key_bytes = STANDARD
            .decode(&self.key)
            .map_err(|_| anyhow::anyhow!("COSMOS_KEY bukan account key Base64 yang valid"))?;
        let string_to_sign = format!(
            "{}\n{}\n{}\n{}\n\n",
            verb.to_ascii_lowercase(),
            resource_type.to_ascii_lowercase(),
            resource_link,
            date.to_ascii_lowercase()
        );
        let mut mac = HmacSha256::new_from_slice(&key_bytes)
            .map_err(|_| anyhow::anyhow!("Gagal menyiapkan HMAC untuk Cosmos DB"))?;
        mac.update(string_to_sign.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        let token = format!("type=master&ver=1.0&sig={signature}");
        Ok(url_encode(&token).into_owned())
    }
}

#[derive(Debug, Serialize)]
struct Reading {
    id: String,
    device_id: String,
    timestamp: String,
    prediction: String,
    confidence: f64,
    sensors: Value,
    feedback: Option<String>,
    corrected_label: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeedbackRequest {
    reading_id: String,
    verdict: String,
    actual_label: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            env::var("RUST_LOG")
                .unwrap_or_else(|_| "aromalab_dashboard=info,tower_http=info".into()),
        )
        .init();

    let db_path = env::var("DATABASE_PATH").unwrap_or_else(|_| "aromalab.sqlite".to_string());
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{db_path}"))?.create_if_missing(true);
    let db = SqlitePool::connect_with(options).await?;
    initialize_database(&db).await?;

    let cosmos = CosmosConfig::from_env()?.map(Arc::new);
    let state = AppState {
        db,
        mqtt_connected: Arc::new(AtomicBool::new(false)),
        cosmos_connected: Arc::new(AtomicBool::new(false)),
        cosmos,
    };

    if state.cosmos.is_some() {
        info!("Cosmos DB diaktifkan; dokumen akan dikirim lewat outbox dengan retry.");
        tokio::spawn(run_cosmos_sync(state.clone()));
    } else {
        info!("Cosmos DB belum dikonfigurasi; data disimpan ke SQLite lokal saja.");
    }

    if mqtt_configured() {
        tokio::spawn(run_mqtt(state.clone()));
    } else {
        info!("MQTT belum dikonfigurasi; dashboard tetap berjalan dalam mode lokal.");
    }

    let app = Router::new()
        .route("/", get(index))
        .route("/api/dashboard", get(get_dashboard))
        .route("/api/feedback", post(save_feedback))
        .route("/health", get(|| async { "ok" }))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let address = env::var("APP_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    info!("AromaLab dashboard listening on http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn initialize_database(db: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS readings (
            id TEXT PRIMARY KEY,
            device_id TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            prediction TEXT NOT NULL,
            confidence REAL NOT NULL,
            sensors_json TEXT NOT NULL,
            feedback TEXT NULL CHECK(feedback IN ('correct', 'incorrect') OR feedback IS NULL),
            corrected_label TEXT NULL,
            raw_payload TEXT NOT NULL
        )",
    )
    .execute(db)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_readings_timestamp ON readings(timestamp DESC)")
        .execute(db)
        .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS cosmos_outbox (
            id TEXT PRIMARY KEY,
            document_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        )",
    )
    .execute(db)
    .await?;
    Ok(())
}

async fn index() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], Html(DASHBOARD))
}

async fn get_dashboard(State(state): State<AppState>) -> Result<Json<Value>, (StatusCode, String)> {
    let rows = sqlx::query(
        "SELECT id, device_id, timestamp, prediction, confidence, sensors_json, feedback, corrected_label
         FROM readings ORDER BY timestamp DESC LIMIT 100",
    )
    .fetch_all(&state.db)
    .await
    .map_err(internal_error)?;

    let totals = sqlx::query(
        "SELECT COUNT(*) AS total,
            SUM(CASE WHEN feedback = 'correct' THEN 1 ELSE 0 END) AS verified_correct,
            SUM(CASE WHEN feedback = 'incorrect' THEN 1 ELSE 0 END) AS verified_incorrect,
            AVG(confidence) AS average_confidence
         FROM readings",
    )
    .fetch_one(&state.db)
    .await
    .map_err(internal_error)?;
    let total: i64 = totals.try_get("total").map_err(internal_error)?;
    let verified_correct: i64 = totals
        .try_get::<Option<i64>, _>("verified_correct")
        .map_err(internal_error)?
        .unwrap_or(0);
    let verified_incorrect: i64 = totals
        .try_get::<Option<i64>, _>("verified_incorrect")
        .map_err(internal_error)?
        .unwrap_or(0);
    let average_confidence: Option<f64> = totals
        .try_get("average_confidence")
        .map_err(internal_error)?;

    let mut readings = Vec::with_capacity(rows.len());
    for row in rows {
        let sensors_text: String = row.try_get("sensors_json").map_err(internal_error)?;
        let sensors = serde_json::from_str(&sensors_text).unwrap_or(Value::Null);
        readings.push(Reading {
            id: row.try_get("id").map_err(internal_error)?,
            device_id: row.try_get("device_id").map_err(internal_error)?,
            timestamp: row.try_get("timestamp").map_err(internal_error)?,
            prediction: row.try_get("prediction").map_err(internal_error)?,
            confidence: row.try_get("confidence").map_err(internal_error)?,
            sensors,
            feedback: row.try_get("feedback").map_err(internal_error)?,
            corrected_label: row.try_get("corrected_label").map_err(internal_error)?,
        });
    }

    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cosmos_outbox")
        .fetch_one(&state.db)
        .await
        .map_err(internal_error)?;

    Ok(Json(json!({
        "readings": readings,
        "summary": {
            "total_readings": total,
            "verified_correct": verified_correct,
            "verified_incorrect": verified_incorrect,
            "average_confidence": average_confidence.unwrap_or(0.0)
        },
        "mqtt_connected": state.mqtt_connected.load(Ordering::Relaxed),
        "cosmos_configured": state.cosmos.is_some(),
        "cosmos_connected": state.cosmos_connected.load(Ordering::Relaxed),
        "cosmos_pending": pending,
        "storage": "SQLite lokal"
    })))
}

async fn save_feedback(
    State(state): State<AppState>,
    Json(input): Json<FeedbackRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if input.verdict != "correct" && input.verdict != "incorrect" {
        return Err((StatusCode::BAD_REQUEST, "verdict harus 'correct' atau 'incorrect'".into()));
    }
    let actual_label = if input.verdict == "incorrect" {
        let label = input.actual_label.as_deref().unwrap_or_default().trim();
        if !ARABICA_LABELS.contains(&label) {
            return Err((StatusCode::BAD_REQUEST, "actual_label harus berupa label Arabika yang tersedia".into()));
        }
        Some(label.to_string())
    } else {
        None
    };

    let device_id: Option<String> = sqlx::query_scalar("SELECT device_id FROM readings WHERE id = ?")
        .bind(&input.reading_id)
        .fetch_optional(&state.db)
        .await
        .map_err(internal_error)?;
    let device_id = device_id.ok_or_else(|| (StatusCode::NOT_FOUND, "reading_id tidak ditemukan".to_string()))?;

    let feedback_doc = json!({
        "id": format!("feedback-{}", input.reading_id),
        "document_type": "feedback",
        "device_id": device_id,
        "reading_id": input.reading_id,
        "verdict": input.verdict,
        "actual_label": actual_label,
        "timestamp": Utc::now().to_rfc3339()
    });

    let mut tx = state.db.begin().await.map_err(internal_error)?;
    let result = sqlx::query("UPDATE readings SET feedback = ?, corrected_label = ? WHERE id = ?")
        .bind(&input.verdict)
        .bind(&actual_label)
        .bind(&input.reading_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_error)?;
    if result.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, "reading_id tidak ditemukan".into()));
    }
    if state.cosmos.is_some() {
        enqueue_cosmos_document(&mut tx, &format!("feedback:{}", input.reading_id), &feedback_doc)
            .await
            .map_err(internal_error)?;
    }
    tx.commit().await.map_err(internal_error)?;

    Ok(Json(json!({
        "ok": true,
        "reading_id": input.reading_id,
        "verdict": input.verdict,
        "actual_label": actual_label,
        "cosmos_sync_queued": state.cosmos.is_some()
    })))
}

fn internal_error(error: sqlx::Error) -> (StatusCode, String) {
    error!("database error: {error}");
    (StatusCode::INTERNAL_SERVER_ERROR, "kesalahan database".to_string())
}

fn mqtt_configured() -> bool {
    env::var("MQTT_HOST").map(|x| !x.trim().is_empty()).unwrap_or(false)
        && env::var("MQTT_USERNAME").map(|x| !x.trim().is_empty()).unwrap_or(false)
        && env::var("MQTT_PASSWORD").map(|x| !x.trim().is_empty()).unwrap_or(false)
}

async fn run_mqtt(state: AppState) {
    let host = env::var("MQTT_HOST").expect("checked MQTT_HOST");
    let port = env::var("MQTT_PORT").ok().and_then(|x| x.parse().ok()).unwrap_or(8883);
    let username = env::var("MQTT_USERNAME").expect("checked MQTT_USERNAME");
    let password = env::var("MQTT_PASSWORD").expect("checked MQTT_PASSWORD");
    let topic = env::var("MQTT_TOPIC").unwrap_or_else(|_| "aromalab/esp32/prediction".to_string());
    let client_id = env::var("MQTT_CLIENT_ID").unwrap_or_else(|_| "aromalab-dashboard-rust".to_string());

    let mut mqtt = MqttOptions::new(client_id, host, port);
    mqtt.set_credentials(username, password);
    mqtt.set_keep_alive(Duration::from_secs(30));
    mqtt.set_transport(Transport::tls_with_default_config());
    let (client, mut eventloop) = AsyncClient::new(mqtt, 32);
    if let Err(e) = client.subscribe(&topic, QoS::AtLeastOnce).await {
        warn!("MQTT subscribe belum berhasil: {e}");
    }
    info!("MQTT subscriber dimulai untuk topic {topic} (TLS)");

    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                state.mqtt_connected.store(true, Ordering::Relaxed);
                info!("terhubung ke HiveMQ melalui MQTT TLS");
                if let Err(e) = client.subscribe(&topic, QoS::AtLeastOnce).await {
                    warn!("subscribe gagal: {e}");
                }
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                match serde_json::from_slice::<Value>(&publish.payload) {
                    Ok(payload) => {
                        match store_payload(&state, payload).await {
                            Ok(_) => {}
                            Err(e) => error!("gagal menyimpan payload MQTT: {e}"),
                        }
                    }
                    Err(e) => warn!("payload MQTT bukan JSON valid: {e}"),
                }
            }
            Ok(_) => {}
            Err(e) => {
                state.mqtt_connected.store(false, Ordering::Relaxed);
                warn!("MQTT koneksi terputus/gagal: {e}; mencoba kembali");
                sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

async fn store_payload(state: &AppState, payload: Value) -> anyhow::Result<()> {
    let prediction = string_at(&payload, &["prediction", "predicted_label", "class", "label"])
        .or_else(|| payload.get("result").and_then(|r| string_at(r, &["prediction", "label", "class"])))
        .unwrap_or_else(|| "Tidak diketahui".to_string());
    let device_id = string_at(&payload, &["device_id", "deviceId"])
        .unwrap_or_else(|| "ESP32-01".to_string());
    let id = string_at(&payload, &["reading_id", "event_id"])
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let timestamp = string_at(&payload, &["timestamp", "ts", "created_at"])
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    let confidence = number_at(&payload, &["confidence", "probability", "score"])
        .or_else(|| payload.get("result").and_then(|r| number_at(r, &["confidence", "probability", "score"])))
        .unwrap_or(0.0);
    let sensors = payload.get("sensors")
        .or_else(|| payload.get("sensor_values"))
        .or_else(|| payload.get("sensorValues"))
        .or_else(|| payload.get("values"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let cloud_doc = json!({
        "id": id,
        "document_type": "reading",
        "device_id": device_id,
        "timestamp": timestamp,
        "prediction": prediction,
        "confidence": confidence,
        "sensors": sensors,
        "raw_payload": payload
    });
    let sensors_json = serde_json::to_string(&sensors)?;
    let raw = serde_json::to_string(&payload)?;

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT OR IGNORE INTO readings (id, device_id, timestamp, prediction, confidence, sensors_json, raw_payload)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&device_id)
    .bind(&timestamp)
    .bind(&prediction)
    .bind(confidence)
    .bind(sensors_json)
    .bind(raw)
    .execute(&mut *tx)
    .await?;

    if state.cosmos.is_some() {
        enqueue_cosmos_document(&mut tx, &format!("reading:{id}"), &cloud_doc).await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn enqueue_cosmos_document(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    outbox_id: &str,
    document: &Value,
) -> Result<(), sqlx::Error> {
    let document_json = serde_json::to_string(document).unwrap_or_else(|_| "{}".to_string());
    sqlx::query(
        "INSERT INTO cosmos_outbox (id, document_json, created_at) VALUES (?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET document_json = excluded.document_json, created_at = excluded.created_at",
    )
    .bind(outbox_id)
    .bind(document_json)
    .bind(Utc::now().to_rfc3339())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn run_cosmos_sync(state: AppState) {
    let cosmos = match state.cosmos.clone() {
        Some(config) => config,
        None => return,
    };

    loop {
        let pending = sqlx::query("SELECT id, document_json FROM cosmos_outbox ORDER BY created_at LIMIT 25")
            .fetch_all(&state.db)
            .await;
        let rows = match pending {
            Ok(rows) => rows,
            Err(e) => {
                state.cosmos_connected.store(false, Ordering::Relaxed);
                error!("gagal membaca antrean Cosmos DB: {e}");
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        for row in rows {
            let outbox_id: String = match row.try_get("id") {
                Ok(value) => value,
                Err(e) => {
                    error!("gagal membaca id outbox: {e}");
                    continue;
                }
            };
            let document_text: String = match row.try_get("document_json") {
                Ok(value) => value,
                Err(e) => {
                    error!("gagal membaca dokumen outbox: {e}");
                    continue;
                }
            };
            let document: Value = match serde_json::from_str(&document_text) {
                Ok(value) => value,
                Err(e) => {
                    error!("JSON outbox {outbox_id} tidak valid: {e}");
                    continue;
                }
            };

            match cosmos.upsert_document(&document).await {
                Ok(()) => {
                    if let Err(e) = sqlx::query("DELETE FROM cosmos_outbox WHERE id = ?")
                        .bind(&outbox_id)
                        .execute(&state.db)
                        .await
                    {
                        error!("Cosmos berhasil menerima {outbox_id}, tetapi outbox gagal dihapus: {e}");
                    }
                    state.cosmos_connected.store(true, Ordering::Relaxed);
                    info!("dokumen tersinkron ke Cosmos DB: {outbox_id}");
                }
                Err(e) => {
                    state.cosmos_connected.store(false, Ordering::Relaxed);
                    error!("sinkronisasi Cosmos gagal untuk {outbox_id}: {e}");
                    break;
                }
            }
        }
        sleep(Duration::from_secs(5)).await;
    }
}

fn string_at(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| value.get(*key).and_then(Value::as_str).map(ToOwned::to_owned))
}

fn number_at(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| value.get(*key).and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok())))
}
