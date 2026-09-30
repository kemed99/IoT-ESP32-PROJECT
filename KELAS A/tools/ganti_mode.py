import sys
import socket
import ssl
import time
import paho.mqtt.client as mqtt

# =====================================================================
# KONFIGURASI BROKER & OTA
# =====================================================================
MQTT_HOST = "9fd45ca183ff4259bc0ee557d5e2ae7c.s1.eu.hivemq.cloud"
MQTT_PORT = 8883
MQTT_USER = "esqy123"
MQTT_PASS = "esqysatusampe8"

OTA_TOPIC = "aromalab/esp32/ota"
STATUS_TOPIC = "aromalab/esp32/prediction"
HTTP_PORT = 8000

def get_local_ip():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(('8.8.8.8', 80))
        ip = s.getsockname()[0]
    except Exception:
        ip = '10.93.178.157'
    finally:
        s.close()
    return ip

LOCAL_IP = get_local_ip()

# Daftar firmware yang tersedia
MODES = {
    "1": ("Mode RGB (Pelangi Berwarna-warni)", f"http://{LOCAL_IP}:{HTTP_PORT}/firmware_rgb.bin"),
    "2": ("Mode Putih (Solid White LED)", f"http://{LOCAL_IP}:{HTTP_PORT}/firmware_white.bin"),
    "rgb": ("Mode RGB (Pelangi Berwarna-warni)", f"http://{LOCAL_IP}:{HTTP_PORT}/firmware_rgb.bin"),
    "putih": ("Mode Putih (Solid White LED)", f"http://{LOCAL_IP}:{HTTP_PORT}/firmware_white.bin"),
    "white": ("Mode Putih (Solid White LED)", f"http://{LOCAL_IP}:{HTTP_PORT}/firmware_white.bin"),
}

def main():
    print("==================================================")
    print("       PENGGANTI MODE FIRMWARE ESP32-S3 VIA OTA   ")
    print("==================================================")
    print(f"[INFO] IP Laptop: {LOCAL_IP}")

    # Cek apakah mode diberikan lewat argumen command line
    selected_mode = None
    if len(sys.argv) > 1:
        arg = sys.argv[1].lower().strip()
        if arg in MODES:
            selected_mode = MODES[arg]

    if not selected_mode:
        print("\nPilih mode firmware yang ingin dikirim ke ESP32:")
        print("  [1] Mode RGB   (Lampu menyala siklus warna-warni)")
        print("  [2] Mode Putih (Lampu menyala putih saja)")
        pilihan = input("\nMasukkan pilihan (1/2): ").strip()

        if pilihan in MODES:
            selected_mode = MODES[pilihan]
        else:
            print("[ERROR] Pilihan tidak valid. Silakan pilih 1 atau 2.")
            return

    mode_name, ota_url = selected_mode
    print("--------------------------------------------------")
    print(f"Target Mode : {mode_name}")
    print(f"URL Firmware: {ota_url}")
    print("--------------------------------------------------")

    def on_connect(client, userdata, flags, rc, props=None):
        if rc == 0 or str(rc) == "Success":
            print("[OK] Terhubung ke HiveMQ Cloud Broker!")
            payload = f'{{"url": "{ota_url}"}}'
            client.publish(OTA_TOPIC, payload, qos=1)
            print(f"[SEND] Perintah OTA Terkirim -> {payload}")
            print("\n[WAIT] Menunggu ESP32 mengunduh & restart (sekitar 10-15 detik)...")
        else:
            print(f"[ERROR] Gagal terhubung ke broker: {rc}")

    def on_message(client, userdata, msg):
        payload = msg.payload.decode('utf-8', errors='replace')
        print(f"[RESPON ESP32] {msg.topic} -> {payload}")

    client = mqtt.Client(client_id="ota_mode_switcher", protocol=mqtt.MQTTv5)
    client.username_pw_set(MQTT_USER, MQTT_PASS)
    client.tls_set(tls_version=ssl.PROTOCOL_TLS)

    client.on_connect = on_connect
    client.on_message = on_message

    print("[CONNECT] Menghubungkan ke HiveMQ Cloud...")
    client.connect(MQTT_HOST, MQTT_PORT)
    client.subscribe("aromalab/esp32/#")

    client.loop_start()

    try:
        for _ in range(18):
            time.sleep(1)
    except KeyboardInterrupt:
        pass

    client.loop_stop()
    client.disconnect()
    print("\n[DONE] Selesai!")

if __name__ == "__main__":
    main()
