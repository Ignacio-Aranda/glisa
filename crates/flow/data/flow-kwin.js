// Glisa Flow: reports when the pointer reaches an outer edge of the
// desktop. Loaded at runtime by Glisa through org.kde.KWin /Scripting
// (never installed) and unloaded when Flow is turned off. Read-only: it only
// reads the cursor position and the screen layout and calls Glisa over
// D-Bus. The @...@ values are filled in when the script is loaded.
var SERVICE = @SERVICE@;
var PATH = @PATH@;
var IFACE = @IFACE@;
var RELEASE = @RELEASE@;    // px away from the edge before it counts as left
var RESEND_PX = 24;         // re-report while moving along the edge
var RESEND_MS = 120;
var V = 1;

var state = null;           // side the pointer is at, or null
var stateRect = null;
var lastX = 1e9, lastY = 1e9, lastT = 0;

function send(obj) {
    obj.v = V;
    callDBus(SERVICE, PATH, IFACE, "Event", JSON.stringify(obj));
}

function rectOf(g) {
    return {x: Math.round(g.x), y: Math.round(g.y), w: Math.round(g.width), h: Math.round(g.height)};
}

function screens() {
    var out = [];
    var list = workspace.screens || [];
    for (var i = 0; i < list.length && i < 16; i++) {
        out.push(rectOf(list[i].geometry));
    }
    return out;
}

function outputAt(rects, x, y) {
    for (var i = 0; i < rects.length; i++) {
        var r = rects[i];
        if (x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h) {
            return r;
        }
    }
    return null;
}

function edgeAt(rects, x, y) {
    var o = outputAt(rects, x, y);
    if (!o) {
        return null;
    }
    if (x < o.x + 1 && !outputAt(rects, o.x - 1, y)) return {side: "left", rect: o};
    if (x >= o.x + o.w - 1 && !outputAt(rects, o.x + o.w, y)) return {side: "right", rect: o};
    if (y < o.y + 1 && !outputAt(rects, x, o.y - 1)) return {side: "top", rect: o};
    if (y >= o.y + o.h - 1 && !outputAt(rects, x, o.y + o.h)) return {side: "bottom", rect: o};
    return null;
}

function distanceFrom(side, r, x, y) {
    if (side === "left") return x - r.x;
    if (side === "right") return r.x + r.w - 1 - x;
    if (side === "top") return y - r.y;
    return r.y + r.h - 1 - y;
}

function movingWindow() {
    // Dragging or resizing a window to the edge (e.g. quick tiling) is not a switch.
    var w = workspace.activeWindow;
    return !!(w && (w.move || w.resize));
}

function onMove() {
    var p = workspace.cursorPos;
    var x = p.x, y = p.y;
    var rects = screens();
    var hit = movingWindow() ? null : edgeAt(rects, x, y);
    var now = Date.now();
    if (hit) {
        var moved = Math.abs(x - lastX) + Math.abs(y - lastY);
        if (hit.side !== state || moved >= RESEND_PX || (moved > 0 && now - lastT >= RESEND_MS)) {
            if (state !== null && hit.side !== state) {
                send({type: "leave", side: state});
            }
            state = hit.side;
            stateRect = hit.rect;
            lastX = x; lastY = y; lastT = now;
            send({type: "edge", side: hit.side, x: Math.round(x), y: Math.round(y), screen: hit.rect});
        } else {
            stateRect = hit.rect;
        }
    } else if (state !== null) {
        var r = stateRect;
        var inside = r && x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
        if (!inside || distanceFrom(state, r, x, y) > RELEASE) {
            send({type: "leave", side: state});
            state = null;
            stateRect = null;
            lastX = 1e9; lastY = 1e9;
        }
    }
}

function onScreens() {
    send({type: "screens", screens: screens()});
}

workspace.cursorPosChanged.connect(onMove);
if (workspace.screensChanged) {
    workspace.screensChanged.connect(onScreens);
}

(function hello() {
    var p = workspace.cursorPos;
    var w = workspace.activeWindow;
    send({type: "hello", cursor: {x: Math.round(p.x), y: Math.round(p.y)}, screens: screens(),
          api: {screens: !!workspace.screens, activeWindowMove: !!w && typeof w.move === "boolean"}});
})();
