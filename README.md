# IoT-ESP32-Project

> Repository Proyek IoT - ESP32-S3 | Mata Kuliah Internet of Things (IoT) Semester 5

## Deskripsi Proyek

Proyek ini mengimplementasikan sistem **IoT berbasis ESP32-S3** yang mencakup:

- **Firmware ESP32-S3** ditulis dalam **Rust** menggunakan framework `esp-idf-svc`
- **Koneksi MQTT** ke **HiveMQ Cloud** untuk telemetri dan kontrol jarak jauh
- **Over-The-Air (OTA) Update** - update firmware ESP32 via Wi-Fi tanpa kabel USB
- **Kontrol LED RGB** (WS2812 NeoPixel) pada GPIO 48
- **Pembacaan Sensor MQ** (ADC) untuk deteksi aroma
- **Multi-firmware switching** - berganti mode firmware (RGB/Putih) lewat OTA

## Struktur Repository

```
IoT-ESP32-Project/
├── README.md                  # Dokumentasi utama
├── kelas-a/                   # Proyek Kelas A (Zhafran & Lia)
│   ├── README.md              # Dokumentasi lengkap Kelas A
│   ├── Cargo.toml             # Konfigurasi project Rust
│   ├── Cargo.lock             # Lock file dependencies
│   ├── build.rs               # Build script
│   ├── rust-toolchain.toml    # Toolchain Rust (Xtensa)
│   ├── sdkconfig.defaults     # Konfigurasi ESP-IDF
│   ├── partitions.csv         # Partition table (OTA dual-slot)
│   ├── .gitignore             # Git ignore rules
│   ├── .cargo/
│   │   └── config.toml        # Cargo build target config
│   ├── .github/
│   │   └── workflows/
│   │       └── rust_ci.yml    # GitHub Actions CI
│   ├── src/
│   │   ├── main.rs            # Firmware utama (v1.0.5) - Sensor + OTA
│   │   ├── main_rgb.rs        # Firmware RGB (v1.0.8) - Siklus warna
│   │   └── main_white.rs      # Firmware Putih (v1.0.7) - Solid white
│   └── tools/
│       ├── ganti_mode.py      # Script ganti mode firmware via OTA
│       └── kirim_ota.py       # Script kirim perintah OTA
└── kelas-c/                   # Proyek Kelas C
    └── README.md              # Dokumentasi Kelas C
```

## Teknologi yang Digunakan

| Komponen | Teknologi |
|----------|-----------|
| Microcontroller | ESP32-S3 (Xtensa dual-core) |
| Bahasa | Rust (no_std + esp-idf-svc) |
| MQTT Broker | HiveMQ Cloud (TLS/SSL) |
| LED Driver | WS2812 NeoPixel (GPIO 48) |
| OTA Method | HTTP + ESP-IDF OTA API |
| Sensor | MQ Gas Sensor (ADC GPIO 3) |
| Build Tool | Cargo + espflash |

## Quick Start

Lihat dokumentasi masing-masing kelas untuk petunjuk lebih detail:

- [Kelas A - Dokumentasi Lengkap](./kelas-a/README.md)
- [Kelas C - Dokumentasi](./kelas-c/README.md)

## Kontributor

- **Kelas A**: Zhafran & Lia
- **Kelas C**: Rijal & Esqy
