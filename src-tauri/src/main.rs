// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};

type Clients = Arc<Mutex<Vec<tokio::sync::mpsc::UnboundedSender<String>>>>;

#[derive(Serialize, Deserialize, Debug)]
struct Telemetry {
    // ── Race state ─────────────────────────────────────────────────────────
    #[serde(rename = "isRaceOn")]
    is_race_on: i32,

    // ── Engine ─────────────────────────────────────────────────────────────
    #[serde(rename = "maxRpm")]
    max_rpm: f32,
    #[serde(rename = "idleRpm")]
    idle_rpm: f32,
    rpm: f32,

    // ── Physics – accelerations (car local space, m/s²) ───────────────────
    // X = right (lateral),  Y = up (vertical),  Z = forward (longitudinal)
    #[serde(rename = "accelX")]
    accel_x: f32,
    #[serde(rename = "accelY")]
    accel_y: f32,
    #[serde(rename = "accelZ")]
    accel_z: f32,

    // ── Physics – angular velocity (car local space, rad/s) ───────────────
    // X = pitch,  Y = yaw,  Z = roll
    #[serde(rename = "angVelX")]
    ang_vel_x: f32,
    #[serde(rename = "angVelY")]
    ang_vel_y: f32,
    #[serde(rename = "angVelZ")]
    ang_vel_z: f32,

    // ── Suspension travel – normalized (0 = max stretch, 1 = max compression)
    #[serde(rename = "suspFL")]
    susp_fl: f32,
    #[serde(rename = "suspFR")]
    susp_fr: f32,
    #[serde(rename = "suspRL")]
    susp_rl: f32,
    #[serde(rename = "suspRR")]
    susp_rr: f32,

     // ── Tire slip ratio (0 = 100% grip, |ratio| > 1 = loss of grip) ───────
    #[serde(rename = "slipFL")]
    slip_fl: f32,
    #[serde(rename = "slipFR")]
    slip_fr: f32,
    #[serde(rename = "slipRL")]
    slip_rl: f32,
    #[serde(rename = "slipRR")]
    slip_rr: f32,

    // ── Wheel rotation speed (rad/s) ───────────────────────────────────────
    // Near 0 while braking hard = wheel lockup (ABS territory)
    #[serde(rename = "wheelSpeedFL")]
    wheel_speed_fl: f32,
    #[serde(rename = "wheelSpeedFR")]
    wheel_speed_fr: f32,
    #[serde(rename = "wheelSpeedRL")]
    wheel_speed_rl: f32,
    #[serde(rename = "wheelSpeedRR")]
    wheel_speed_rr: f32,

    // ── Drivetrain outputs ─────────────────────────────────────────────────
    speed: f32,         // m/s
    power: f32,         // HP  (converted from watts)
    torque: f32,        // N·m
    boost: f32,         // PSI above atmospheric

    // ── Driver inputs ──────────────────────────────────────────────────────
    gear: u8,
    throttle: f32,      // 0.0 – 1.0
    brake: f32,         // 0.0 – 1.0
    hand_brake: u8,     // 255 = handbrake held (used for LC detection)

    // ── Car identity ───────────────────────────────────────────────────────
    #[serde(rename = "carOrdinal")]
    car_ordinal: i32,
    #[serde(rename = "numCylinders")]
    num_cylinders: i32, // 0 = EV

    // ── Race status ────────────────────────────────────────────────────────
    #[serde(rename = "racePosition")]
    race_position: u8,
    #[serde(rename = "lapNumber")]
    lap_number: u16,
}

fn parse_telemetry(data: &[u8]) -> Option<Telemetry> {
    if data.len() < 324 {  // FH6 packet is 324 bytes
        return None;
    }

    Some(Telemetry {
        // ── Race state ─────────────────────────────────────────────────────
        is_race_on:    i32::from_le_bytes(data[0..4].try_into().ok()?),
 
        // ── Engine ─────────────────────────────────────────────────────────
        max_rpm:       f32::from_le_bytes(data[8..12].try_into().ok()?),
        idle_rpm:      f32::from_le_bytes(data[12..16].try_into().ok()?),
        rpm:           f32::from_le_bytes(data[16..20].try_into().ok()?),
 
        // ── Accelerations (offsets 20 / 24 / 28) ──────────────────────────
        accel_x:       f32::from_le_bytes(data[20..24].try_into().ok()?),
        accel_y:       f32::from_le_bytes(data[24..28].try_into().ok()?),
        accel_z:       f32::from_le_bytes(data[28..32].try_into().ok()?),
 
        // ── Angular velocity (offsets 44 / 48 / 52) ───────────────────────
        ang_vel_x:     f32::from_le_bytes(data[44..48].try_into().ok()?),
        ang_vel_y:     f32::from_le_bytes(data[48..52].try_into().ok()?),
        ang_vel_z:     f32::from_le_bytes(data[52..56].try_into().ok()?),
 
        // ── Suspension travel normalized (offsets 68 / 72 / 76 / 80) ──────
        susp_fl:       f32::from_le_bytes(data[68..72].try_into().ok()?),
        susp_fr:       f32::from_le_bytes(data[72..76].try_into().ok()?),
        susp_rl:       f32::from_le_bytes(data[76..80].try_into().ok()?),
        susp_rr:       f32::from_le_bytes(data[80..84].try_into().ok()?),
 
        // ── Tire slip ratio (offsets 84 / 88 / 92 / 96) ───────────────────
        slip_fl:       f32::from_le_bytes(data[84..88].try_into().ok()?),
        slip_fr:       f32::from_le_bytes(data[88..92].try_into().ok()?),
        slip_rl:       f32::from_le_bytes(data[92..96].try_into().ok()?),
        slip_rr:       f32::from_le_bytes(data[96..100].try_into().ok()?),
 
        // ── Wheel rotation speed (offsets 100 / 104 / 108 / 112) ──────────
        wheel_speed_fl: f32::from_le_bytes(data[100..104].try_into().ok()?),
        wheel_speed_fr: f32::from_le_bytes(data[104..108].try_into().ok()?),
        wheel_speed_rl: f32::from_le_bytes(data[108..112].try_into().ok()?),
        wheel_speed_rr: f32::from_le_bytes(data[112..116].try_into().ok()?),
 
        // ── Drivetrain outputs ─────────────────────────────────────────────
        speed:         f32::from_le_bytes(data[256..260].try_into().ok()?),          // m/s
        power:         f32::from_le_bytes(data[260..264].try_into().ok()?) / 745.7,  // W → HP
        torque:        f32::from_le_bytes(data[264..268].try_into().ok()?),           // N·m
        boost:         f32::from_le_bytes(data[284..288].try_into().ok()?),           // PSI
 
        // ── Driver inputs ──────────────────────────────────────────────────
        gear:          data[319],
        throttle:      data[315] as f32 / 255.0,
        brake:         data[316] as f32 / 255.0,
        hand_brake:    data[318],   // 1 = held, used for LC detection
 
        // ── Car identity ───────────────────────────────────────────────────
        car_ordinal:   i32::from_le_bytes(data[212..216].try_into().ok()?),
        num_cylinders: i32::from_le_bytes(data[228..232].try_into().ok()?),
 
        // ── Race status ────────────────────────────────────────────────────
        lap_number:    u16::from_le_bytes(data[312..314].try_into().ok()?),
        race_position: data[314],
    })
}

async fn udp_listener(clients: Clients) {
    // FH6 doc: avoid ports 5200–5300 (game uses that range for its own socket)
    let socket = UdpSocket::bind("127.0.0.1:5301").expect("Failed to bind UDP socket");
    socket.set_nonblocking(true).expect("Failed to set non-blocking");

    println!("[Bridge] Listening for Forza telemetry on 127.0.0.1:5301");

    let mut buffer = [0u8; 1024];

    loop {
        match socket.recv_from(&mut buffer) {
            Ok((size, _)) => {
                if let Some(telemetry) = parse_telemetry(&buffer[..size]) {
                    if let Ok(json) = serde_json::to_string(&telemetry) {
                        let mut clients = clients.lock().unwrap();
                        clients.retain(|tx| tx.send(json.clone()).is_ok());
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
            }
            Err(e) => eprintln!("[Bridge] Error: {}", e),
        }
    }
}

async fn websocket_server(clients: Clients) {
    let listener = TcpListener::bind("127.0.0.1:8765").await.expect("Failed to bind WebSocket");
    println!("[WebSocket] Running on ws://127.0.0.1:8765");

    while let Ok((stream, _)) = listener.accept().await {
        let clients = clients.clone();

        tokio::spawn(async move {
            let ws_stream = match accept_async(stream).await {
                Ok(ws) => ws,
                Err(e) => {
                    eprintln!("[WebSocket] Error: {}", e);
                    return;
                }
            };

            let (mut ws_tx, _ws_rx) = ws_stream.split();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

            {
                let mut clients = clients.lock().unwrap();
                clients.clear(); // drop all existing senders → old connections close cleanly
                clients.push(tx);
                println!("[WebSocket] Client connected. Total: {}", clients.len());
            }

            while let Some(msg) = rx.recv().await {
                if ws_tx.send(tokio_tungstenite::tungstenite::Message::Text(msg)).await.is_err() {
                    break;
                }
            }

            println!("[WebSocket] Client disconnected");
        });
    }
}

#[tokio::main]
async fn main() {
    let clients: Clients = Arc::new(Mutex::new(Vec::new()));

    // Start UDP listener
    let clients_udp = clients.clone();
    tokio::spawn(async move {
        udp_listener(clients_udp).await;
    });

    // Start WebSocket server
    let clients_ws = clients.clone();
    tokio::spawn(async move {
        websocket_server(clients_ws).await;
    });

    // Start Tauri window
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}