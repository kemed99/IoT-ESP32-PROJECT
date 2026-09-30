import socket
import ssl
import time
import paho.mqtt.client as mqtt

# =====================================================================
# KONFIGURASI PENGIRIMAN OTA
# =====================================================================
MQTT_HOST = "9fd45ca183ff4259bc0ee557d5e2ae7c.s1.eu.hivemq.cloud"
MQTT_PORT = 8883
MQTT_USER = "esqy123"
MQTT_PASS = "esqysatusampe8"

OTA_TOPIC = "aromalab/esp32/ota"
STATUS_TOPIC = "aromalab/esp32/prediction"

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
HTTP_PORT = 8000
OTA_URL = f"http://{LOCAL_IP}:{HTTP_PORT}/iot.bin"

print("==================================================")
print("       PENGIRIM PERINTAH OTA ESP32-S3             ")
print("==================================================")
print(f"[INFO] IP Laptop Terdeteksi : {LOCAL_IP}")
print(f"[INFO] Target Firmware URL  : {OTA_URL}")
print(f"[INFO] Topic MQTT           : {OTA_TOPIC}")
print("==================================================")

def on_connect(client, userdata, flags, rc, props=None):
    if rc == 0 or str(rc) == "Success":
        print("\n[OK] Berhasil terhubung ke HiveMQ Cloud Broker!")
        payload = f'{{"url": "{OTA_URL}"}}'
        client.publish(OTA_TOPIC, payload, qos=1)
        print(f"[SEND] Perintah OTA Terkirim -> {payload}")
        print("\n[WAIT] Menunggu respon dari ESP32 (mendengarkan telemetri)...")
    else:
        print(f"[ERROR] Gagal terhubung ke broker, kode: {rc}")

def on_message(client, userdata, msg):
    payload = msg.payload.decode('utf-8', errors='replace')
    print(f"\n[RESPON ESP32] {msg.topic}:")
    print(f"   {payload}")

client = mqtt.Client(client_id="ota_commander_cli", protocol=mqtt.MQTTv5)
client.username_pw_set(MQTT_USER, MQTT_PASS)
client.tls_set(tls_version=ssl.PROTOCOL_TLS)

client.on_connect = on_connect
client.on_message = on_message

print("[CONNECT] Menghubungkan ke broker...")
client.connect(MQTT_HOST, MQTT_PORT)
client.subscribe("aromalab/esp32/#")

client.loop_start()

try:
    for i in range(15, 0, -1):
        time.sleep(1)
except KeyboardInterrupt:
    pass

client.loop_stop()
client.disconnect()
print("\n[DONE] Selesai!")
