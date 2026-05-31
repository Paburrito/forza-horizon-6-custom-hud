// =============================================================================
// shared/car-learning.js
// Per-car rev limiter learning and car change detection.
//
// Dispatches these CustomEvents on window:
//   'car:changed'  — { detail: { carOrdinal, carKey, isKnown } }
//   'car:learned'  — { detail: { carOrdinal, carKey, limiter, redline } }
//
// Exports processLearning(data) → { redlineRpm, currentCarOrdinal }
// Call this on every telemetry frame before rendering.
//
// Usage:
//   import { initCarLearning, processLearning } from '../shared/car-learning.js';
//   initCarLearning();
//
//   window.addEventListener('telemetry', (e) => {
//       const { redlineRpm } = processLearning(e.detail);
//       drawTachometer(e.detail, redlineRpm);
//   });
// =============================================================================

// ── Persistent car database ───────────────────────────────────────────────────
export let carDatabase =
    JSON.parse(localStorage.getItem('forza_car_redlines') || '{}');

function _saveDatabase() {
    localStorage.setItem('forza_car_redlines', JSON.stringify(carDatabase));
}

// ── State ─────────────────────────────────────────────────────────────────────
export let currentCarOrdinal = null;
export let lastSweepCarOrdinal = null;

let _hasLearnedThisSession = new Set();
let _lastRaceState = 0;
let _currentMaxRPM = 10000;

// Limiter learning state machine
let _isLearning    = false;
let _peakRpm       = 0;
let _startGear     = 0;
let _avgSpeed      = 0;
let _speedSamples  = [];
let _lastPowerSign = 1;
let _flipCount     = 0;
let _lastFlipTime  = 0;

// ── Public — update lastSweepCarOrdinal after a sweep fires ──────────────────
export function markSweepFired(carOrdinal) {
    lastSweepCarOrdinal = carOrdinal;
}

// ── Public — force re-learn current car ───────────────────────────────────────
export function forceRelearn() {
    if (currentCarOrdinal === null) return;
    const carKey = `car_${currentCarOrdinal}`;
    _hasLearnedThisSession.delete(carKey);
    _flipCount = 0;
    _isLearning = false;
    window.showNotification?.('Re-learning mode activated! 🔄 Bounce the limiter.', 4000);
}

// ── Main processor — call once per telemetry frame ────────────────────────────
export function processLearning(data) {
    // ── Race state transition ─────────────────────────────────────────────────
    if (data.isRaceOn === 1 && _lastRaceState === 0) {
        _hasLearnedThisSession.clear();
        console.log('🏁 Race started — learning re-enabled');
    }
    _lastRaceState = data.isRaceOn;

    // ── Car change detection ──────────────────────────────────────────────────
    if (data.carOrdinal !== undefined && data.carOrdinal !== 0 && data.carOrdinal !== currentCarOrdinal) {
        currentCarOrdinal = data.carOrdinal;
        _isLearning = false;
        _peakRpm    = 0;
        _flipCount  = 0;

        const carKey = `car_${currentCarOrdinal}`;
        const isKnown = !!carDatabase[carKey];

        console.log(`🏎️ Car changed: ${currentCarOrdinal}${isKnown ? ` (known: ${carDatabase[carKey].limiter} RPM)` : ' (new)'}`);

        window.dispatchEvent(new CustomEvent('car:changed', {
            detail: { 
                carOrdinal: currentCarOrdinal, 
                carKey, 
                isKnown,
                redline: isKnown ? carDatabase[carKey].redline : null,
                maxRpm:  data.maxRpm,
                idleRpm: data.idleRpm,
            }
        }));
    }

    if (data.maxRpm && data.maxRpm !== _currentMaxRPM) {
        _currentMaxRPM = data.maxRpm;
    }

    const carKey   = `car_${currentCarOrdinal}`;
    const carData  = carDatabase[carKey];
    const redlineRpm = carData ? carData.redline : (data.maxRpm ?? _currentMaxRPM) * 0.85;

    // ── Power flip detection (limiter bounce) ─────────────────────────────────
    const currentPowerSign = data.power >= 0 ? 1 : -1;
    const powerFlipped =
        currentPowerSign !== _lastPowerSign &&
        data.throttle > 0.95 &&
        data.gear > 0;
    _lastPowerSign = currentPowerSign;

    if (powerFlipped && data.rpm > data.maxRpm * 0.7) {
        const now = Date.now();
        _flipCount = (now - _lastFlipTime < 3000) ? _flipCount + 1 : 1;
        _lastFlipTime = now;

        if (
            _flipCount >= 6 &&
            !_isLearning &&
            !_hasLearnedThisSession.has(carKey) &&
            data.isRaceOn === 1
        ) {
            _isLearning  = true;
            _peakRpm     = data.rpm;
            _startGear   = data.gear;
            _avgSpeed    = data.speed;
            _speedSamples = [data.speed];
            window.showNotification?.('🔴 Sustained limiter bounce detected! Learning...');
        }
    }

    if (data.throttle < 0.9 || data.rpm < data.maxRpm * 0.7) _flipCount = 0;

    // ── Active learning ───────────────────────────────────────────────────────
    if (_isLearning) {
        if (data.rpm > _peakRpm) _peakRpm = data.rpm;

        _speedSamples.push(data.speed);
        if (_speedSamples.length > 10) _speedSamples.shift();
        _avgSpeed = _speedSamples.reduce((a, b) => a + b) / _speedSamples.length;

        const done =
            data.gear   !== _startGear              ||   // gear changed
            data.throttle < 0.9                     ||   // throttle lifted
            Math.abs(data.speed - _avgSpeed) > 10   ||   // speed drifted
            data.rpm < _peakRpm - 500;                   // rpm dropped

        if (done) {
            const detectedLimiter  = Math.round(_peakRpm);
            const existingLimiter  = carDatabase[carKey]?.limiter || 0;
            const difference       = Math.abs(detectedLimiter - existingLimiter);

            if (!carDatabase[carKey] || difference > 500) {
                const detectedRedline = Math.round(detectedLimiter * 0.93);
                carDatabase[carKey] = {
                    limiter:   detectedLimiter,
                    redline:   detectedRedline,
                    maxRpm:    data.maxRpm,
                    timestamp: Date.now(),
                };
                _saveDatabase();

                window.showNotification?.(
                    `✅ LEARNED! Car ${currentCarOrdinal}: Limiter ${detectedLimiter} RPM, Redline ${detectedRedline} RPM`
                );
                window.dispatchEvent(new CustomEvent('car:learned', {
                    detail: {
                        carOrdinal: currentCarOrdinal,
                        carKey,
                        limiter: detectedLimiter,
                        redline: detectedRedline,
                    }
                }));
            } else {
                console.log(`ℹ️ Limiter ${detectedLimiter} matches known value (${existingLimiter})`);
            }

            _hasLearnedThisSession.add(carKey);
            _isLearning = false;
            _peakRpm    = 0;
            _flipCount  = 0;
        }
    }

    return { redlineRpm, currentCarOrdinal };
}

// ── Init ──────────────────────────────────────────────────────────────────────
export function initCarLearning() {
    console.log('[CarLearning] Initialized, database has', Object.keys(carDatabase).length, 'cars');
}

// ── Expose on window for non-module script blocks (tutorial, hotkeys) ─────────
window.forceRelearn       = forceRelearn;
window.getCarDatabase     = () => carDatabase;
window.getCurrentCarOrdinal = () => currentCarOrdinal;
