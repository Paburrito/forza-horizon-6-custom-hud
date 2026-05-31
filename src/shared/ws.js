// =============================================================================
// shared/ws.js
// WebSocket connection to the Rust bridge.
// Parses each incoming packet and dispatches a 'telemetry' CustomEvent on
// window so any HUD can listen without knowing about the socket directly.
//
// Usage (in any HUD or root script):
//   import { initWebSocket } from '../shared/ws.js';
//   initWebSocket();
//
//   window.addEventListener('telemetry', (e) => {
//       const data = e.detail; // full parsed telemetry object
//   });
// =============================================================================

const WS_URL = 'ws://localhost:8765';

// Reconnect delay in ms — if the bridge isn't running yet, retry quietly
const RECONNECT_DELAY = 3000;

let _ws = null;


export function initWebSocket() {
    if (_ws && _ws.readyState !== WebSocket.CLOSED) {
        console.log('[WS] Closing existing connection before reconnecting');
        _ws.close();
    }

    _connect();
}

function _connect() {
    _ws = new WebSocket(WS_URL);

    _ws.onopen = () => {
        console.log('[WS] Connected to Forza bridge on', WS_URL);
        window.dispatchEvent(new CustomEvent('ws:connected'));
    };

    _ws.onmessage = (event) => {
        let data;
        try {
            data = JSON.parse(event.data);
        } catch (err) {
            console.error('[WS] Failed to parse packet:', err);
            return;
        }
        // Every listener gets the same parsed object — read-only by convention
        window.dispatchEvent(new CustomEvent('telemetry', { detail: data }));
    };

    _ws.onerror = (e) => {
        // Suppress noise — onclose fires right after and handles reconnect
        console.warn('[WS] Socket error', e);
    };

    _ws.onclose = () => {
        console.warn(`[WS] Disconnected — retrying in ${RECONNECT_DELAY}ms`);
        window.dispatchEvent(new CustomEvent('ws:disconnected'));
        setTimeout(_connect, RECONNECT_DELAY);
    };
}

// Expose on window for non-module scripts that need to check connection state
window.wsIsConnected = () => _ws?.readyState === WebSocket.OPEN;
