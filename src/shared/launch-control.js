// shared/launch-control.js
// Launch Control detection — shared between all HUDs
// Call processLC(data) each telemetry frame from the root launcher.
// Dispatches window event 'lc:state' with { armed, launched } on state change.

let _prevHandBrake  = 0;
let _lcArmed        = false;
let _lcLaunched     = false;
let _launchTimeout  = null;

export function processLC(data) {
    const hb = data.hand_brake ?? 0;

    const armed = hb > 0
               && (data.power  ?? 0) < -2
               && (data.torque ?? 0) < -2
               && (data.speed  ?? 0) < 0.5;

    const justLaunched = hb === 0
                      && _prevHandBrake > 0
                      && (data.speed ?? 0) > 0
                      && (data.power ?? 0) > 0;

    _prevHandBrake = hb;

    // One-shot trigger → held by timeout only
    if (justLaunched && !_lcLaunched) {
        _lcLaunched = true;
        clearTimeout(_launchTimeout);
        _launchTimeout = setTimeout(() => {
            _lcLaunched = false;
            _dispatch();
        }, 1500);
    }

    // Compare against previous PERSISTENT state
    const prevArmed    = _lcArmed;
    const prevLaunched = _lcLaunched;
    _lcArmed = armed;

    if (_lcArmed !== prevArmed || _lcLaunched !== prevLaunched) {
        _dispatch();
    }
}

function _dispatch() {
    window.dispatchEvent(new CustomEvent('lc:state', {
        detail: { armed: _lcArmed, launched: _lcLaunched }
    }));
}