use esp_idf_hal::peripherals::Peripherals;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::mqtt::client::{EspMqttClient, EventPayload, MqttClientConfiguration};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::ota::EspOta;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use embedded_io::Write as IoWrite;
use log::info;
use smart_leds::{SmartLedsWrite, RGB8};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;
use ws2812_esp32_rmt_driver::Ws2812Esp32Rmt;

// =====================================================================
// KONFIGURASI
// =====================================================================

const WIFI_SSID: &str = "Papale";
const WIFI_PASS: &str = "12345678";

const MQTT_HOST: &str = "9fd45ca183ff4259bc0ee557d5e2ae7c.s1.eu.hivemq.cloud";
const MQTT_PORT: u16 = 8883;
const MQTT_USERNAME: &str = "esqy123";
const MQTT_PASSWORD: &str = "esqysatusampe8";
const MQTT_TOPIC: &str = "aromalab/esp32/prediction";
const MQTT_OTA_TOPIC: &str = "aromalab/esp32/ota";

const DEVICE_ID: &str = "esp32-s3-01";
const FIRMWARE_VERSION: &str = "1.0.7-WHITE";

// =====================================================================

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let app_thread = std::thread::Builder::new()
        .name("white_ota_app".to_string())
        .stack_size(20 * 1024)
        .spawn(|| -> anyhow::Result<()> {
            run_white_app()
        })?;

    app_thread.join().unwrap()?;
    Ok(())
}

fn run_white_app() -> anyhow::Result<()> {
    info!("=====================================================");
    info!(" ESP32-S3 FIRMWARE v{} - LAMPU PUTIH", FIRMWARE_VERSION);
    info!("=====================================================");

    let peripherals = Peripherals::take().unwrap();
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // 1. Inisialisasi WS2812 RGB LED (GPIO 48) dan nyalakan WARNA PUTIH
    info!("Menyalakan Onboard RGB LED (GPIO 48) warna PUTIH...");
    let mut ws2812 = Ws2812Esp32Rmt::new(peripherals.rmt.channel0, peripherals.pins.gpio48)?;
    
    // Warna Putih Bersih (R: 60, G: 60, B: 60 - kecerahan seimbang & nyaman di mata)
    let white_color = RGB8 { r: 60, g: 60, b: 60 };
    ws2812.write([white_color].iter().cloned())?;
    info!(">>> LAMPU PUTIH AKTIF! <<<");

    // 2. Setup Wi-Fi
    info!("Menghubungkan ke Wi-Fi '{}'...", WIFI_SSID);
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs))?,
        sys_loop,
    )?;

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: WIFI_SSID.try_into().unwrap(),
        password: WIFI_PASS.try_into().unwrap(),
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    }))?;

    wifi.start()?;
    unsafe {
        esp_idf_svc::sys::esp_wifi_set_ps(esp_idf_svc::sys::wifi_ps_type_t_WIFI_PS_NONE);
    }
    wifi.connect()?;
    wifi.wait_netif_up()?;
    info!("Wi-Fi Terhubung! IP: {:?}", wifi.wifi().sta_netif().get_ip_info()?);

    // 3. MQTT Callback Client (Siap menerima OTA update kapan saja)
    let broker_url = format!(
        "mqtts://{}:{}@{}:{}",
        MQTT_USERNAME, MQTT_PASSWORD, MQTT_HOST, MQTT_PORT
    );
    let mqtt_config = MqttClientConfiguration {
        client_id: Some("esp32-s3-white-01"),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };

    let ota_url: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let ota_url_cb = ota_url.clone();
    let connected = Arc::new(AtomicBool::new(false));
    let connected_cb = connected.clone();

    info!("Menghubungkan ke MQTT HiveMQ Cloud...");
    let mut mqtt_client = EspMqttClient::new_cb(
        &broker_url,
        &mqtt_config,
        move |event| {
            match event.payload() {
                EventPayload::Connected(_) => {
                    info!("MQTT CONNECTED!");
                    connected_cb.store(true, Ordering::SeqCst);
                }
                EventPayload::Subscribed(id) => {
                    info!("MQTT SUBSCRIBED ke OTA (id={})", id);
                }
                EventPayload::Received { topic, data, .. } => {
                    let t = topic.unwrap_or("");
                    let d = core::str::from_utf8(data).unwrap_or("");
                    info!("Pesan diterima [{}]: {}", t, d);

                    if t == MQTT_OTA_TOPIC {
                        info!(">>> PERINTAH OTA DITERIMA! <<<");
                        if let Some(url) = extract_url(d) {
                            info!("Target Firmware URL: {}", url);
                            if let Ok(mut lock) = ota_url_cb.lock() {
                                *lock = Some(url);
                            }
                        }
                    }
                }
                EventPayload::Disconnected => {
                    info!("MQTT DISCONNECTED");
                    connected_cb.store(false, Ordering::SeqCst);
                }
                _ => {}
            }
        },
    )?;

    // Tunggu koneksi broker stabil
    for _ in 0..20 {
        if connected.load(Ordering::SeqCst) {
            break;
        }
        sleep(Duration::from_millis(500));
    }

    // Subscribe ke topic OTA
    info!("Subscribe ke topic OTA: {}", MQTT_OTA_TOPIC);
    let _ = mqtt_client.subscribe(MQTT_OTA_TOPIC, esp_idf_svc::mqtt::client::QoS::AtLeastOnce);

    // 4. Main loop: Jaga lampu putih tetap menyala, cek OTA, & kirim status berkala
    loop {
        // Cek jika ada perintah OTA masuk
        {
            let mut lock = ota_url.lock().unwrap();
            if let Some(url) = lock.take() {
                info!("!!! MEMULAI PROSES OTA DARI: {} !!!", url);
                match perform_ota_update(&url) {
                    Ok(_) => {
                        info!("OTA SUKSES! Restarting dalam 3 detik...");
                        sleep(Duration::from_secs(3));
                        unsafe { esp_idf_svc::sys::esp_restart() };
                    }
                    Err(e) => {
                        log::error!("OTA GAGAL: {:?}", e);
                    }
                }
            }
        }

        // Pastikan LED tetap menyala putih
        let _ = ws2812.write([white_color].iter().cloned());

        // Kirim status telemetry ke broker
        let payload = format!(
            "{{\"deviceId\":\"{}\",\"status\":\"ONLINE\",\"led_mode\":\"SOLID_WHITE\",\"firmware_version\":\"{}\"}}",
            DEVICE_ID, FIRMWARE_VERSION
        );
        let _ = mqtt_client.publish(
            MQTT_TOPIC,
            esp_idf_svc::mqtt::client::QoS::AtLeastOnce,
            false,
            payload.as_bytes(),
        );

        sleep(Duration::from_secs(3));
    }
}

fn extract_url(json: &str) -> Option<String> {
    let key = "\"url\"";
    let start = json.find(key)?;
    let rest = &json[start + key.len()..];
    let rest = rest.trim_start().strip_prefix(':')?;
    let rest = rest.trim_start();
    let q1 = rest.find('"')? + 1;
    let q2 = rest[q1..].find('"')? + q1;
    Some(rest[q1..q2].to_string())
}

fn perform_ota_update(url: &str) -> anyhow::Result<()> {
    use esp_idf_svc::sys::*;
    use std::ffi::CString;

    info!("OTA: Mengunduh firmware dari {}", url);
    let url_c = CString::new(url)?;
    let http_config = esp_http_client_config_t {
        url: url_c.as_ptr(),
        timeout_ms: 30000,
        buffer_size: 4096,
        buffer_size_tx: 512,
        ..Default::default()
    };

    let client = unsafe { esp_http_client_init(&http_config) };
    if client.is_null() { anyhow::bail!("Gagal inisialisasi HTTP client"); }

    let err = unsafe { esp_http_client_open(client, 0) };
    if err != ESP_OK as i32 {
        unsafe { esp_http_client_cleanup(client) };
        anyhow::bail!("Gagal membuka koneksi HTTP: {}", err);
    }

    let content_length = unsafe { esp_http_client_fetch_headers(client) };
    let status = unsafe { esp_http_client_get_status_code(client) };
    info!("OTA: HTTP Status {} | Ukuran: {} bytes", status, content_length);

    if status != 200 {
        unsafe { esp_http_client_cleanup(client) };
        anyhow::bail!("Server mengembalikan status HTTP {}", status);
    }

    let mut ota = EspOta::new()?;
    let mut update = ota.initiate_update()?;
    let mut buf = vec![0u8; 4096];
    let mut total = 0usize;

    loop {
        let n = unsafe { esp_http_client_read(client, buf.as_mut_ptr(), buf.len() as i32) };
        if n <= 0 { break; }
        IoWrite::write_all(&mut update, &buf[..n as usize])?;
        total += n as usize;
        if total % (64 * 1024) < 4096 {
            info!("Progress OTA: {} KB tertulis...", total / 1024);
        }
    }

    unsafe { esp_http_client_cleanup(client) };
    update.complete()?;
    info!("OTA Selesai! Total: {} KB", total / 1024);
    Ok(())
}
