# Kelas A - ESP32-S3 IoT Firmware (Rust)

> Proyek IoT oleh **Zhafran** | ESP32-S3 dengan Rust + OTA Update + MQTT HiveMQ Cloud

## Fitur Utama

### 1. Firmware Utama (`main.rs`) - v1.0.5
- Koneksi Wi-Fi otomatis ke jaringan "Papale"
- Koneksi MQTT ke HiveMQ Cloud dengan TLS/SSL
- Pembacaan sensor MQ (ADC pada GPIO 3) untuk deteksi aroma
- Prediksi aroma sederhana berdasarkan tegangan sensor:
  - `< 1.80V` → Normal
  - `< 2.50V` → Kopi Arabika
  - `>= 2.50V` → Kopi Robusta
- Mengirim telemetri JSON ke topic `aromalab/esp32/prediction`
- **OTA Update** - mendengarkan perintah update di topic `aromalab/esp32/ota`

### 2. Firmware RGB (`main_rgb.rs`) - v1.0.8-RGB
- Menyalakan LED RGB onboard (WS2812 NeoPixel di GPIO 48)
- Siklus warna otomatis: Merah → Hijau → Biru → Kuning → Cyan → Ungu → Putih
- Mengirim status warna aktif ke MQTT
- Siap menerima OTA update untuk beralih ke firmware lain

### 3. Firmware Putih (`main_white.rs`) - v1.0.7-WHITE
- Menyalakan LED onboard dengan warna putih solid
- Mengirim status online ke MQTT
- Siap menerima OTA update

## Arsitektur Sistem

```
┌─────────────────┐        MQTT (TLS)         ┌──────────────────┐
│   ESP32-S3      │◄─────────────────────────► │  HiveMQ Cloud    │
│                 │                            │  Broker          │
│  ┌───────────┐  │    Topic: aromalab/        │                  │
│  │ MQ Sensor │  │    esp32/prediction        └────────┬─────────┘
│  │ (GPIO 3)  │  │    esp32/ota                        │
│  └───────────┘  │                                     │
│                 │                            ┌────────▼─────────┐
│  ┌───────────┐  │        HTTP (OTA)          │  Laptop/PC       │
│  │ WS2812 LED│  │◄───────────────────────────│  (HTTP Server)   │
│  │ (GPIO 48) │  │    Download firmware.bin   │  + ganti_mode.py │
│  └───────────┘  │                            └──────────────────┘
└─────────────────┘
```

## Cara Penggunaan

### Prasyarat
- Rust toolchain dengan target `xtensa-esp32s3-espidf`
- `espflash` CLI tool
- Python 3 dengan package `paho-mqtt` dan `pyserial`
- ESP32-S3 board dengan onboard NeoPixel (GPIO 48)

### 1. Build Firmware
```bash
# Build firmware utama (sensor + OTA)
cargo build --release --target xtensa-esp32s3-espidf --bin iot

# Build firmware RGB
cargo build --release --target xtensa-esp32s3-espidf --bin iot_rgb

# Build firmware putih
cargo build --release --target xtensa-esp32s3-espidf --bin iot_white
```

### 2. Flash ke ESP32 (Pertama Kali via USB)
```bash
espflash flash --chip esp32s3 --partition-table partitions.csv target/xtensa-esp32s3-espidf/release/iot
```

### 3. Generate Binary untuk OTA
```bash
# Untuk mode RGB
espflash save-image --chip esp32s3 --flash-size 16mb --partition-table partitions.csv target/xtensa-esp32s3-espidf/release/iot_rgb firmware_rgb.bin

# Untuk mode putih
espflash save-image --chip esp32s3 --flash-size 16mb --partition-table partitions.csv target/xtensa-esp32s3-espidf/release/iot_white firmware_white.bin
```

### 4. Update Firmware via OTA (Tanpa Kabel USB)

**Langkah 1**: Nyalakan HTTP server
```bash
python -m http.server 8000
```

**Langkah 2**: Kirim perintah OTA menggunakan script interaktif
```bash
# Mode interaktif (pilih dari menu)
python tools/ganti_mode.py

# Atau langsung:
python tools/ganti_mode.py rgb     # Ganti ke RGB
python tools/ganti_mode.py putih   # Ganti ke Putih
```

## Konfigurasi MQTT

| Parameter | Nilai |
|-----------|-------|
| Broker | HiveMQ Cloud |
| Host | `9fd45ca183ff4259bc0ee557d5e2ae7c.s1.eu.hivemq.cloud` |
| Port | 8883 (MQTTS) |
| Topic Telemetri | `aromalab/esp32/prediction` |
| Topic OTA | `aromalab/esp32/ota` |

## Partition Table

| Label | Type | Offset | Size |
|-------|------|--------|------|
| nvs | WiFi data | 0x9000 | 24KB |
| otadata | OTA data | 0xF000 | 8KB |
| phy_init | RF data | 0x11000 | 4KB |
| ota_0 | OTA app | 0x20000 | 2MB |
| ota_1 | OTA app | 0x220000 | 2MB |

## Lisensi

Proyek ini dibuat untuk keperluan tugas mata kuliah IoT Semester 5.
