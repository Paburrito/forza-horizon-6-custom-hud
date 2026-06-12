// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tauri::Manager;

type Clients = Arc<Mutex<Vec<tokio::sync::mpsc::Sender<String>>>>;

// ── Raw telemetry from UDP packet ─────────────────────────────────────────────
#[derive(Debug)]
struct Telemetry {
    is_race_on:     i32,
    max_rpm:        f32,
    idle_rpm:       f32,
    rpm:            f32,
    accel_x:        f32,
    accel_y:        f32,
    accel_z:        f32,
    ang_vel_x:      f32,
    ang_vel_y:      f32,
    ang_vel_z:      f32,
    susp_fl:        f32,
    susp_fr:        f32,
    susp_rl:        f32,
    susp_rr:        f32,
    slip_fl:        f32,
    slip_fr:        f32,
    slip_rl:        f32,
    slip_rr:        f32,
    wheel_speed_fl: f32,
    wheel_speed_fr: f32,
    wheel_speed_rl: f32,
    wheel_speed_rr: f32,
    speed:          f32,
    power:          f32,
    torque:         f32,
    boost:          f32,
    gear:           u8,
    throttle:       f32,
    brake:          f32,
    hand_brake:     u8,
    car_ordinal:    i32,
    num_cylinders:  i32,
    race_position:  u8,
    lap_number:     u16,
}

// ── Car learning database entry ───────────────────────────────────────────────
#[derive(Serialize, Deserialize, Clone, Debug)]
struct CarEntry {
    limiter:    f32,
    redline:    f32,
    #[serde(rename = "maxRpm")]
    max_rpm:    f32,
    confidence: u32,
    timestamp:  u64,
}

type CarDatabase = HashMap<String, CarEntry>;

// ── Car learning result — returned per frame ──────────────────────────────────
struct LearningResult {
    redline_rpm:  f32,
    car_changed:  bool,
    car_key:      String,
    is_known:     bool,
    car_learned:  bool,
    notification: String,
}

// ── Car learning constants ────────────────────────────────────────────────────
const CONFIDENCE_TRUSTED:    u32 = 3;
const DEFAULT_REDLINE_RATIO: f32 = 0.93; // How far the redline sits below the limiter once learned

fn tune_key(car_ordinal: i32, max_rpm: f32, num_cylinders: i32) -> String {
    let rpm = ((max_rpm / 100.0).round() as i32) * 100;
    format!("car_{}_{}_cyl{}", car_ordinal, rpm, num_cylinders)
}

fn default_redline(max_rpm: f32) -> f32 {
    (max_rpm * DEFAULT_REDLINE_RATIO).round()
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Car learning state machine ────────────────────────────────────────────────
struct CarLearning {
    db:              CarDatabase,
    db_path:         PathBuf,
    current_key:     Option<String>,
    current_max_rpm: f32,
    last_race_state: i32,
    is_learning:     bool,
    peak_rpm:        f32,
    start_gear:      u8,
    speed_samples:   Vec<f32>,
    force_relearn:   Arc<AtomicBool>,
}

impl CarLearning {
    fn new(db_path: PathBuf, force_relearn: Arc<AtomicBool>) -> Self {
        let db = Self::load_db(&db_path);
        println!("[CarLearning] Initialized, database has {} entries", db.len());
        CarLearning {
            db,
            db_path,
            current_key:     None,
            current_max_rpm: 0.0,
            last_race_state: 0,
            is_learning:     false,
            peak_rpm:        0.0,
            start_gear:      0,
            speed_samples:   Vec::new(),
            force_relearn,
        }
    }

    fn load_db(path: &PathBuf) -> CarDatabase {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                eprintln!("[CarLearning] Parse error: {} — starting fresh", e);
                CarDatabase::new()
            }),
            Err(_) => CarDatabase::new(),
        }
    }

    fn save_db(&self) {
        if let Some(parent) = self.db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&self.db) {
            Ok(json) => { let _ = std::fs::write(&self.db_path, json); }
            Err(e)   => eprintln!("[CarLearning] Save error: {}", e),
        }
    }

    fn ensure_entry(&mut self, key: &str, max_rpm: f32) {
        self.db.entry(key.to_string()).or_insert_with(|| CarEntry {
            limiter:    max_rpm.round(),
            redline:    default_redline(max_rpm),
            max_rpm,
            confidence: 0,
            timestamp:  now_millis(),
        });
    }

    fn process(&mut self, t: &Telemetry) -> LearningResult {
        // Guard: ignore invalid frames (menus, cutscenes)
        if t.car_ordinal == 0 || t.max_rpm == 0.0 {
            return LearningResult {
                redline_rpm:  if self.current_max_rpm > 0.0 {
                    self.current_max_rpm * DEFAULT_REDLINE_RATIO
                } else { 8000.0 },
                car_changed:  false,
                car_key:      self.current_key.clone().unwrap_or_default(),
                is_known:     false,
                car_learned:  false,
                notification: String::new(),
            };
        }

        // Race state logging
        if t.is_race_on == 1 && self.last_race_state == 0 {
            println!("🏁 Race started");
        }
        self.last_race_state = t.is_race_on;

        // Handle force relearn signal from UI
        if self.force_relearn.swap(false, Ordering::Relaxed) {
            if let Some(key) = &self.current_key.clone() {
                if let Some(entry) = self.db.get_mut(key) {
                    entry.confidence = 0;
                    self.save_db();
                    self.is_learning = false;
                    self.peak_rpm    = 0.0;
                    println!("[CarLearning] Force relearn: {}", key);
                }
            }
        }

        // Tune/car change detection
        let new_key     = tune_key(t.car_ordinal, t.max_rpm, t.num_cylinders);
        let tune_changed = Some(&new_key) != self.current_key.as_ref();

        let mut car_changed  = false;
        let mut notification = String::new();

        if tune_changed {
            self.is_learning   = false;
            self.peak_rpm      = 0.0;
            self.speed_samples.clear();
            car_changed          = true;

            self.ensure_entry(&new_key, t.max_rpm);
            let entry    = &self.db[&new_key];
            let is_known = entry.confidence >= CONFIDENCE_TRUSTED;

            println!(
                "🏎️ Car changed → {} ({}, redline {} RPM, confidence {})",
                new_key,
                if is_known { "known" } else { "estimate" },
                entry.redline as i32,
                entry.confidence
            );

            if !is_known {
                notification = "New car detected! 🏎️ Rev it up so I can learn the limiter!".into();
            }

            self.current_key     = Some(new_key.clone());
            self.current_max_rpm = t.max_rpm;
        }

        self.current_max_rpm = t.max_rpm;

        let key = self.current_key.clone().unwrap_or_else(|| new_key.clone());
        self.ensure_entry(&key, t.max_rpm);

        let redline_rpm = self.db[&key].redline;
        let confidence  = self.db[&key].confidence;
        let is_known    = confidence >= CONFIDENCE_TRUSTED;

        // Skip learning if already trusted
        if is_known {
            return LearningResult {
                redline_rpm, car_changed, car_key: key,
                is_known, car_learned: false, notification,
            };
        }

        // Limiter zone detection
        let in_limiter_zone = t.rpm > t.max_rpm * 0.65 // 65% of maxRPM telemetry we begin learning
                           && t.throttle > 0.90
                           && t.power    > 0.0
                           && t.gear     > 0;

        if in_limiter_zone && !self.is_learning {
            self.is_learning   = true;
            self.peak_rpm      = t.rpm;
            self.start_gear    = t.gear;
            self.speed_samples = vec![t.speed];
        }

        let mut car_learned = false;

        if self.is_learning {
            if t.rpm > self.peak_rpm { self.peak_rpm = t.rpm; }

            self.speed_samples.push(t.speed);
            if self.speed_samples.len() > 10 { self.speed_samples.remove(0); }
            let avg_speed = self.speed_samples.iter().sum::<f32>()
                          / self.speed_samples.len() as f32;

            let exited_cleanly = t.gear     != self.start_gear
                              || t.throttle  < 0.90
                              || t.power    <= 0.0
                              || t.rpm       < self.peak_rpm - 500.0;

            let exited_dirty = (t.speed - avg_speed).abs() > 15.0;

            if exited_cleanly && self.peak_rpm > t.max_rpm * 0.80 { // exit quality check for when we're shifting, recorded rpm at that moment is considered meaningful
                let detected  = self.peak_rpm.round();
                let entry     = &self.db[&key];
                let existing  = entry.limiter;
                let conf      = entry.confidence;
                let diff      = (detected - existing).abs();

                if conf == 0 || diff < 600.0 {
                    let blended = if conf == 0 {
                        detected
                    } else {
                        ((existing * conf as f32 + detected) / (conf + 1) as f32).round()
                    };

                    let new_conf    = (conf + 1).min(CONFIDENCE_TRUSTED);
                    let new_redline = (blended * DEFAULT_REDLINE_RATIO).round();

                    println!(
                        "📈 Limiter refined: {} RPM → redline {} RPM (confidence {}/{})",
                        blended as i32, new_redline as i32, new_conf, CONFIDENCE_TRUSTED
                    );

                    if let Some(e) = self.db.get_mut(&key) {
                        e.limiter    = blended;
                        e.redline    = new_redline;
                        e.confidence = new_conf;
                        e.timestamp  = now_millis();
                    }
                    self.save_db();

                    if new_conf >= CONFIDENCE_TRUSTED {
                        car_learned  = true;
                        notification = format!("✅ Redline locked in: {} RPM", new_redline as i32);
                    }
                }

                self.is_learning = false;
                self.peak_rpm    = 0.0;

            } else if exited_dirty || (!in_limiter_zone && !exited_cleanly) {
                self.is_learning = false;
                self.peak_rpm    = 0.0;
            }
        }

        let final_redline = self.db[&key].redline;
        let final_known   = self.db[&key].confidence >= CONFIDENCE_TRUSTED;

        LearningResult {
            redline_rpm:  final_redline,
            car_changed,
            car_key:      key,
            is_known:     final_known,
            car_learned,
            notification,
        }
    }
}

// ── Session maxima tracker ────────────────────────────────────────────────────
#[derive(Serialize, Clone, Debug)]
struct SessionMaxima {
    #[serde(rename = "maxBoost")]  max_boost:  f32,
    #[serde(rename = "maxVacuum")] max_vacuum: f32,
    #[serde(rename = "maxHP")]     max_hp:     f32,
    #[serde(rename = "minHP")]     min_hp:     f32,
    #[serde(rename = "maxTQ")]     max_tq:     f32,
    #[serde(rename = "minTQ")]     min_tq:     f32,
    reset:                         bool,
}

impl SessionMaxima {
    fn defaults() -> Self {
        SessionMaxima {
            max_boost: 5.0, max_vacuum: 2.0,
            max_hp:    5.0, min_hp:     5.0,
            max_tq:    5.0, min_tq:     5.0,
            reset:     false,
        }
    }
}

struct MaximaTracker {
    values:       SessionMaxima,
    last_race:    i32,
    last_car_ord: i32,
    last_max_rpm: f32,
}

impl MaximaTracker {
    fn new() -> Self {
        MaximaTracker {
            values:       SessionMaxima::defaults(),
            last_race:    0,
            last_car_ord: -1,
            last_max_rpm: 0.0,
        }
    }

    fn update(&mut self, t: &Telemetry) -> SessionMaxima {
        let mut reset = false;
        if t.is_race_on == 1 && self.last_race  == 0                    { reset = true; }
        if t.car_ordinal != self.last_car_ord && self.last_car_ord != -1 { reset = true; }
        if t.max_rpm != self.last_max_rpm && self.last_max_rpm != 0.0   { reset = true; }

        if reset { self.values = SessionMaxima::defaults(); }

        self.last_race    = t.is_race_on;
        self.last_car_ord = t.car_ordinal;
        self.last_max_rpm = t.max_rpm;

        if t.speed > 10.0 {
            if t.boost  > 0.0 && t.boost  > self.values.max_boost              { self.values.max_boost  = t.boost; }
            if t.boost  < 0.0 && -t.boost > self.values.max_vacuum             { self.values.max_vacuum = -t.boost; }
            if t.power  > self.values.max_hp                                    { self.values.max_hp     = t.power; }
            if t.torque > self.values.max_tq                                    { self.values.max_tq     = t.torque; }
            if t.power  < 0.0 && -t.power  > self.values.min_hp && -t.power  < 150.0 { self.values.min_hp = -t.power; }
            if t.torque < 0.0 && -t.torque > self.values.min_tq && -t.torque < 150.0 { self.values.min_tq = -t.torque; }
        }

        let mut snapshot = self.values.clone();
        snapshot.reset = reset;
        snapshot
    }
}

// ── Lockup computation ────────────────────────────────────────────────────────
#[derive(Serialize, Clone, Debug)]
struct LockupDeltas { fl: f32, fr: f32, rl: f32, rr: f32 }

impl LockupDeltas {
    fn zero() -> Self { LockupDeltas { fl: 0.0, fr: 0.0, rl: 0.0, rr: 0.0 } }
}

const WHEEL_RADIUS: f32 = 0.33;
const BRAKE_THRESH: f32 = 0.15;
const SPEED_FLOOR:  f32 = 5.0;

fn compute_lockup(t: &Telemetry) -> LockupDeltas {
    if t.brake < BRAKE_THRESH || t.speed < SPEED_FLOOR { return LockupDeltas::zero(); }
    let expected = t.speed / WHEEL_RADIUS;
    if expected < 1.0 { return LockupDeltas::zero(); }
    let d = |ws: f32| (0.0_f32).max((expected - ws.abs()) / expected);
    LockupDeltas {
        fl: d(t.wheel_speed_fl), fr: d(t.wheel_speed_fr),
        rl: d(t.wheel_speed_rl), rr: d(t.wheel_speed_rr),
    }
}

// ── Launch control ────────────────────────────────────────────────────────────
struct LaunchControl {
    prev_hand_brake: u8,
    armed:           bool,
    launched:        bool,
    launch_time:     Option<std::time::Instant>,
}

impl LaunchControl {
    fn new() -> Self {
        LaunchControl { prev_hand_brake: 0, armed: false, launched: false, launch_time: None }
    }

    fn process(&mut self, t: &Telemetry) -> &'static str {
        let hb = t.hand_brake;

        let armed = hb > 0 && t.power < -2.0 && t.torque < -2.0 && t.speed < 0.5;

        let just_launched = hb == 0
            && self.prev_hand_brake > 0
            && t.speed > 0.0
            && t.power > 0.0;

        self.prev_hand_brake = hb;

        if self.launched {
            if let Some(lt) = self.launch_time {
                if lt.elapsed() >= std::time::Duration::from_millis(1500) {
                    self.launched    = false;
                    self.launch_time = None;
                }
            }
        }

        if just_launched && !self.launched {
            self.launched    = true;
            self.launch_time = Some(std::time::Instant::now());
        }

        self.armed = armed;

        if self.launched   { "launched" }
        else if self.armed { "armed"    }
        else               { "inactive" }
    }
}

// ── HUD payload ───────────────────────────────────────────────────────────────
#[derive(Serialize, Debug)]
struct HudPayload<'a> {
    // Passthrough telemetry
    #[serde(rename = "isRaceOn")]     is_race_on:     i32,
    #[serde(rename = "maxRpm")]       max_rpm:        f32,
    #[serde(rename = "idleRpm")]      idle_rpm:       f32,
                                      rpm:            f32,
    #[serde(rename = "accelX")]       accel_x:        f32,
    #[serde(rename = "accelY")]       accel_y:        f32,
    #[serde(rename = "accelZ")]       accel_z:        f32,
    #[serde(rename = "angVelX")]      ang_vel_x:      f32,
    #[serde(rename = "angVelY")]      ang_vel_y:      f32,
    #[serde(rename = "angVelZ")]      ang_vel_z:      f32,
    #[serde(rename = "suspFL")]       susp_fl:        f32,
    #[serde(rename = "suspFR")]       susp_fr:        f32,
    #[serde(rename = "suspRL")]       susp_rl:        f32,
    #[serde(rename = "suspRR")]       susp_rr:        f32,
    #[serde(rename = "slipFL")]       slip_fl:        f32,
    #[serde(rename = "slipFR")]       slip_fr:        f32,
    #[serde(rename = "slipRL")]       slip_rl:        f32,
    #[serde(rename = "slipRR")]       slip_rr:        f32,
    #[serde(rename = "wheelSpeedFL")] wheel_speed_fl: f32,
    #[serde(rename = "wheelSpeedFR")] wheel_speed_fr: f32,
    #[serde(rename = "wheelSpeedRL")] wheel_speed_rl: f32,
    #[serde(rename = "wheelSpeedRR")] wheel_speed_rr: f32,
                                      speed:          f32,
                                      power:          f32,
                                      torque:         f32,
                                      boost:          f32,
                                      gear:           u8,
                                      throttle:       f32,
                                      brake:          f32,
                                      hand_brake:     u8,
    #[serde(rename = "carOrdinal")]   car_ordinal:    i32,
    #[serde(rename = "numCylinders")] num_cylinders:  i32,
    #[serde(rename = "racePosition")] race_position:  u8,
    #[serde(rename = "lapNumber")]    lap_number:     u16,

    // Phase 1
    #[serde(rename = "sessionMaxima")] session_maxima: &'a SessionMaxima,
                                       lockup:         &'a LockupDeltas,

    // Phase 2
    #[serde(rename = "lcState")]       lc_state:       &'static str,

    // Phase 3
    #[serde(rename = "redlineRpm")]    redline_rpm:    f32,
    #[serde(rename = "carChanged")]    car_changed:    bool,
    #[serde(rename = "carKey")]        car_key:        &'a str,
    #[serde(rename = "isKnown")]       is_known:       bool,
    #[serde(rename = "carLearned")]    car_learned:    bool,
                                       notification:   &'a str,
}

// ── Tauri command — force relearn current car ─────────────────────────────────
#[tauri::command]
fn force_relearn(flag: tauri::State<Arc<AtomicBool>>) {
    flag.store(true, Ordering::Relaxed);
    println!("[CarLearning] Force relearn requested");
}

// ── UDP listener ──────────────────────────────────────────────────────────────
fn udp_listener(clients: Clients, db_path: PathBuf, force_relearn: Arc<AtomicBool>) {
    let socket = UdpSocket::bind("127.0.0.1:5301").expect("Failed to bind UDP socket");
    println!("[Bridge] Listening for Forza telemetry on 127.0.0.1:5301");

    let packet_count = Arc::new(AtomicU64::new(0));
    let count_clone  = Arc::clone(&packet_count);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let count = count_clone.swap(0, Ordering::Relaxed);
            if count > 0 { println!("[Bridge] UDP packets/sec: {}", count); }
        }
    });

    let mut buffer    = [0u8; 1024];
    let cap           = std::time::Duration::from_micros(12_500);
    let mut last_sent = std::time::Instant::now()
        .checked_sub(cap).unwrap_or(std::time::Instant::now());

    let mut maxima  = MaximaTracker::new();
    let mut lc      = LaunchControl::new();
    let mut learning = CarLearning::new(db_path, force_relearn);

    loop {
        match socket.recv_from(&mut buffer) {
            Ok((size, _)) => {
                packet_count.fetch_add(1, Ordering::Relaxed);

                let now = std::time::Instant::now();
                if now.duration_since(last_sent) < cap { continue; }
                last_sent = now;

                if let Some(t) = parse_telemetry(&buffer[..size]) {
                    let session_maxima = maxima.update(&t);
                    let lockup         = compute_lockup(&t);
                    let lc_state       = lc.process(&t);
                    let lr             = learning.process(&t);

                    let payload = HudPayload {
                        is_race_on:     t.is_race_on,
                        max_rpm:        t.max_rpm,
                        idle_rpm:       t.idle_rpm,
                        rpm:            t.rpm,
                        accel_x:        t.accel_x,
                        accel_y:        t.accel_y,
                        accel_z:        t.accel_z,
                        ang_vel_x:      t.ang_vel_x,
                        ang_vel_y:      t.ang_vel_y,
                        ang_vel_z:      t.ang_vel_z,
                        susp_fl:        t.susp_fl,
                        susp_fr:        t.susp_fr,
                        susp_rl:        t.susp_rl,
                        susp_rr:        t.susp_rr,
                        slip_fl:        t.slip_fl,
                        slip_fr:        t.slip_fr,
                        slip_rl:        t.slip_rl,
                        slip_rr:        t.slip_rr,
                        wheel_speed_fl: t.wheel_speed_fl,
                        wheel_speed_fr: t.wheel_speed_fr,
                        wheel_speed_rl: t.wheel_speed_rl,
                        wheel_speed_rr: t.wheel_speed_rr,
                        speed:          t.speed,
                        power:          t.power,
                        torque:         t.torque,
                        boost:          t.boost,
                        gear:           t.gear,
                        throttle:       t.throttle,
                        brake:          t.brake,
                        hand_brake:     t.hand_brake,
                        car_ordinal:    t.car_ordinal,
                        num_cylinders:  t.num_cylinders,
                        race_position:  t.race_position,
                        lap_number:     t.lap_number,
                        session_maxima: &session_maxima,
                        lockup:         &lockup,
                        lc_state,
                        redline_rpm:    lr.redline_rpm,
                        car_changed:    lr.car_changed,
                        car_key:        &lr.car_key,
                        is_known:       lr.is_known,
                        car_learned:    lr.car_learned,
                        notification:   &lr.notification,
                    };

                    if let Ok(json) = serde_json::to_string(&payload) {
                        let mut clients = clients.lock().unwrap();
                        clients.retain(|tx| tx.try_send(json.clone()).is_ok());
                    }
                }
            }
            Err(e) => eprintln!("[Bridge] Error: {}", e),
        }
    }
}

fn parse_telemetry(data: &[u8]) -> Option<Telemetry> {
    if data.len() < 324 { return None; }
    Some(Telemetry {
        is_race_on:     i32::from_le_bytes(data[0..4].try_into().ok()?),
        max_rpm:        f32::from_le_bytes(data[8..12].try_into().ok()?),
        idle_rpm:       f32::from_le_bytes(data[12..16].try_into().ok()?),
        rpm:            f32::from_le_bytes(data[16..20].try_into().ok()?),
        accel_x:        f32::from_le_bytes(data[20..24].try_into().ok()?),
        accel_y:        f32::from_le_bytes(data[24..28].try_into().ok()?),
        accel_z:        f32::from_le_bytes(data[28..32].try_into().ok()?),
        ang_vel_x:      f32::from_le_bytes(data[44..48].try_into().ok()?),
        ang_vel_y:      f32::from_le_bytes(data[48..52].try_into().ok()?),
        ang_vel_z:      f32::from_le_bytes(data[52..56].try_into().ok()?),
        susp_fl:        f32::from_le_bytes(data[68..72].try_into().ok()?),
        susp_fr:        f32::from_le_bytes(data[72..76].try_into().ok()?),
        susp_rl:        f32::from_le_bytes(data[76..80].try_into().ok()?),
        susp_rr:        f32::from_le_bytes(data[80..84].try_into().ok()?),
        slip_fl:        f32::from_le_bytes(data[84..88].try_into().ok()?),
        slip_fr:        f32::from_le_bytes(data[88..92].try_into().ok()?),
        slip_rl:        f32::from_le_bytes(data[92..96].try_into().ok()?),
        slip_rr:        f32::from_le_bytes(data[96..100].try_into().ok()?),
        wheel_speed_fl: f32::from_le_bytes(data[100..104].try_into().ok()?),
        wheel_speed_fr: f32::from_le_bytes(data[104..108].try_into().ok()?),
        wheel_speed_rl: f32::from_le_bytes(data[108..112].try_into().ok()?),
        wheel_speed_rr: f32::from_le_bytes(data[112..116].try_into().ok()?),
        speed:          f32::from_le_bytes(data[256..260].try_into().ok()?),
        power:          f32::from_le_bytes(data[260..264].try_into().ok()?) / 745.7,
        torque:         f32::from_le_bytes(data[264..268].try_into().ok()?),
        boost:          f32::from_le_bytes(data[284..288].try_into().ok()?),
        gear:           data[319],
        throttle:       data[315] as f32 / 255.0,
        brake:          data[316] as f32 / 255.0,
        hand_brake:     data[318],
        car_ordinal:    i32::from_le_bytes(data[212..216].try_into().ok()?),
        num_cylinders:  i32::from_le_bytes(data[228..232].try_into().ok()?),
        lap_number:     u16::from_le_bytes(data[312..314].try_into().ok()?),
        race_position:  data[314],
    })
}

async fn websocket_server(clients: Clients) {
    let listener = TcpListener::bind("127.0.0.1:8765").await.expect("Failed to bind WebSocket");
    println!("[WebSocket] Running on ws://127.0.0.1:8765");

    while let Ok((stream, _)) = listener.accept().await {
        let clients = clients.clone();
        tokio::spawn(async move {
            let ws_stream = match accept_async(stream).await {
                Ok(ws) => ws,
                Err(e) => { eprintln!("[WebSocket] Error: {}", e); return; }
            };
            let (mut ws_tx, _ws_rx) = ws_stream.split();
            let (tx, mut rx) = tokio::sync::mpsc::channel(2);
            {
                let mut clients = clients.lock().unwrap();
                clients.clear();
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
    let clients: Clients      = Arc::new(Mutex::new(Vec::new()));
    let force_relearn_flag    = Arc::new(AtomicBool::new(false));

    let clients_ws = clients.clone();
    tokio::spawn(async move { websocket_server(clients_ws).await; });

    let clients_setup      = clients.clone();
    let relearn_setup      = Arc::clone(&force_relearn_flag);
    let relearn_state      = Arc::clone(&force_relearn_flag);

    tauri::Builder::default()
        .manage(relearn_state)
        .setup(move |app| {
            let db_path = app.path().app_data_dir()?.join("car_redlines.json");
            println!("[CarLearning] Database path: {}", db_path.display());
            let clients_udp  = clients_setup.clone();
            let relearn_udp  = Arc::clone(&relearn_setup);
            std::thread::spawn(move || {
                udp_listener(clients_udp, db_path, relearn_udp);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![force_relearn])
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
