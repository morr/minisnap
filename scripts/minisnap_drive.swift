// Synthetic input driver for live minisnap verification (see the
// "Живая проверка" section of .zcode/skills/minisnap-capture/SKILL.md).
// Posts real HID-level mouse/keyboard events, lists on-screen windows and
// screens, and checks the Accessibility trust — everything a session needs
// to drive the running app without the user.
//
// Build and run:
//   swiftc -O -o /tmp/minisnap_drive scripts/minisnap_drive.swift
//   /tmp/minisnap_drive click 1912,794
//
// Modes:
//   axcheck                    — AXIsProcessTrusted (synthetic input needs it)
//   move|down|drag|up x,y      — single mouse events, global top-left points
//   click x,y                  — move + down + up at the point
//   key <keycode> <flags>      — key press; flags: 2=shift 4=ctrl 8=alt 16=cmd
//                                (e.g. ⌘⌥4: `key 21 24`). Note: global
//                                hotkeys do not fire from synthetic keys —
//                                trigger captures via the tray menu instead.
//   windows                    — window server list (CGWindowList), layer 0,
//                                front-to-back: pid, owner, frame
//   screens                    — NSScreen frames, scales, main display pixels
//
// App hotkeys for reference: ⌘⌥4 capture, ⌘⌥5 capture+edit, ⌘⌥0 open last.

import AppKit
import CoreGraphics
import Foundation

let args = CommandLine.arguments
let mode = args.count > 1 ? args[1] : ""

func point(_ s: String) -> CGPoint {
    let parts = s.split(separator: ",").map { Double($0.trimmingCharacters(in: .whitespaces))! }
    return CGPoint(x: parts[0], y: parts[1])
}

func post(_ e: CGEvent?) {
    e?.post(tap: .cghidEventTap)
    usleep(30_000)
}

func mouse(_ type: CGEventType, _ p: CGPoint) {
    let e = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: p, mouseButton: .left)
    post(e)
}

switch mode {
case "axcheck":
    print("AXIsProcessTrusted:", AXIsProcessTrusted())
case "move":
    mouse(.mouseMoved, point(args[2]))
case "down":
    mouse(.leftMouseDown, point(args[2]))
case "drag":
    mouse(.leftMouseDragged, point(args[2]))
case "up":
    mouse(.leftMouseUp, point(args[2]))
case "click":
    let p = point(args[2])
    mouse(.mouseMoved, p)
    usleep(80_000)
    mouse(.leftMouseDown, p)
    usleep(80_000)
    mouse(.leftMouseUp, p)
case "key":
    // key <virtualKeycode> <flagsInt> — flags: 2=shift 4=control 8=alt 16=cmd
    let kc = CGKeyCode(UInt16(args[2])!)
    let cf = CGEventFlags(rawValue: args.count > 3 ? UInt64(args[3])! : 0)
    let down = CGEvent(keyboardEventSource: nil, virtualKey: kc, keyDown: true)
    down?.flags = cf
    post(down)
    usleep(60_000)
    let up = CGEvent(keyboardEventSource: nil, virtualKey: kc, keyDown: false)
    up?.flags = cf
    post(up)
case "windows":
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
    for w in list {
        let layer = w[kCGWindowLayer as String] as? Int ?? -1
        if layer != 0 { continue }
        let pid = w[kCGWindowOwnerPID as String] as? Int ?? 0
        let b = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
        let name = w[kCGWindowOwnerName as String] as? String ?? "?"
        print("\(pid)\t\(name)\t\(b["X"] ?? 0),\(b["Y"] ?? 0),\(b["Width"] ?? 0),\(b["Height"] ?? 0)")
    }
case "screens":
    for s in NSScreen.screens {
        print("screen frame=\(s.frame) scale=\(s.backingScaleFactor)")
    }
    let id = CGMainDisplayID()
    print("main display pixels: \(CGDisplayPixelsWide(id))x\(CGDisplayPixelsHigh(id))")
default:
    print("modes: axcheck|move|down|drag|up|click|key|windows|screens <args>")
}
