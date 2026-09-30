use esp_idf_hal::adc::attenuation::DB_12;
use esp_idf_hal::adc::oneshot::config::AdcChannelConfig;
use esp_idf_hal::adc::oneshot::{AdcChannelDriver, AdcDriver};
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::mqtt::client::{EspMqttClient, EventPayload, MqttClientConfiguration};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::ota::EspOta;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use embedded_io::Write as IoWrite;
use log::info;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;

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
const FIRMWARE_VERSION: &str = "1.0.5";

// =====================================================================

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let app_thread = std::thread::Builder::new()
        .name("iot_app".to_string())
        .stack_size(20 * 1024)
        .spawn(|| -> anyhow::Result<()> {
            run_iot_app()
        })?;

    app_thread.join().unwrap()?;
    Ok(())
}

fn run_iot_app() -> anyhow::Result<()> {
    info!("===========================================");
    info!(" ESP32-S3 IoT Firmware v{}", FIRMWARE_VERSION);
    info!(" OTA Topic: {}", MQTT_OTA_TOPIC);
    info!("===========================================");

    let peripherals = Peripherals::take().unwrap();
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // 1. Wi-Fi
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
    info!("Connecting Wi-Fi '{}'...", WIFI_SSID);
    wifi.connect()?;
    wifi.wait_netif_up()?;
    info!("Wi-Fi OK! IP: {:?}", wifi.wifi().sta_netif().get_ip_info()?);

    // 2. MQTT - menggunakan callback API (new_cb) agar subscribe langsung berfungsi
    let broker_url = format!(
        "mqtts://{}:{}@{}:{}",
        MQTT_USERNAME, MQTT_PASSWORD, MQTT_HOST, MQTT_PORT
    );
    let mqtt_config = MqttClientConfiguration {
        client_id: Some("esp32-s3-node-01"),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };

    // Shared OTA URL: callback menulis, main loop membaca
    let ota_url: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let ota_url_cb = ota_url.clone();

    // Flag: apakah broker sudah connected (agar subscribe bisa dilakukan)
    let connected = Arc::new(AtomicBool::new(false));
    let connected_cb = connected.clone();

    info!("Connecting MQTT broker...");
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
                    info!("MQTT SUBSCRIBED (id={})", id);
                }
                EventPayload::Received { topic, data, .. } => {
                    let t = topic.unwrap_or("");
                    let d = core::str::from_utf8(data).unwrap_or("");
                    info!("MQTT RX [{}]: {}", t, d);

                    if t == MQTT_OTA_TOPIC {
                        info!(">>> OTA COMMAND RECEIVED! <<<");
                        if let Some(url) = extract_url(d) {
                            info!("OTA URL: {}", url);
                            if let Ok(mut lock) = ota_url_cb.lock() {
                                *lock = Some(url);
                            }
                        } else {
                            log::error!("Invalid OTA format. Send: {{\"url\":\"http://IP:8000/iot.bin\"}}");
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

    // Tunggu MQTT connected, lalu subscribe OTA topic
    info!("Waiting for MQTT connection...");
    for i in 0..20 {
        if connected.load(Ordering::SeqCst) {
            break;
        }
        sleep(Duration::from_millis(500));
        if i == 19 {
            log::warn!("MQTT connection timeout, continuing anyway...");
        }
    }

    info!("Subscribing to OTA topic: {}", MQTT_OTA_TOPIC);
    mqtt_client.subscribe(MQTT_OTA_TOPIC, esp_idf_svc::mqtt::client::QoS::AtLeastOnce)?;
    info!(">>> SUBSCRIBED TO OTA TOPIC OK <<<");

    // 3. ADC
    let adc = AdcDriver::new(peripherals.adc1)?;
    let adc_config = AdcChannelConfig {
        attenuation: DB_12,
        ..Default::default()
    };
    let mut adc_mq = AdcChannelDriver::new(&adc, peripherals.pins.gpio3, &adc_config)?;
    info!("ADC ready!");

    // 4. Main loop
    let mut temperature = 25.0_f32;
    loop {
        // ===== CHECK OTA =====
        {
            let mut lock = ota_url.lock().unwrap();
            if let Some(url) = lock.take() {
                info!("!!! STARTING OTA from: {} !!!", url);
                match perform_ota_update(&url) {
                    Ok(_) => {
                        info!("OTA SUCCESS! Restarting in 3s...");
                        sleep(Duration::from_secs(3));
                        unsafe { esp_idf_svc::sys::esp_restart() };
                    }
                    Err(e) => {
                        log::error!("OTA FAILED: {:?}", e);
                    }
                }
            }
        }

        // ===== SENSOR & TELEMETRY =====
        let raw_adc = adc.read_raw(&mut adc_mq).unwrap_or(0);
        let mv = adc.read(&mut adc_mq).unwrap_or((raw_adc as f32 / 4095.0 * 3300.0) as u16);
        let voltage = mv as f32 / 1000.0;

        let (prediction, confidence) = if voltage < 1.80 {
            ("Normal", 0.96)
        } else if voltage < 2.50 {
            ("Kopi_Arabika", 0.92)
        } else {
            ("Kopi_Robusta", 0.94)
        };

        let payload = format!(
            "{{\"deviceId\":\"{}\",\"prediction\":\"{}\",\"confidence\":{:.2},\"mq_raw\":{},\"mq_voltage\":{:.2},\"temperature\":{:.2},\"firmware_version\":\"{}\"}}",
            DEVICE_ID, prediction, confidence, raw_adc, voltage, temperature, FIRMWARE_VERSION
        );

        let _ = mqtt_client.publish(
            MQTT_TOPIC,
            esp_idf_svc::mqtt::client::QoS::AtLeastOnce,
            false,
            payload.as_bytes(),
        );

        temperature += 0.5;
        if temperature > 35.0 { temperature = 25.0; }
        sleep(Duration::from_secs(5));
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

    info!("OTA: Connecting to {}", url);
    let url_c = CString::new(url)?;
    let http_config = esp_http_client_config_t {
        url: url_c.as_ptr(),
        timeout_ms: 30000,
        buffer_size: 4096,
        buffer_size_tx: 512,
        ..Default::default()
    };

    let client = unsafe { esp_http_client_init(&http_config) };
    if client.is_null() { anyhow::bail!("HTTP client init failed"); }

    let err = unsafe { esp_http_client_open(client, 0) };
    if err != ESP_OK as i32 {
        unsafe { esp_http_client_cleanup(client) };
        anyhow::bail!("HTTP open failed: {}", err);
    }

    let content_length = unsafe { esp_http_client_fetch_headers(client) };
    let status = unsafe { esp_http_client_get_status_code(client) };
    info!("OTA: HTTP {} | Size: {} bytes", status, content_length);

    if status != 200 {
        unsafe { esp_http_client_cleanup(client) };
        anyhow::bail!("HTTP error {}", status);
    }

    let mut ota = EspOta::new()?;
    let mut update = ota.initiate_update()?;
    let mut buf = vec![0u8; 4096];
    let mut total = 0usize;

    loop {
        let n = unsafe { esp_http_client_read(client, buf.as_mut_ptr(), buf.len() as i32) };
        if n < 0 {
            unsafe { esp_http_client_cleanup(client) };
            anyhow::bail!("HTTP read error");
        }
        if n == 0 { break; }
        IoWrite::write_all(&mut update, &buf[..n as usize])?;
        total += n as usize;
        if total % (64 * 1024) < 4096 {
            info!("OTA progress: {} KB", total / 1024);
        }
    }

    unsafe { esp_http_client_cleanup(client) };
    update.complete()?;
    info!("OTA complete! {} KB written", total / 1024);
    Ok(())
}
