//! V8-based JavaScript runtime for executing 17track's sign generation module.
//!
//! Uses `deno_core` to embed a V8 engine that can run the obfuscated fingerprint
//! JS module, with mocked browser globals (navigator, screen, document, canvas).
//!
//! The sign module (chunk 839 / ff19fa74) contains an embedded WASM binary using
//! wasm-bindgen. The module's JS wrapper has a stale Uint8Array cache issue with
//! WASM memory views, so we bypass it and call the raw WASM exports directly,
//! reading the result string from WASM linear memory ourselves.

use anyhow::Result;
use deno_core::{JsRuntime, PollEventLoopOptions, RuntimeOptions};

/// Browser mocks script that provides fake DOM/browser globals.
///
/// The sign module probes various browser APIs during fingerprint generation.
/// We provide deterministic mock values that produce a valid sign.
const BROWSER_MOCKS: &str = r#"
// DOM class constructors (must be defined before mocks for instanceof checks)
globalThis.EventTarget = function EventTarget() {};
globalThis.Node = function Node() {};
Node.prototype = Object.create(EventTarget.prototype);
globalThis.Element = function Element() {};
Element.prototype = Object.create(Node.prototype);
globalThis.HTMLElement = function HTMLElement() {};
HTMLElement.prototype = Object.create(Element.prototype);
globalThis.HTMLCanvasElement = function HTMLCanvasElement() {};
HTMLCanvasElement.prototype = Object.create(HTMLElement.prototype);
globalThis.CanvasRenderingContext2D = function CanvasRenderingContext2D() {};
globalThis.WebGLRenderingContext = function WebGLRenderingContext() {};
globalThis.Document = function Document() {};
globalThis.HTMLDocument = function HTMLDocument() {};
HTMLDocument.prototype = Object.create(Document.prototype);
globalThis.Window = function Window() {};
Object.setPrototypeOf(globalThis, Window.prototype);

// Patch WebAssembly.instantiate to:
// 1. Force all wbg instanceof checks to return true (the module checks
//    HTMLCanvasElement, CanvasRenderingContext2D, WebGLRenderingContext, Window)
// 2. Capture the WASM instance exports for direct memory access
(function() {
    var _origInstantiate = WebAssembly.instantiate;
    globalThis.__wasmInstance = null;
    globalThis.__wasmMemory = null;

    function patchImports(imports) {
        if (!imports || !imports.wbg) return imports;
        var wbg = imports.wbg;
        for (var key in wbg) {
            if (wbg.hasOwnProperty(key) && key.indexOf("instanceof") !== -1) {
                wbg[key] = function() { return 1; };
            }
        }
        return imports;
    }

    WebAssembly.instantiate = function(source, imports) {
        var result = _origInstantiate.call(WebAssembly, source, patchImports(imports));
        if (result && typeof result.then === "function") {
            return result.then(function(r) {
                var inst = r.instance || r;
                globalThis.__wasmInstance = inst;
                if (inst.exports && inst.exports.memory) {
                    globalThis.__wasmMemory = inst.exports.memory;
                }
                return r;
            });
        }
        return result;
    };
})();

// Core globals
globalThis.window = globalThis;
globalThis.self = globalThis;

// Navigator mock
globalThis.navigator = {
    userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
    language: "en-US",
    languages: ["en-US", "en"],
    platform: "Win32",
    hardwareConcurrency: 8,
    maxTouchPoints: 0,
    webdriver: false,
    cookieEnabled: true,
    doNotTrack: null,
    vendor: "Google Inc.",
    vendorSub: "",
    productSub: "20030107",
    appVersion: "5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
    appName: "Netscape",
    appCodeName: "Mozilla",
    onLine: true,
    mimeTypes: { length: 0 },
    plugins: { length: 0, refresh: function() {} },
    javaEnabled: function() { return false; },
    getGamepads: function() { return []; },
    mediaDevices: { enumerateDevices: async function() { return []; } },
    connection: { effectiveType: "4g", downlink: 10, rtt: 50 },
    getBattery: async function() { return { charging: true, chargingTime: 0, dischargingTime: Infinity, level: 1 }; },
    permissions: { query: async function() { return { state: "prompt" }; } },
    userAgentData: {
        brands: [
            { brand: "Chromium", version: "143" },
            { brand: "Google Chrome", version: "143" },
            { brand: "Not?A_Brand", version: "99" }
        ],
        mobile: false,
        platform: "Windows",
        getHighEntropyValues: async function() {
            return {
                architecture: "x86",
                bitness: "64",
                model: "",
                platformVersion: "15.0.0",
                uaFullVersion: "143.0.0.0",
                fullVersionList: [
                    { brand: "Chromium", version: "143.0.0.0" },
                    { brand: "Google Chrome", version: "143.0.0.0" }
                ]
            };
        }
    },
    storage: { estimate: async function() { return { quota: 1073741824, usage: 0 }; } },
    clipboard: { readText: async function() { return ""; } },
    locks: { request: async function() {} },
    sendBeacon: function() { return true; },
    requestMediaKeySystemAccess: undefined,
};

// Screen mock
globalThis.screen = {
    width: 1920,
    height: 1080,
    availWidth: 1920,
    availHeight: 1040,
    colorDepth: 24,
    pixelDepth: 24,
    orientation: { type: "landscape-primary", angle: 0 },
};

// Location mock
globalThis.location = {
    href: "https://t.17track.net/en",
    hostname: "t.17track.net",
    host: "t.17track.net",
    origin: "https://t.17track.net",
    protocol: "https:",
    pathname: "/en",
    search: "",
    hash: "",
    port: "",
};

// Document mock with canvas support
globalThis.document = {
    cookie: "",
    referrer: "",
    title: "",
    domain: "t.17track.net",
    URL: "https://t.17track.net/en",
    documentElement: {
        style: {},
        getAttribute: function() { return null; },
        classList: { add: function(){}, remove: function(){}, contains: function(){ return false; } },
    },
    head: { appendChild: function(){} },
    body: { appendChild: function(){}, removeChild: function(){}, style: {} },
    createElement: function(tag) {
        // The WASM sign module creates canvas elements. Due to the stale memory view
        // issue, tag names passed from WASM may be corrupted. Default to canvas.
        var t = (tag || "").replace(/\0/g, "").trim().toLowerCase();
        if (t === "canvas" || t === "" || tag.length > 20) return _createMockCanvas();
        if (t === "div") {
            return {
                style: {}, innerHTML: "",
                appendChild: function(){ return this; }, removeChild: function(){},
                setAttribute: function(){}, getAttribute: function(){ return null; },
                getBoundingClientRect: function(){ return {x:0,y:0,width:0,height:0,top:0,left:0,bottom:0,right:0}; },
                children: [], childNodes: [], parentNode: null, offsetWidth: 0, offsetHeight: 0,
            };
        }
        return {
            style: {}, setAttribute: function(){}, getAttribute: function(){ return null; },
            appendChild: function(){ return this; }, removeChild: function(){},
            addEventListener: function(){}, removeEventListener: function(){},
        };
    },
    getElementById: function() { return null; },
    querySelector: function() { return null; },
    querySelectorAll: function() { return []; },
    getElementsByTagName: function() { return []; },
    addEventListener: function(){},
    removeEventListener: function(){},
    createEvent: function() { return { initEvent: function(){} }; },
    createTextNode: function() { return {}; },
    hasFocus: function() { return true; },
    hidden: false,
    visibilityState: "visible",
};

// Canvas mock - must pass instanceof HTMLCanvasElement
function _createMockCanvas() {
    var canvas = Object.create(HTMLCanvasElement.prototype);
    canvas.width = 300;
    canvas.height = 150;
    canvas.style = {};
    canvas.getContext = function(type) {
        var t = (type || "").replace(/\0/g, "").trim().toLowerCase();
        if (t === "2d") return _createMock2DContext();
        if (t === "webgl" || t === "experimental-webgl") return _createMockWebGLContext();
        // Corrupted type string from WASM - return combined 2D+WebGL context
        var ctx = _createMock2DContext();
        var gl = _createMockWebGLContext();
        for (var k in gl) { if (gl.hasOwnProperty(k) && !ctx.hasOwnProperty(k)) ctx[k] = gl[k]; }
        return ctx;
    };
    canvas.toDataURL = function() {
        return "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    };
    canvas.toBlob = function(cb) { cb(new Blob([""], {type: "image/png"})); };
    canvas.setAttribute = function(){};
    canvas.getAttribute = function(){ return null; };
    return canvas;
}

function _createMock2DContext() {
    var ctx = Object.create(CanvasRenderingContext2D.prototype);
    ctx.fillStyle = ""; ctx.strokeStyle = ""; ctx.font = "10px sans-serif";
    ctx.textBaseline = "alphabetic"; ctx.textAlign = "start";
    ctx.globalAlpha = 1; ctx.globalCompositeOperation = "source-over";
    ctx.lineWidth = 1; ctx.lineCap = "butt"; ctx.lineJoin = "miter";
    ctx.shadowBlur = 0; ctx.shadowColor = "rgba(0, 0, 0, 0)";
    ctx.shadowOffsetX = 0; ctx.shadowOffsetY = 0;
    ctx.fillRect = function(){}; ctx.clearRect = function(){};
    ctx.strokeRect = function(){}; ctx.beginPath = function(){};
    ctx.closePath = function(){}; ctx.moveTo = function(){};
    ctx.lineTo = function(){}; ctx.arc = function(){};
    ctx.arcTo = function(){}; ctx.rect = function(){};
    ctx.fill = function(){}; ctx.stroke = function(){};
    ctx.clip = function(){}; ctx.fillText = function(){};
    ctx.strokeText = function(){};
    ctx.measureText = function(text) { return { width: text.length * 6 }; };
    ctx.getImageData = function(x, y, w, h) {
        return { data: new Uint8ClampedArray(w * h * 4), width: w, height: h };
    };
    ctx.putImageData = function(){};
    ctx.createLinearGradient = function(){ return { addColorStop: function(){} }; };
    ctx.createRadialGradient = function(){ return { addColorStop: function(){} }; };
    ctx.createPattern = function(){ return {}; };
    ctx.drawImage = function(){}; ctx.save = function(){};
    ctx.restore = function(){}; ctx.translate = function(){};
    ctx.rotate = function(){}; ctx.scale = function(){};
    ctx.transform = function(){}; ctx.setTransform = function(){};
    ctx.isPointInPath = function(){ return false; };
    ctx.canvas = { width: 300, height: 150 };
    return ctx;
}

function _createMockWebGLContext() {
    var _ext = { UNMASKED_VENDOR_WEBGL: 0x9245, UNMASKED_RENDERER_WEBGL: 0x9246 };
    var gl = Object.create(WebGLRenderingContext.prototype);
    gl.getExtension = function(name) {
        if (name === "WEBGL_debug_renderer_info") return _ext;
        if (name === "EXT_texture_filter_anisotropic") return { MAX_TEXTURE_MAX_ANISOTROPY_EXT: 0x84FF };
        return null;
    };
    gl.getParameter = function(param) {
        if (param === 0x9245) return "Google Inc. (NVIDIA)";
        if (param === 0x9246) return "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)";
        if (param === 0x1F01) return "WebKit WebGL";
        if (param === 0x1F00) return "WebKit";
        if (param === 0x1F02) return "OpenGL ES 2.0 (WebGL 1.0)";
        if (param === 0x8B8C) return "WebGL GLSL ES 1.0";
        if (param === 0x0D33) return 16384; if (param === 0x0D3A) return 16;
        if (param === 0x8869) return 16; if (param === 0x8B4C) return 1024;
        if (param === 0x8DFD) return 30; if (param === 0x8872) return 16;
        if (param === 0x8B4A) return 16; if (param === 0x8871) return 32;
        if (param === 0x8B49) return 4096; if (param === 0x851C) return 16;
        if (param === 0x0B71) return 1;
        return 0;
    };
    gl.getSupportedExtensions = function() {
        return ["ANGLE_instanced_arrays", "EXT_blend_minmax", "EXT_color_buffer_half_float",
                "EXT_float_blend", "EXT_frag_depth", "EXT_shader_texture_lod",
                "EXT_texture_filter_anisotropic", "OES_element_index_uint",
                "OES_standard_derivatives", "OES_texture_float", "OES_texture_float_linear",
                "OES_texture_half_float", "OES_texture_half_float_linear",
                "OES_vertex_array_object", "WEBGL_color_buffer_float",
                "WEBGL_compressed_texture_s3tc", "WEBGL_debug_renderer_info",
                "WEBGL_depth_texture", "WEBGL_draw_buffers", "WEBGL_lose_context"];
    };
    gl.createBuffer = function(){ return {}; }; gl.createProgram = function(){ return {}; };
    gl.createShader = function(){ return {}; }; gl.shaderSource = function(){};
    gl.compileShader = function(){}; gl.getShaderParameter = function(){ return true; };
    gl.attachShader = function(){}; gl.linkProgram = function(){};
    gl.getProgramParameter = function(){ return true; }; gl.useProgram = function(){};
    gl.bindBuffer = function(){}; gl.bufferData = function(){};
    gl.enableVertexAttribArray = function(){}; gl.vertexAttribPointer = function(){};
    gl.drawArrays = function(){}; gl.getAttribLocation = function(){ return 0; };
    gl.getUniformLocation = function(){ return {}; }; gl.uniform1f = function(){};
    gl.viewport = function(){}; gl.clearColor = function(){};
    gl.clear = function(){}; gl.enable = function(){};
    gl.disable = function(){}; gl.blendFunc = function(){};
    gl.readPixels = function(){};
    gl.canvas = { width: 300, height: 150 };
    gl.drawingBufferWidth = 300; gl.drawingBufferHeight = 150;
    return gl;
}

// Performance mock
globalThis.performance = {
    now: (function() {
        var _start = Date.now();
        return function() { return Date.now() - _start + Math.random() * 0.1; };
    })(),
    timing: { navigationStart: Date.now() - 1000, loadEventEnd: Date.now() },
    getEntriesByType: function() { return []; },
    mark: function(){}, measure: function(){},
};

// Crypto mock
globalThis.crypto = {
    getRandomValues: function(arr) {
        for (var i = 0; i < arr.length; i++) arr[i] = Math.floor(Math.random() * 256);
        return arr;
    },
    subtle: { digest: async function() { return new ArrayBuffer(32); } },
    randomUUID: function() {
        return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, function(c) {
            var r = Math.random() * 16 | 0;
            return (c === "x" ? r : (r & 0x3 | 0x8)).toString(16);
        });
    },
};

// Miscellaneous browser APIs
globalThis.fetch = async function() { return { ok: false, status: 404, text: async function(){ return ""; } }; };
globalThis.XMLHttpRequest = function() {
    this.open = function(){}; this.send = function(){};
    this.setRequestHeader = function(){}; this.readyState = 0;
    this.status = 0; this.responseText = "";
};
globalThis.localStorage = {
    _data: {},
    getItem: function(k){ return this._data[k] || null; },
    setItem: function(k,v){ this._data[k] = String(v); },
    removeItem: function(k){ delete this._data[k]; },
    clear: function(){ this._data = {}; },
    get length(){ return Object.keys(this._data).length; },
    key: function(i){ return Object.keys(this._data)[i] || null; },
};
globalThis.sessionStorage = Object.create(globalThis.localStorage);
globalThis.Intl = globalThis.Intl || {};
globalThis.Intl.DateTimeFormat = globalThis.Intl.DateTimeFormat || function() {
    return { resolvedOptions: function() { return { timeZone: "America/New_York" }; } };
};

// Timer stubs (V8 doesn't provide browser timers)
(function() {
    var _timerId = 0;
    if (typeof globalThis.setTimeout === 'undefined') {
        globalThis.setTimeout = function(cb, ms) { if (typeof cb === 'function') { try { cb(); } catch(e) {} } return ++_timerId; };
    }
    if (typeof globalThis.clearTimeout === 'undefined') globalThis.clearTimeout = function() {};
    if (typeof globalThis.setInterval === 'undefined') globalThis.setInterval = function(cb, ms) { return ++_timerId; };
    if (typeof globalThis.clearInterval === 'undefined') globalThis.clearInterval = function() {};
})();
globalThis.requestAnimationFrame = function(cb) { return setTimeout(cb, 16); };
globalThis.cancelAnimationFrame = function(id) { clearTimeout(id); };
globalThis.addEventListener = function(){};
globalThis.removeEventListener = function(){};
globalThis.dispatchEvent = function(){ return true; };
globalThis.getComputedStyle = function() {
    return new Proxy({}, { get: function(t, p) { return ""; } });
};
globalThis.matchMedia = function() {
    return { matches: false, media: "", addListener: function(){}, removeListener: function(){}, addEventListener: function(){}, removeEventListener: function(){} };
};
globalThis.innerWidth = 1920; globalThis.innerHeight = 1080;
globalThis.outerWidth = 1920; globalThis.outerHeight = 1120;
globalThis.devicePixelRatio = 1;
globalThis.pageXOffset = 0; globalThis.pageYOffset = 0;
globalThis.scrollX = 0; globalThis.scrollY = 0;
globalThis.Blob = globalThis.Blob || function(parts, opts) { this.size = 0; this.type = (opts && opts.type) || ""; };

// TextEncoder/TextDecoder polyfills for the sign module's wasm-bindgen glue
{
    globalThis.TextEncoder = function() {};
    globalThis.TextEncoder.prototype.encode = function(str) {
        var arr = [];
        for (var i = 0; i < str.length; i++) {
            var c = str.charCodeAt(i);
            if (c < 0x80) { arr.push(c); }
            else if (c < 0x800) { arr.push(0xC0 | (c >> 6), 0x80 | (c & 0x3F)); }
            else if (c < 0xD800 || c >= 0xE000) { arr.push(0xE0 | (c >> 12), 0x80 | ((c >> 6) & 0x3F), 0x80 | (c & 0x3F)); }
            else { i++; c = 0x10000 + (((c & 0x3FF) << 10) | (str.charCodeAt(i) & 0x3FF)); arr.push(0xF0 | (c >> 18), 0x80 | ((c >> 12) & 0x3F), 0x80 | ((c >> 6) & 0x3F), 0x80 | (c & 0x3F)); }
        }
        return new Uint8Array(arr);
    };
    globalThis.TextEncoder.prototype.encodeInto = function(str, dest) {
        var encoded = this.encode(str);
        var written = Math.min(encoded.length, dest.length);
        for (var i = 0; i < written; i++) dest[i] = encoded[i];
        return { read: str.length, written: written };
    };
}
{
    globalThis.TextDecoder = function(label) { this.encoding = label || 'utf-8'; };
    globalThis.TextDecoder.prototype.decode = function(buf) {
        if (!buf) return '';
        var bytes = new Uint8Array(buf.buffer || buf);
        var result = '';
        for (var i = 0; i < bytes.length; ) {
            var b = bytes[i];
            if (b < 0x80) { result += String.fromCharCode(b); i++; }
            else if (b < 0xE0) { result += String.fromCharCode(((b & 0x1F) << 6) | (bytes[i+1] & 0x3F)); i += 2; }
            else if (b < 0xF0) { result += String.fromCharCode(((b & 0x0F) << 12) | ((bytes[i+1] & 0x3F) << 6) | (bytes[i+2] & 0x3F)); i += 3; }
            else { var cp = ((b & 0x07) << 18) | ((bytes[i+1] & 0x3F) << 12) | ((bytes[i+2] & 0x3F) << 6) | (bytes[i+3] & 0x3F); cp -= 0x10000; result += String.fromCharCode(0xD800 + (cp >> 10), 0xDC00 + (cp & 0x3FF)); i += 4; }
        }
        return result;
    };
}
globalThis.URL = globalThis.URL || { createObjectURL: function(){ return "blob:null"; }, revokeObjectURL: function(){} };

// Base64 atob/btoa (not in bare V8)
if (typeof atob === 'undefined') {
    globalThis.atob = function(s) {
        var chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=';
        s = String(s).replace(/\s/g, '');
        var out = '', i = 0;
        while (i < s.length) {
            var a = chars.indexOf(s.charAt(i++)), b = chars.indexOf(s.charAt(i++));
            var c = chars.indexOf(s.charAt(i++)), d = chars.indexOf(s.charAt(i++));
            var bits = (a << 18) | (b << 12) | (c << 6) | d;
            out += String.fromCharCode((bits >> 16) & 0xFF);
            if (c !== 64) out += String.fromCharCode((bits >> 8) & 0xFF);
            if (d !== 64) out += String.fromCharCode(bits & 0xFF);
        }
        return out;
    };
}
if (typeof btoa === 'undefined') {
    globalThis.btoa = function(s) {
        var chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
        var out = '';
        for (var i = 0; i < s.length; i += 3) {
            var a = s.charCodeAt(i), b = s.charCodeAt(i+1), c = s.charCodeAt(i+2);
            var bits = (a << 16) | ((b || 0) << 8) | (c || 0);
            out += chars[(bits >> 18) & 63] + chars[(bits >> 12) & 63];
            out += (i+1 < s.length) ? chars[(bits >> 6) & 63] : '=';
            out += (i+2 < s.length) ? chars[bits & 63] : '=';
        }
        return out;
    };
}
globalThis.Worker = undefined;
globalThis.SharedWorker = undefined;
globalThis.ServiceWorker = undefined;
globalThis.AudioContext = undefined;
globalThis.webkitAudioContext = undefined;
globalThis.OfflineAudioContext = undefined;
globalThis.WebSocket = function() { this.close = function(){}; };
globalThis.Request = function Request(url) { this.url = url; };
globalThis.Response = function Response(body, opts) { this.ok = true; this.status = 200; };
globalThis.MutationObserver = function() { this.observe = function(){}; this.disconnect = function(){}; };
globalThis.IntersectionObserver = function() { this.observe = function(){}; this.disconnect = function(){}; };
globalThis.ResizeObserver = function() { this.observe = function(){}; this.disconnect = function(){}; };
globalThis.Image = function() { this.src = ""; this.width = 0; this.height = 0; };
globalThis.Event = function(type) { this.type = type; };
globalThis.CustomEvent = function(type, opts) { this.type = type; this.detail = opts && opts.detail; };

// Set document prototype for instanceof checks
Object.setPrototypeOf(globalThis.document, HTMLDocument.prototype);
globalThis.history = { pushState: function(){}, replaceState: function(){}, back: function(){}, forward: function(){}, length: 1, state: null };
globalThis.process = undefined; // Not Node.js
globalThis.parent = globalThis;
globalThis.top = globalThis;
globalThis.frames = globalThis;
globalThis.opener = null;
globalThis.closed = false;
globalThis.name = "";
globalThis.frameElement = null;
globalThis.origin = "https://t.17track.net";
globalThis.isSecureContext = true;
globalThis.crossOriginIsolated = false;

if (typeof console === 'undefined') {
    globalThis.console = {
        log: function(){}, warn: function(){}, error: function(){},
        info: function(){}, debug: function(){}, trace: function(){},
        dir: function(){}, table: function(){},
    };
}
"#;

/// Webpack interception script that captures the module factory from chunk 839.
///
/// The chunk registers itself via:
/// ```js
/// (self["webpackChunk_N_E"] = self["webpackChunk_N_E"] || []).push([[839], {4279: factory}])
/// ```
/// We intercept the `push()` call to capture the factory and execute it.
const WEBPACK_INTERCEPT: &str = r#"
globalThis.__captured_modules = {};
globalThis.webpackChunk_N_E = globalThis.webpackChunk_N_E || [];

var _origPush = Array.prototype.push;

self["webpackChunk_N_E"] = new Proxy([], {
    get: function(target, prop) {
        if (prop === "push") {
            return function(chunkData) {
                if (Array.isArray(chunkData) && chunkData.length >= 2) {
                    var modules = chunkData[1];
                    if (typeof modules === "object") {
                        for (var moduleId in modules) {
                            if (modules.hasOwnProperty(moduleId)) {
                                __captured_modules[moduleId] = modules[moduleId];
                            }
                        }
                    }
                }
                return _origPush.call(target, chunkData);
            };
        }
        return target[prop];
    },
    set: function(target, prop, value) {
        target[prop] = value;
        return true;
    }
});

// Execute a captured webpack module and return its exports
globalThis.__executeModule = function(moduleId) {
    var factory = __captured_modules[moduleId];
    if (!factory) {
        throw new Error("Module " + moduleId + " not found. Available: " + Object.keys(__captured_modules).join(", "));
    }
    var module = { exports: {} };
    var exports = module.exports;
    var require = function(id) {
        throw new Error("Module " + moduleId + " tried to require(" + id + ")");
    };
    require.r = function(exports) {
        if (typeof Symbol !== "undefined" && Symbol.toStringTag) {
            Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });
        }
        Object.defineProperty(exports, "__esModule", { value: true });
    };
    require.d = function(exports, definition) {
        for (var key in definition) {
            if (definition.hasOwnProperty(key) && !exports.hasOwnProperty(key)) {
                Object.defineProperty(exports, key, { enumerable: true, get: definition[key] });
            }
        }
    };
    require.n = function(module) {
        var getter = module && module.__esModule ? function() { return module["default"]; } : function() { return module; };
        require.d(getter, { a: getter });
        return getter;
    };
    require.o = function(obj, prop) { return Object.prototype.hasOwnProperty.call(obj, prop); };

    factory(module, exports, require);
    return module.exports;
};
"#;

/// Sign generator that uses V8 to execute 17track's fingerprint JS module.
pub struct SignGenerator {
    runtime: JsRuntime,
    initialized: bool,
}

impl SignGenerator {
    /// Create a new V8 runtime with browser mocks.
    pub fn new() -> Result<Self> {
        let runtime = JsRuntime::new(RuntimeOptions::default());

        let mut generator = Self {
            runtime,
            initialized: false,
        };

        // Install browser mocks
        generator
            .runtime
            .execute_script("[browser_mocks]", BROWSER_MOCKS)
            .map_err(|e| anyhow::anyhow!("Failed to install browser mocks: {}", e))?;

        // Install webpack interception
        generator
            .runtime
            .execute_script("[webpack_intercept]", WEBPACK_INTERCEPT)
            .map_err(|e| anyhow::anyhow!("Failed to install webpack intercept: {}", e))?;

        Ok(generator)
    }

    /// Initialize with the sign module JS content.
    ///
    /// Executes the ff19fa74 chunk JS which registers its module factory,
    /// then extracts and initializes the module (including WASM compilation).
    pub async fn initialize(&mut self, sign_module_js: &str) -> Result<()> {
        // Execute the chunk JS - triggers webpackChunk_N_E.push() interception
        self.runtime
            .execute_script("[sign_module]", sign_module_js.to_string())
            .map_err(|e| anyhow::anyhow!("Failed to execute sign module: {}", e))?;

        // Run event loop to handle any async initialization
        self.runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await
            .map_err(|e| anyhow::anyhow!("Event loop error during module load: {}", e))?;

        // Find and execute the module, then call default() to initialize WASM
        let init_script = r#"
            (async function() {
                var moduleExports = null;
                var targetIds = ["4279"];

                for (var i = 0; i < targetIds.length; i++) {
                    if (__captured_modules[targetIds[i]]) {
                        moduleExports = __executeModule(targetIds[i]);
                        break;
                    }
                }

                // Fallback: search all captured modules for get_fingerprint
                if (!moduleExports) {
                    for (var id in __captured_modules) {
                        try {
                            var exports = __executeModule(id);
                            if (exports && exports.get_fingerprint) {
                                moduleExports = exports;
                                break;
                            }
                        } catch(e) {}
                    }
                }

                if (!moduleExports) {
                    throw new Error("Could not find sign module. Captured: " + Object.keys(__captured_modules).join(", "));
                }

                globalThis.__signModule = moduleExports;

                // Call default() to initialize (compiles WASM, sets up exports)
                if (typeof moduleExports.default === "function") {
                    await moduleExports.default();
                }

                // Save references to raw WASM exports for direct memory access.
                // The JS wrapper's string decode uses a cached Uint8Array that becomes
                // stale after WASM memory growth, returning all-zero strings. We bypass
                // this by reading WASM memory directly with fresh views.
                if (globalThis.__wasmInstance) {
                    var exp = globalThis.__wasmInstance.exports;
                    globalThis.__rawWasm = {
                        get_fingerprint: exp.get_fingerprint,
                        stack: exp.__wbindgen_add_to_stack_pointer,
                        memory: exp.memory,
                        free: exp.__wbindgen_export_2  // __wbindgen_free
                    };
                }

                return "ok";
            })()
        "#;

        let result = self
            .runtime
            .execute_script("[init_sign_module]", init_script)
            .map_err(|e| anyhow::anyhow!("Failed to init sign module: {}", e))?;

        let resolved = self.runtime.resolve(result);
        self.runtime
            .with_event_loop_promise(resolved, PollEventLoopOptions::default())
            .await
            .map_err(|e| anyhow::anyhow!("Sign module init failed: {}", e))?;

        self.initialized = true;
        Ok(())
    }

    /// Generate a sign value by calling the WASM get_fingerprint export directly.
    ///
    /// Bypasses the JS wrapper's broken string decode by reading the result
    /// string from WASM linear memory with fresh Uint8Array/Int32Array views.
    pub async fn generate_sign(&mut self) -> Result<String> {
        if !self.initialized {
            anyhow::bail!("SignGenerator not initialized - call initialize() first");
        }

        let gen_script = r#"
            (function() {
                var rw = globalThis.__rawWasm;
                if (!rw || !rw.get_fingerprint || !rw.stack || !rw.memory) {
                    throw new Error("Raw WASM exports not available");
                }

                // Allocate return pointer on the WASM stack
                var retptr = rw.stack(-16);
                try {
                    // Call get_fingerprint(retptr, mousePointsPtr=0, mousePointsLen=0)
                    rw.get_fingerprint(retptr, 0, 0);

                    // Read ptr+len from retptr using FRESH Int32Array view
                    // (avoids stale buffer reference after WASM memory growth)
                    var i32 = new Int32Array(rw.memory.buffer);
                    var ptr = i32[retptr / 4 + 0];
                    var len = i32[retptr / 4 + 1];

                    if (len <= 0 || len > 100000) {
                        throw new Error("Invalid sign length: " + len + " (ptr=" + ptr + ")");
                    }

                    // Decode UTF-8 string from WASM memory with FRESH Uint8Array view
                    var u8 = new Uint8Array(rw.memory.buffer);
                    var bytes = u8.slice(ptr, ptr + len);
                    var sign = new TextDecoder("utf-8").decode(bytes);

                    // Free the WASM-allocated string
                    if (rw.free) {
                        try { rw.free(ptr, len, 1); } catch(e) {}
                    }

                    globalThis.__signResult = sign;
                    return "ok";
                } finally {
                    rw.stack(16); // restore stack pointer
                }
            })()
        "#;

        self.runtime
            .execute_script("[generate_sign]", gen_script)
            .map_err(|e| anyhow::anyhow!("Failed to call get_fingerprint: {}", e))?;

        self.runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await
            .ok();

        // Read the sign result
        let read_script = r#"
            (function() {
                var result = globalThis.__signResult;
                if (result === undefined || result === null) {
                    return JSON.stringify({"error": "Sign generation returned no result"});
                }
                return JSON.stringify({"sign": result});
            })()
        "#;

        let result = self
            .runtime
            .execute_script("[read_sign]", read_script)
            .map_err(|e| anyhow::anyhow!("Failed to read sign result: {}", e))?;

        let json_str: String = {
            let context = self.runtime.main_context();
            let isolate = self.runtime.v8_isolate();
            let mut handle_scope = deno_core::v8::HandleScope::new(isolate);
            let handle_scope = unsafe { std::pin::Pin::new_unchecked(&mut handle_scope) };
            let handle_scope = &mut handle_scope.init();
            let context_local = deno_core::v8::Local::new(handle_scope, context);
            let scope = &mut deno_core::v8::ContextScope::new(handle_scope, context_local);
            let local = deno_core::v8::Local::new(scope, &result);
            let str_val = local
                .to_string(scope)
                .ok_or_else(|| anyhow::anyhow!("V8 result is not a string"))?;
            str_val.to_rust_string_lossy(scope)
        };

        let parsed: serde_json::Value = serde_json::from_str(&json_str).map_err(|e| {
            anyhow::anyhow!(
                "Failed to parse sign result JSON: {} (raw: {})",
                e,
                json_str
            )
        })?;

        if let Some(error) = parsed.get("error").and_then(|v| v.as_str()) {
            anyhow::bail!("Sign generation error: {}", error);
        }

        parsed
            .get("sign")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Sign not found in result: {}", json_str))
    }

    /// Check if the runtime has been initialized with the sign module.
    ///
    /// Returns `true` if `initialize()` has been called successfully and the
    /// V8 runtime is ready to generate signs.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}
