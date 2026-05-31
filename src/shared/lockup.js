// =============================================================================
// shared/lockup.js
// Lockup delta detection — computes how close any individual wheel is to
// locking up under braking, expressed as a 0–1 value.
//
// Usage:
//   import { computeLockupDelta } from '../shared/lockup.js';
//   const delta = computeLockupDelta(data); // 0 = rolling free, 1 = fully locked
// =============================================================================

// Approximate tire radius — consistent enough for ratio math across most cars.
// The exact value matters less than being in the right ballpark since we're
// computing a ratio, not an absolute speed.
export const WHEEL_RADIUS = 0.33; // meters

// Brake input floor — below this we don't care about lockup
// (filters out coasting, creeping, and near-zero pedal touch)
export const BRAKE_THRESHOLD = 0.15; // 0–1

// Speed floor — ignore when nearly stationary
export const SPEED_FLOOR_MS = 5; // m/s (~18 km/h)

// =============================================================================
// computeLockupDelta
// Returns 0–1 representing the worst single wheel's lockup risk.
// 0   = all wheels rolling freely relative to vehicle speed
// 1   = at least one wheel fully locked (0 rad/s while car is still moving)
// =============================================================================
export function computeLockupDelta(data) {
    if (data.brake < BRAKE_THRESHOLD || data.speed < SPEED_FLOOR_MS) return 0;

    const expectedWheelSpeed = data.speed / WHEEL_RADIUS; // rad/s
    if (expectedWheelSpeed < 1) return 0; // guard near-zero division at low speed

    let maxDelta = 0;

    for (const ws of [
        data.wheelSpeedFL,
        data.wheelSpeedFR,
        data.wheelSpeedRL,
        data.wheelSpeedRR,
    ]) {
        // Wheel speeds can be negative in reverse — abs covers that
        const delta = Math.max(0, (expectedWheelSpeed - Math.abs(ws)) / expectedWheelSpeed);
        if (delta > maxDelta) maxDelta = delta;
    }

    return maxDelta; // worst offender across all four wheels
}

// Expose on window for non-module script blocks
window.computeLockupDelta = computeLockupDelta;
