// Window-list dumps for diagnosing click-to-window capture: the window
// server list (CGWindowList — the source of minisnap's T0 window list: real
// frames, compositor order) side by side with ScreenCaptureKit's
// SCShareableContent list (the source of SCWindow objects for render
// verification). The two disagree for some apps — winit/bevy game windows
// resized by a window manager get stale SCK frames and phantom records,
// brand-new windows are missing from SCK for minutes — and this script is
// how that divergence is seen directly. Window ids are the same
// CGWindowID namespace in both lists.
//
// Run (interpreted, no build step; the SCK dump needs the parent terminal
// to hold the Screen Recording grant):
//   swift scripts/window_dump.swift          # both lists
//   swift scripts/window_dump.swift cg|sck   # one list
//
// Output: <id>  pid=<pid>  <owner>  <x>,<y>,<w>,<h>  layer=<n>[  onScreen=<b>]

import CoreGraphics
import Foundation
import ScreenCaptureKit

let which = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "both"

if which == "cg" || which == "both" {
    guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID)
        as? [[String: Any]] else {
        print("cg: no list")
        exit(1)
    }
    print("— window server (CGWindowList, on-screen, any layer) —")
    for w in list {
        let layer = w[kCGWindowLayer as String] as? Int ?? -1
        let pid = w[kCGWindowOwnerPID as String] as? Int ?? 0
        let num = w[kCGWindowNumber as String] as? Int ?? 0
        let name = w[kCGWindowOwnerName as String] as? String ?? "?"
        let b = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
        print("cg: \(num)\tpid=\(pid)\t\(name)\t\(b["X"] ?? 0),\(b["Y"] ?? 0),\(b["Width"] ?? 0),\(b["Height"] ?? 0)\tlayer=\(layer)")
    }
}

if which == "sck" || which == "both" {
    let sem = DispatchSemaphore(value: 0)
    Task {
        do {
            let content = try await SCShareableContent.current
            print("— ScreenCaptureKit (SCShareableContent, any layer) —")
            for w in content.windows {
                let pid = w.owningApplication?.processID ?? 0
                let app = w.owningApplication?.applicationName ?? "?"
                let f = w.frame
                print("sck: \(w.windowID)\tpid=\(pid)\t\(app)\t\(Int(f.origin.x)),\(Int(f.origin.y)),\(Int(f.width)),\(Int(f.height))\tlayer=\(w.windowLayer)\tonScreen=\(w.isOnScreen)")
            }
        } catch {
            print("sck: error:", error)
        }
        sem.signal()
    }
    sem.wait()
}
