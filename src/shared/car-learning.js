// =============================================================================
// shared/car-learning.js
// Per-car/per-tune rev limiter learning with confidence scoring.
//
// Tune identity = carOrdinal + maxRpm (rounded) + numCylinders
// Each unique combination gets its own database entry and confidence score.
// Default redline = maxRpm * 0.93 until confidence reaches TRUSTED (3).
// Learning is fully passive — no tutorial step or deliberate input required.
//
// Dispatches these CustomEvents on window:
//   'car:changed'  — { detail: { carOrdinal, carKey, isKnown, redline, maxRpm } }
//   'car:learned'  — { detail: { carOrdinal, carKey, limiter, redline, confidence } }
//
// Exports processLearning(data) → { redlineRpm, currentCarOrdinal }
// Call this on every telemetry frame before rendering.
// =============================================================================

// ── Persistent car database ───────────────────────────────────────────────────
export let carDatabase =
    JSON.parse(localStorage.getItem('forza_car_redlines') || '{}');

function _saveDatabase() {
    localStorage.setItem('forza_car_redlines', JSON.stringify(carDatabase));
}

// ── Tune identity ─────────────────────────────────────────────────────────────
// Round maxRpm to nearest 100 to avoid floating point key mismatches
// e.g. 16499.99 and 16500.01 both become 16500
function _tuneKey(carOrdinal, maxRpm, numCylinders) {
    const rpm = Math.round((maxRpm ?? 0) / 100) * 100;
    return `car_${carOrdinal}_${rpm}_cyl${numCylinders ?? 0}`;
}

// ── Confidence thresholds ─────────────────────────────────────────────────────
const CONFIDENCE_TRUSTED    = 3;     // >= this: value is reliable, stop learning
const DEFAULT_REDLINE_RATIO = 0.93;  // fallback until confidence is earned

// ── State ─────────────────────────────────────────────────────────────────────
export let currentCarOrdinal   = null;
export let lastSweepCarOrdinal = null;

let _currentCarKey  = null;
let _currentMaxRpm  = 0;
let _lastRaceState  = 0;

// Active learning state
let _isLearning   = false;
let _peakRpm      = 0;
let _startGear    = 0;
let _speedSamples = [];

// ── Helpers ───────────────────────────────────────────────────────────────────
function _defaultRedline(maxRpm) {
    return Math.round((maxRpm ?? 10000) * DEFAULT_REDLINE_RATIO);
}

function _getOrCreateEntry(carKey, maxRpm) {
    if (!carDatabase[carKey]) {
        carDatabase[carKey] = {
            limiter:    Math.round(maxRpm ?? 10000),
            redline:    _defaultRedline(maxRpm),
            maxRpm:     maxRpm,
            confidence: 0,
            timestamp:  Date.now(),
        };
    }
    return carDatabase[carKey];
}

// ── Public — update lastSweepCarOrdinal after a sweep fires ──────────────────
export function markSweepFired(carOrdinal) {
    lastSweepCarOrdinal = carOrdinal;
}

// ── Public — force re-learn current tune ──────────────────────────────────────
export function forceRelearn() {
    if (!_currentCarKey) return;
    if (carDatabase[_currentCarKey]) {
        carDatabase[_currentCarKey].confidence = 0;
        _saveDatabase();
    }
    _isLearning = false;
    _peakRpm    = 0;
    window.showNotification?.('Re-learning mode activated! 🔄 Drive to the limiter.', 4000);
}

// ── Main processor — call once per telemetry frame ────────────────────────────
export function processLearning(data) {

    // ── Guard: ignore invalid frames (cutscenes, menus, paused) ──────────────
    if (!data.carOrdinal || data.carOrdinal === 0 ||
        !data.maxRpm     || data.maxRpm     === 0) {
        return {
            redlineRpm:        _currentMaxRpm * DEFAULT_REDLINE_RATIO || 8000,
            currentCarOrdinal,
        };
    }

    // ── Race state transition ─────────────────────────────────────────────────
    if (data.isRaceOn === 1 && _lastRaceState === 0) {
        console.log('🏁 Race started');
    }
    _lastRaceState = data.isRaceOn;

    // ── Tune / car change detection ───────────────────────────────────────────
    const newKey     = _tuneKey(data.carOrdinal, data.maxRpm, data.numCylinders);
    const tuneChanged = newKey !== _currentCarKey;

    if (tuneChanged) {
        _isLearning    = false;
        _peakRpm       = 0;
        _speedSamples  = [];

        const carChanged    = data.carOrdinal !== currentCarOrdinal;
        currentCarOrdinal   = data.carOrdinal;
        _currentCarKey      = newKey;
        _currentMaxRpm      = data.maxRpm;

        const entry   = _getOrCreateEntry(newKey, data.maxRpm);
        const isKnown = entry.confidence >= CONFIDENCE_TRUSTED;

        console.log(
            `🏎️ ${carChanged ? 'Car' : 'Tune'} changed → ${newKey} ` +
            `(${isKnown
                ? `confidence ${entry.confidence}, redline ${entry.redline} RPM`
                : `estimate ${entry.redline} RPM (confidence ${entry.confidence})`})`
        );

        window.dispatchEvent(new CustomEvent('car:changed', {
            detail: {
                carOrdinal: currentCarOrdinal,
                carKey:     newKey,
                isKnown,
                redline:    entry.redline,
                maxRpm:     data.maxRpm,
                idleRpm:    data.idleRpm,
            }
        }));
    }

    _currentMaxRpm = data.maxRpm;

    const entry      = _getOrCreateEntry(_currentCarKey, data.maxRpm);
    const redlineRpm = entry.redline;

    // ── Skip learning if already trusted ─────────────────────────────────────
    if (entry.confidence >= CONFIDENCE_TRUSTED) {
        return { redlineRpm, currentCarOrdinal };
    }

    // ── Limiter zone detection ────────────────────────────────────────────────
    // Only genuine limiter contact: high RPM + demanding throttle + positive power
    // Filters out: engine braking (power < 0), coasting, rev matching
    const inLimiterZone =
        data.rpm      > data.maxRpm * 0.90 &&
        data.throttle > 0.90               &&
        data.power    > 0                  &&
        data.gear     > 0;

    // Start a learning window when entering the zone
    if (inLimiterZone && !_isLearning) {
        _isLearning   = true;
        _peakRpm      = data.rpm;
        _startGear    = data.gear;
        _speedSamples = [data.speed];
    }

    // ── Active learning window ────────────────────────────────────────────────
    if (_isLearning) {
        if (data.rpm > _peakRpm) _peakRpm = data.rpm;

        _speedSamples.push(data.speed);
        if (_speedSamples.length > 10) _speedSamples.shift();
        const avgSpeed = _speedSamples.reduce((a, b) => a + b) / _speedSamples.length;

        // Clean exit: left the limiter zone naturally
        const exitedCleanly =
            data.gear     !== _startGear   ||
            data.throttle  < 0.90          ||
            data.power     <= 0            ||
            data.rpm       < _peakRpm - 500;

        // Dirty exit: something unexpected happened
        const exitedDirty = Math.abs(data.speed - avgSpeed) > 15;

        if (exitedCleanly && _peakRpm > data.maxRpm * 0.88) {
            // Valid observation — refine the limiter estimate
            const detectedLimiter = Math.round(_peakRpm);
            const existingLimiter = entry.limiter;
            const difference      = Math.abs(detectedLimiter - existingLimiter);

            // Only record if it's close to what we expect (filters outliers)
            if (entry.confidence === 0 || difference < 400) {
                // Weighted blend: new observations carry less weight as confidence grows
                const blended = entry.confidence === 0
                    ? detectedLimiter
                    : Math.round(
                        (existingLimiter * entry.confidence + detectedLimiter) /
                        (entry.confidence + 1)
                    );

                entry.limiter    = blended;
                entry.redline    = Math.round(blended * DEFAULT_REDLINE_RATIO);
                entry.confidence = Math.min(entry.confidence + 1, CONFIDENCE_TRUSTED);
                entry.timestamp  = Date.now();
                _saveDatabase();

                console.log(
                    `📈 Limiter refined: ${blended} RPM → ` +
                    `redline ${entry.redline} RPM ` +
                    `(confidence ${entry.confidence}/${CONFIDENCE_TRUSTED})`
                );

                // Notify only when fully trusted
                if (entry.confidence >= CONFIDENCE_TRUSTED) {
                    window.showNotification?.(
                        `✅ Redline locked in: ${entry.redline} RPM`
                    );
                    window.dispatchEvent(new CustomEvent('car:learned', {
                        detail: {
                            carOrdinal: currentCarOrdinal,
                            carKey:     _currentCarKey,
                            limiter:    blended,
                            redline:    entry.redline,
                            confidence: entry.confidence,
                        }
                    }));
                }
            }

            _isLearning = false;
            _peakRpm    = 0;

        } else if (exitedDirty || (!inLimiterZone && !exitedCleanly)) {
            // Inconclusive — discard without penalising confidence
            _isLearning = false;
            _peakRpm    = 0;
        }
    }

    return { redlineRpm, currentCarOrdinal };
}

// ── Init ──────────────────────────────────────────────────────────────────────
export function initCarLearning() {
    console.log(
        '[CarLearning] Initialized, database has',
        Object.keys(carDatabase).length,
        'entries'
    );
}

// ── Expose on window for non-module script blocks ─────────────────────────────
window.forceRelearn         = forceRelearn;
window.getCarDatabase       = () => carDatabase;
window.getCurrentCarOrdinal = () => currentCarOrdinal;
