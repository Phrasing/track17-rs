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
